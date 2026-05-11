//! Project-aware codex rule resolution.
//!
//! This module composes parsed [`CodexRule`] files from the deterministic
//! sources that define a project's active codex. It owns source ordering,
//! provenance, and resolved-set validation; individual file parsing stays
//! in [`crate::capability::codex`].

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use specify_error::{Error, ValidationStatus, ValidationSummary};

use crate::capability::capability::Capability;
use crate::capability::codex::CodexRule;

/// Foundational capability name resolved before the project capability.
pub const DEFAULT_CODEX_CAPABILITY: &str = "default";

/// Conventional directory containing codex rule markdown files.
pub const CODEX_DIR_NAME: &str = "codex";

const DUPLICATE_RULE_ID: &str = "codex.rule-id-unique";

/// Fully resolved active codex for a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedCodex {
    /// Rules in deterministic source order.
    pub rules: Vec<ResolvedCodexRule>,
}

/// A parsed codex rule plus the source that contributed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedCodexRule {
    /// Parsed rule file.
    pub rule: CodexRule,
    /// Source provenance for the rule.
    pub provenance: CodexProvenance,
}

/// Provenance attached to every resolved codex rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CodexProvenance {
    /// Rule came from a capability's `codex/` tree.
    Capability {
        /// Capability manifest name.
        name: String,
        /// Capability manifest version.
        version: u32,
    },
    /// Rule came from a future shared catalog codex source.
    Catalog {
        /// Catalog source name.
        name: String,
    },
    /// Rule came from the repository root `codex/` overlay.
    Repo,
}

/// Future shared catalog source hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCatalogSource {
    /// Catalog source name used in provenance.
    pub name: String,
    /// Filesystem root whose `codex/` tree should be loaded.
    pub root_dir: PathBuf,
}

/// Resolver input for project-aware codex resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexResolver {
    project_dir: PathBuf,
    project_capability: Option<String>,
    hub: bool,
    catalogs: Vec<CodexCatalogSource>,
}

impl ResolvedCodex {
    /// Resolve the active codex for `project_dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if a capability cannot be resolved, a rule file
    /// is invalid, or duplicate rule ids are found in the resolved set.
    pub fn resolve(
        project_dir: &Path, project_capability: Option<&str>, hub: bool,
    ) -> Result<Self, Error> {
        CodexResolver::new(
            project_dir.to_path_buf(),
            project_capability.map(ToOwned::to_owned),
            hub,
        )
        .resolve()
    }
}

impl CodexResolver {
    /// Create a resolver for a project.
    #[must_use]
    pub const fn new(project_dir: PathBuf, project_capability: Option<String>, hub: bool) -> Self {
        Self {
            project_dir,
            project_capability,
            hub,
            catalogs: Vec::new(),
        }
    }

    /// Add shared catalog sources.
    ///
    /// V1 callers leave this empty; the method keeps the source-order
    /// boundary explicit for the future shared-catalog configuration.
    #[must_use]
    pub fn with_catalogs(mut self, catalogs: Vec<CodexCatalogSource>) -> Self {
        self.catalogs = catalogs;
        self
    }

    /// Resolve the active codex.
    ///
    /// Source order is always:
    ///
    /// 1. foundational `default` capability,
    /// 2. project capability, unless this is a hub or it resolves to the
    ///    same root as `default`,
    /// 3. shared catalog hook sources,
    /// 4. repo-root `codex/` overlay.
    ///
    /// # Errors
    ///
    /// Returns an error if resolution, parsing, or duplicate-id validation
    /// fails.
    pub fn resolve(&self) -> Result<ResolvedCodex, Error> {
        let default = resolve_default(&self.project_dir)?;
        let default_root = default.root_dir.clone();
        let mut rules = load_capability_rules(&default)?;

        if let Some(capability) = self.project_capability.as_deref() {
            let project = Capability::resolve(capability, &self.project_dir)?;
            if project.root_dir != default_root {
                rules.extend(load_capability_rules(&project)?);
            }
        } else if !self.hub {
            return Err(Error::Diag {
                code: "codex-project-capability-missing",
                detail: "non-hub projects must declare a capability".to_string(),
            });
        }

        for catalog in &self.catalogs {
            let provenance = CodexProvenance::Catalog {
                name: catalog.name.clone(),
            };
            rules.extend(load_rules(&catalog.root_dir, &provenance)?);
        }

        rules.extend(load_rules(&self.project_dir, &CodexProvenance::Repo)?);
        reject_duplicate_ids(&rules)?;
        Ok(ResolvedCodex { rules })
    }
}

