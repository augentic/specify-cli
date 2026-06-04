//! Parsing the `<adapter>` argument: first-party shorthand
//! (`omnia`, `omnia@v1`), bare local paths, `file://` URIs, and
//! `https://github.com/...` URIs (with optional `@ref` or
//! `tree/<ref>` discriminators).
//!
//! First-party shorthand resolves a bare adapter name against the
//! framework repo: an on-disk checkout named by
//! `$SPECIFY_FRAMEWORK_ROOT` when set (offline dev + acceptance), else
//! the canonical published adapter on GitHub (ref defaults to `v1`).

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
        if let Some((name, reference)) = parse_first_party_shorthand(adapter) {
            return Self::from_shorthand(name, reference, framework_root_env().as_deref());
        }
        Self::from_local(adapter, project_dir)
    }

    fn from_local(adapter: &str, project_dir: &Path) -> Result<Self, Error> {
        let path =
            adapter.strip_prefix("file://").map_or_else(|| PathBuf::from(adapter), PathBuf::from);
        let source_dir = if path.is_absolute() { path } else { project_dir.join(path) };
        Self::from_resolved_dir(&source_dir, adapter)
    }

    /// Build an [`AdapterUri`] from an already-resolved local directory,
    /// recording a `file://` `adapter_value`. Shared by [`Self::from_local`]
    /// (bare/`file://` paths) and [`Self::from_shorthand`] (a first-party
    /// shorthand resolved against an on-disk framework checkout).
    fn from_resolved_dir(source_dir: &Path, original: &str) -> Result<Self, Error> {
        ensure_adapter_dir(source_dir, original)?;
        let canonical = fs::canonicalize(source_dir).map_err(|err| Error::Diag {
            code: "adapter-canonicalize-failed",
            detail: format!(
                "failed to canonicalize local adapter `{original}` at {}: {err}",
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

    /// Resolve a first-party shorthand (`omnia`, `omnia@v1`) to a
    /// concrete adapter source. Prefers an on-disk framework checkout
    /// named by `framework_root` (the `$SPECIFY_FRAMEWORK_ROOT` value,
    /// for offline dev + acceptance) and otherwise falls back to the
    /// canonical published adapter on GitHub. `init` is target-only, so
    /// the shorthand resolves under `adapters/targets/<name>/`.
    ///
    /// `framework_root` is threaded explicitly so unit tests can drive
    /// resolution without mutating process-global environment.
    fn from_shorthand(
        name: &str, reference: &str, framework_root: Option<&Path>,
    ) -> Result<Self, Error> {
        if let Some(root) = framework_root {
            let candidate = root
                .join(crate::adapter::ADAPTERS_DIR)
                .join(crate::adapter::Axis::Target.dir_segment())
                .join(name);
            if candidate.join(crate::adapter::ADAPTER_FILENAME).is_file() {
                return Self::from_resolved_dir(&candidate, name);
            }
        }
        let url =
            format!("https://github.com/augentic/specify/adapters/targets/{name}@{reference}");
        Self::from_github(&url)
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

/// Read the optional `$SPECIFY_FRAMEWORK_ROOT` override pointing at a
/// local checkout of the framework repo (the same env the lint surface
/// honours). Used to resolve first-party adapter shorthand offline.
fn framework_root_env() -> Option<PathBuf> {
    std::env::var_os("SPECIFY_FRAMEWORK_ROOT").map(PathBuf::from)
}

/// Recognise a first-party adapter shorthand and split it into
/// `(name, ref)`, defaulting the ref to `v1`. Returns `None` for paths
/// (`./foo`, `/abs`, `file://…`) and URLs (anything carrying `:` or
/// `/`), so those keep flowing through [`AdapterUri::from_local`] /
/// [`AdapterUri::from_github`]. Accepts `^[a-z][a-z0-9-]*(@v\d+)?$`.
fn parse_first_party_shorthand(adapter: &str) -> Option<(&str, &str)> {
    if adapter.contains('/') || adapter.contains(':') {
        return None;
    }
    match adapter.split_once('@') {
        Some((name, reference)) if is_first_party_name(name) && is_version_ref(reference) => {
            Some((name, reference))
        }
        None if is_first_party_name(adapter) => Some((adapter, "v1")),
        Some(_) | None => None,
    }
}

/// `^[a-z][a-z0-9-]*$` — a kebab-case first-party adapter name.
fn is_first_party_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `^v\d+$` — a major-version ref discriminator (`v1`, `v2`, …).
fn is_version_ref(reference: &str) -> bool {
    reference
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
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

/// Extract the kebab-case adapter name from a `project.yaml.adapter`
/// value. Accepts:
///
/// - bare kebab names (`omnia`) — returned unchanged,
/// - `file://` URIs — last path component,
/// - `https://...` URIs — last path component (suffix `@ref` stripped),
/// - bare local paths — last path component.
#[must_use]
pub fn adapter_name_from_value(value: &str) -> &str {
    let stripped = strip_at_ref_suffix(value);
    let stripped = stripped.strip_prefix("file://").unwrap_or(stripped);
    let stripped = stripped.strip_suffix('/').unwrap_or(stripped);
    stripped.rsplit('/').next().unwrap_or(stripped)
}

fn strip_at_ref_suffix(value: &str) -> &str {
    let last_slash = value.rfind('/').unwrap_or(0);
    if let Some(at) = value.rfind('@')
        && at > last_slash
    {
        return &value[..at];
    }
    value
}

#[cfg(test)]
mod tests;
