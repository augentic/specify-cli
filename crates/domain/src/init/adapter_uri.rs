//! Parsing the `<adapter>` argument: bare local paths, `file://`
//! URIs, and `https://github.com/...` URIs (with optional `@ref` or
//! `tree/<ref>` discriminators).

use std::fs;
use std::path::{Path, PathBuf};

use specify_error::Error;
use tempfile::TempDir;

use crate::init::git::sparse_checkout_github;

#[derive(Debug)]
pub(super) struct AdapterUri {
    pub(crate) adapter_value: String,
    pub(crate) adapter_name: String,
    pub(crate) source_dir: PathBuf,
    _checkout_guard: Option<TempDir>,
}

impl AdapterUri {
    pub(crate) fn parse(adapter: &str, project_dir: &Path) -> Result<Self, Error> {
        if is_github_url(adapter) {
            return Self::from_github(adapter);
        }
        Self::from_local(adapter, project_dir)
    }

    fn from_local(adapter: &str, project_dir: &Path) -> Result<Self, Error> {
        let path =
            adapter.strip_prefix("file://").map_or_else(|| PathBuf::from(adapter), PathBuf::from);
        let source_dir = if path.is_absolute() { path } else { project_dir.join(path) };
        ensure_adapter_dir(&source_dir, adapter)?;
        let canonical = fs::canonicalize(&source_dir).map_err(|err| Error::Diag {
            code: "adapter-canonicalize-failed",
            detail: format!(
                "failed to canonicalize local adapter `{adapter}` at {}: {err}",
                source_dir.display()
            ),
        })?;
        let adapter_name = adapter_name_from_dir(&canonical)?;
        let adapter_value = format!("file://{}", canonical.display());
        Ok(Self {
            adapter_value,
            adapter_name,
            source_dir: canonical,
            _checkout_guard: None,
        })
    }

    fn from_github(adapter: &str) -> Result<Self, Error> {
        let spec = GithubAdapterUri::parse(adapter)?;
        let repo_url = format!("https://github.com/{}/{}.git", spec.owner, spec.repo);
        let checkout =
            sparse_checkout_github(&repo_url, spec.checkout_ref.as_deref(), &spec.adapter_path)?;
        let source_dir = checkout.path().join(&spec.adapter_path);
        ensure_adapter_dir(&source_dir, adapter)?;

        Ok(Self {
            adapter_value: adapter.to_string(),
            adapter_name: spec.adapter_name,
            source_dir,
            _checkout_guard: Some(checkout),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GithubAdapterUri {
    owner: String,
    repo: String,
    checkout_ref: Option<String>,
    adapter_path: String,
    adapter_name: String,
}

impl GithubAdapterUri {
    fn parse(adapter: &str) -> Result<Self, Error> {
        let (without_suffix, suffix_ref) = split_ref_suffix(adapter);
        let pathless =
            without_suffix.strip_prefix("https://github.com/").ok_or_else(|| Error::Diag {
                code: "adapter-github-uri-unsupported",
                detail: format!("unsupported GitHub adapter URI `{adapter}`"),
            })?;
        let mut parts: Vec<&str> = pathless.split('/').filter(|part| !part.is_empty()).collect();
        if parts.len() < 3 {
            return Err(Error::Diag {
                code: "adapter-github-uri-malformed",
                detail: format!(
                    "GitHub adapter URI `{adapter}` must include owner, repo, and adapter path"
                ),
            });
        }
        let owner = parts.remove(0).to_string();
        let repo = parts.remove(0).to_string();

        let (tree_ref, adapter_parts): (Option<&str>, Vec<&str>) = if parts.first() == Some(&"tree")
        {
            if parts.len() < 3 {
                return Err(Error::Diag {
                    code: "adapter-github-uri-malformed",
                    detail: format!(
                        "GitHub tree adapter URI `{adapter}` must include a ref and adapter path"
                    ),
                });
            }
            (Some(parts[1]), parts[2..].to_vec())
        } else {
            (None, parts)
        };

        let checkout_ref = suffix_ref.or(tree_ref).map(str::to_string);
        let adapter_path = adapter_parts.join("/");
        let adapter_name = adapter_parts.last().ok_or_else(|| Error::Diag {
            code: "adapter-url-name-unresolved",
            detail: format!("cannot derive a adapter name from `{adapter}`"),
        })?;

        Ok(Self {
            owner,
            repo,
            checkout_ref,
            adapter_path,
            adapter_name: (*adapter_name).to_string(),
        })
    }
}

fn is_github_url(adapter: &str) -> bool {
    adapter.starts_with("https://github.com/")
}

fn split_ref_suffix(adapter: &str) -> (&str, Option<&str>) {
    let last_slash = adapter.rfind('/').unwrap_or(0);
    if let Some(at) = adapter.rfind('@')
        && at > last_slash
        && at + 1 < adapter.len()
    {
        return (&adapter[..at], Some(&adapter[at + 1..]));
    }
    (adapter, None)
}

pub fn ensure_adapter_dir(path: &Path, original: &str) -> Result<(), Error> {
    if path.join(crate::adapter::ADAPTER_FILENAME).is_file() {
        return Ok(());
    }
    Err(Error::Diag {
        code: "adapter-dir-missing-manifest",
        detail: format!(
            "adapter `{original}` did not resolve to a directory with `{}` at {}",
            crate::adapter::ADAPTER_FILENAME,
            path.display()
        ),
    })
}

fn adapter_name_from_dir(path: &Path) -> Result<String, Error> {
    path.file_name().and_then(|name| name.to_str()).map(str::to_string).ok_or_else(|| Error::Diag {
        code: "adapter-dir-name-unresolved",
        detail: format!("cannot derive adapter name from {}", path.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_adapter_uri_parses_default_main() {
        let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/schemas/omnia")
            .expect("parse GitHub URI");
        assert_eq!(
            parsed,
            GithubAdapterUri {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
                checkout_ref: None,
                adapter_path: "schemas/omnia".to_string(),
                adapter_name: "omnia".to_string(),
            }
        );
    }

    #[test]
    fn github_adapter_uri_parses_suffix_ref() {
        let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/schemas/omnia@v1")
            .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("v1"));
        assert_eq!(parsed.adapter_path, "schemas/omnia");
        assert_eq!(parsed.adapter_name, "omnia");
    }

    #[test]
    fn github_adapter_uri_parses_tree_ref() {
        let parsed =
            GithubAdapterUri::parse("https://github.com/owner/repo/tree/main/schemas/omnia")
                .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("main"));
        assert_eq!(parsed.adapter_path, "schemas/omnia");
        assert_eq!(parsed.adapter_name, "omnia");
    }
}
