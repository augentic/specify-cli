//! Parsing the `<capability>` argument: bare local paths, `file://`
//! URIs, and `https://github.com/...` URIs (with optional `@ref` or
//! `tree/<ref>` discriminators).

use std::fs;
use std::path::{Path, PathBuf};

use specify_error::Error;

use crate::init::git::sparse_checkout_github;

#[derive(Debug)]
pub(super) struct CapabilityUri {
    pub(crate) capability_value: String,
    pub(crate) capability_name: String,
    pub(crate) source_dir: PathBuf,
}

impl CapabilityUri {
    pub(crate) fn parse(capability: &str, project_dir: &Path) -> Result<Self, Error> {
        if is_github_url(capability) {
            return Self::from_github(capability);
        }
        Self::from_local(capability, project_dir)
    }

    fn from_local(capability: &str, project_dir: &Path) -> Result<Self, Error> {
        let path = capability
            .strip_prefix("file://")
            .map_or_else(|| PathBuf::from(capability), PathBuf::from);
        let source_dir = if path.is_absolute() { path } else { project_dir.join(path) };
        ensure_capability_dir(&source_dir, capability)?;
        let canonical = fs::canonicalize(&source_dir).map_err(|err| Error::Diag {
            code: "capability-canonicalize-failed",
            detail: format!(
                "failed to canonicalize local capability `{capability}` at {}: {err}",
                source_dir.display()
            ),
        })?;
        let capability_name = capability_name_from_dir(&canonical)?;
        let capability_value = format!("file://{}", canonical.display());
        Ok(Self {
            capability_value,
            capability_name,
            source_dir: canonical,
        })
    }

    fn from_github(capability: &str) -> Result<Self, Error> {
        let spec = GithubCapabilityUri::parse(capability)?;
        let repo_url = format!("https://github.com/{}/{}.git", spec.owner, spec.repo);
        let checkout_dir =
            sparse_checkout_github(&repo_url, spec.checkout_ref.as_deref(), &spec.capability_path)?;
        let source_dir = checkout_dir.join(&spec.capability_path);
        ensure_capability_dir(&source_dir, capability)?;

        Ok(Self {
            capability_value: capability.to_string(),
            capability_name: spec.capability_name,
            source_dir,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GithubCapabilityUri {
    owner: String,
    repo: String,
    checkout_ref: Option<String>,
    capability_path: String,
    capability_name: String,
}

impl GithubCapabilityUri {
    fn parse(capability: &str) -> Result<Self, Error> {
        let (without_suffix, suffix_ref) = split_ref_suffix(capability);
        let pathless =
            without_suffix.strip_prefix("https://github.com/").ok_or_else(|| Error::Diag {
                code: "capability-github-uri-unsupported",
                detail: format!("unsupported GitHub capability URI `{capability}`"),
            })?;
        let mut parts: Vec<&str> = pathless.split('/').filter(|part| !part.is_empty()).collect();
        if parts.len() < 3 {
            return Err(Error::Diag {
                code: "capability-github-uri-malformed",
                detail: format!(
                    "GitHub capability URI `{capability}` must include owner, repo, and capability path"
                ),
            });
        }
        let owner = parts.remove(0).to_string();
        let repo = parts.remove(0).to_string();

        let (tree_ref, capability_parts): (Option<&str>, Vec<&str>) = if parts.first()
            == Some(&"tree")
        {
            if parts.len() < 3 {
                return Err(Error::Diag {
                    code: "capability-github-uri-malformed",
                    detail: format!(
                        "GitHub tree capability URI `{capability}` must include a ref and capability path"
                    ),
                });
            }
            (Some(parts[1]), parts[2..].to_vec())
        } else {
            (None, parts)
        };

        let checkout_ref = suffix_ref.or(tree_ref).map(str::to_string);
        let capability_path = capability_parts.join("/");
        let capability_name = capability_parts.last().ok_or_else(|| Error::Diag {
            code: "capability-url-name-unresolved",
            detail: format!("cannot derive a capability name from `{capability}`"),
        })?;

        Ok(Self {
            owner,
            repo,
            checkout_ref,
            capability_path,
            capability_name: (*capability_name).to_string(),
        })
    }
}

fn is_github_url(capability: &str) -> bool {
    capability.starts_with("https://github.com/")
}

fn split_ref_suffix(capability: &str) -> (&str, Option<&str>) {
    let last_slash = capability.rfind('/').unwrap_or(0);
    if let Some(at) = capability.rfind('@')
        && at > last_slash
        && at + 1 < capability.len()
    {
        return (&capability[..at], Some(&capability[at + 1..]));
    }
    (capability, None)
}

pub(super) fn ensure_capability_dir(path: &Path, original: &str) -> Result<(), Error> {
    if path.join(crate::capability::CAPABILITY_FILENAME).is_file() {
        return Ok(());
    }
    Err(Error::Diag {
        code: "capability-dir-missing-manifest",
        detail: format!(
            "capability `{original}` did not resolve to a directory with `{}` at {}",
            crate::capability::CAPABILITY_FILENAME,
            path.display()
        ),
    })
}

fn capability_name_from_dir(path: &Path) -> Result<String, Error> {
    path.file_name().and_then(|name| name.to_str()).map(str::to_string).ok_or_else(|| Error::Diag {
        code: "capability-dir-name-unresolved",
        detail: format!("cannot derive capability name from {}", path.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_capability_uri_parses_default_main() {
        let parsed = GithubCapabilityUri::parse("https://github.com/owner/repo/schemas/omnia")
            .expect("parse GitHub URI");
        assert_eq!(
            parsed,
            GithubCapabilityUri {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
                checkout_ref: None,
                capability_path: "schemas/omnia".to_string(),
                capability_name: "omnia".to_string(),
            }
        );
    }

    #[test]
    fn github_capability_uri_parses_suffix_ref() {
        let parsed = GithubCapabilityUri::parse("https://github.com/owner/repo/schemas/omnia@v1")
            .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("v1"));
        assert_eq!(parsed.capability_path, "schemas/omnia");
        assert_eq!(parsed.capability_name, "omnia");
    }

    #[test]
    fn github_capability_uri_parses_tree_ref() {
        let parsed =
            GithubCapabilityUri::parse("https://github.com/owner/repo/tree/main/schemas/omnia")
                .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("main"));
        assert_eq!(parsed.capability_path, "schemas/omnia");
        assert_eq!(parsed.capability_name, "omnia");
    }
}