impl fmt::Display for CodexProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capability { name, version } => write!(f, "capability {name}@v{version}"),
            Self::Catalog { name } => write!(f, "catalog {name}"),
            Self::Repo => f.write_str("repo overlay"),
        }
    }
}

fn resolve_default(
    project_dir: &Path,
) -> Result<crate::capability::capability::ResolvedCapability, Error> {
    match Capability::resolve(DEFAULT_CODEX_CAPABILITY, project_dir) {
        Ok(capability) => Ok(capability),
        Err(err @ Error::Diag { .. }) => {
            let detail = err.to_string();
            Err(Error::Diag {
                code: "codex-default-capability-unavailable",
                detail: format!(
                    "foundational `{DEFAULT_CODEX_CAPABILITY}` capability could not be resolved: \
                     {detail}"
                ),
            })
        }
        Err(err) => Err(err),
    }
}

fn load_capability_rules(
    capability: &crate::capability::capability::ResolvedCapability,
) -> Result<Vec<ResolvedCodexRule>, Error> {
    let provenance = CodexProvenance::Capability {
        name: capability.manifest.name.clone(),
        version: capability.manifest.version,
    };
    load_rules(&capability.root_dir, &provenance)
}

fn load_rules(
    root_dir: &Path, provenance: &CodexProvenance,
) -> Result<Vec<ResolvedCodexRule>, Error> {
    let codex_dir = root_dir.join(CODEX_DIR_NAME);
    if !codex_dir.is_dir() {
        return Ok(Vec::new());
    }

    markdown_files(&codex_dir)?
        .into_iter()
        .map(|path| {
            let rule = CodexRule::load(&path)?;
            Ok(ResolvedCodexRule {
                rule,
                provenance: provenance.clone(),
            })
        })
        .collect()
}

fn markdown_files(codex_dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    collect_markdown_files(codex_dir, &mut files)?;
    files.sort_by_key(|path| lexical_key(codex_dir, path));
    Ok(files)
}

fn collect_markdown_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Error> {
    let entries = std::fs::read_dir(dir).map_err(|err| Error::Diag {
        code: "codex-source-read-failed",
        detail: format!("failed to read codex directory {}: {err}", dir.display()),
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| Error::Diag {
            code: "codex-source-read-failed",
            detail: format!("failed to read an entry under {}: {err}", dir.display()),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| Error::Diag {
            code: "codex-source-read-failed",
            detail: format!("failed to inspect {}: {err}", path.display()),
        })?;
        if file_type.is_dir() {
            collect_markdown_files(&path, files)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == OsStr::new("md"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn lexical_key(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn reject_duplicate_ids(rules: &[ResolvedCodexRule]) -> Result<(), Error> {
    let mut seen: BTreeMap<&str, &ResolvedCodexRule> = BTreeMap::new();
    let mut failures = Vec::new();

    for resolved in rules {
        let id = resolved.rule.normalized_id.as_str();
        if let Some(first) = seen.get(id) {
            failures.push(ValidationSummary {
                status: ValidationStatus::Fail,
                rule_id: DUPLICATE_RULE_ID.to_string(),
                rule: "codex rule ids are unique across resolved sources".to_string(),
                detail: Some(format!(
                    "codex-rule-id-duplicate: `{id}` appears in {} ({}) and {} ({})",
                    first.rule.path.display(),
                    first.provenance,
                    resolved.rule.path.display(),
                    resolved.provenance
                )),
            });
        } else {
            seen.insert(id, resolved);
        }
    }

    if failures.is_empty() { Ok(()) } else { Err(Error::Validation { results: failures }) }
}
