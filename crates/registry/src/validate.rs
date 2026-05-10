//! Registry-shape validators (RFC-3a §"The Registry", RFC-8 §Layer 2,
//! RFC-9 §1D).
//!
//! Enforces invariants that `serde` cannot express on its own:
//! `version == 1`, kebab-case project names, non-empty required strings,
//! unique project names, well-formed URLs, the multi-project description
//! requirement, and the contract-roles consistency rules. Hub-mode
//! validation (RFC-9 §1D) layers an additional `hub-cannot-be-project`
//! check on top of the base shape rules.
//!
//! The methods are exposed on [`Registry`] itself so callers — including
//! `Registry::load` — keep the same fluent shape they had pre-extraction.

use std::collections::{HashMap, HashSet};

use specify_error::{Error, is_kebab};

use crate::registry::Registry;

impl Registry {
    /// Enforce invariants that serde cannot express on its own:
    /// `version == 1`, kebab-case project names, non-empty required
    /// strings, unique project names, and well-formed `projects[].url`
    /// values (RFC-3a C28). Returns the first error encountered — the
    /// convention used elsewhere in the registry crate for fast-fail
    /// shape validation.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    #[allow(clippy::too_many_lines)]
    pub fn validate_shape(&self) -> Result<(), Error> {
        if self.version != 1 {
            return Err(Error::Diag {
                code: "registry-version-unsupported",
                detail: format!(
                    "registry.yaml: unsupported version {}; v1 is the only accepted value",
                    self.version
                ),
            });
        }

        let mut seen_names: HashSet<&str> = HashSet::new();
        for (idx, project) in self.projects.iter().enumerate() {
            if project.name.is_empty() {
                return Err(Error::Diag {
                    code: "registry-project-name-empty",
                    detail: format!("registry.yaml: projects[{idx}].name is empty"),
                });
            }
            if !is_kebab(&project.name) {
                return Err(Error::Diag {
                    code: "registry-project-name-not-kebab",
                    detail: format!(
                        "registry.yaml: projects[{idx}].name `{}` must be kebab-case \
                         (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)",
                        project.name
                    ),
                });
            }
            if project.url.is_empty() {
                return Err(Error::Diag {
                    code: "registry-project-url-empty",
                    detail: format!(
                        "registry.yaml: projects[{idx}] (`{}`).url is empty",
                        project.name
                    ),
                });
            }
            validate_project_url(&project.url, idx, &project.name)?;
            if project.capability.is_empty() {
                return Err(Error::Diag {
                    code: "registry-project-capability-empty",
                    detail: format!(
                        "registry.yaml: projects[{idx}] (`{}`).capability is empty",
                        project.name
                    ),
                });
            }
            if !seen_names.insert(project.name.as_str()) {
                return Err(Error::Diag {
                    code: "registry-project-name-duplicate",
                    detail: format!("registry.yaml: duplicate project name `{}`", project.name),
                });
            }
        }

        if self.projects.len() > 1 {
            for (idx, project) in self.projects.iter().enumerate() {
                let missing = project.description.as_ref().is_none_or(|s| s.trim().is_empty());
                if missing {
                    return Err(Error::Diag {
                        code: "registry-description-missing-multi-repo",
                        detail: format!(
                            "registry.yaml: projects[{idx}] (`{}`).description is required when the registry declares more than one project",
                            project.name
                        ),
                    });
                }
            }
        }

        // --- Contract role invariants (RFC-8 Layer 2; RFC-12 collapsed
        // the role set to `produces` + `consumes` and dropped the
        // produce/import mutual-exclusion check) ---

        // Invariant 3: Path validity — no absolute or `..` paths.
        for project in &self.projects {
            if let Some(ref roles) = project.contracts {
                for path in roles.produces.iter().chain(roles.consumes.iter()) {
                    if path.starts_with('/') || path.contains("..") {
                        return Err(Error::Diag {
                            code: "registry-contract-path-not-relative",
                            detail: format!(
                                "registry.yaml: contract path '{}' in project '{}' must be relative (no '..' or absolute paths)",
                                path, project.name
                            ),
                        });
                    }
                }
            }
        }

        // Invariant 4: Self-consistency — a project must not list the
        // same path in both `produces` and `consumes`.
        for project in &self.projects {
            if let Some(ref roles) = project.contracts {
                let produced: HashSet<&str> =
                    roles.produces.iter().map(std::string::String::as_str).collect();
                for path in &roles.consumes {
                    if produced.contains(path.as_str()) {
                        return Err(Error::Diag {
                            code: "registry-contract-produces-and-consumes",
                            detail: format!(
                                "registry.yaml: project '{}' lists '{}' in both 'produces' and 'consumes'",
                                project.name, path
                            ),
                        });
                    }
                }
            }
        }

        // Invariant 1: Single producer — each contract path appears in
        // `produces` for at most one project.
        let mut producers: HashMap<&str, &str> = HashMap::new();
        for project in &self.projects {
            if let Some(ref roles) = project.contracts {
                for path in &roles.produces {
                    if let Some(existing) = producers.get(path.as_str()) {
                        return Err(Error::Diag {
                            code: "registry-contract-multiple-producers",
                            detail: format!(
                                "registry.yaml: contract path '{}' is produced by both '{}' and '{}'",
                                path, existing, project.name
                            ),
                        });
                    }
                    producers.insert(path, &project.name);
                }
            }
        }

        Ok(())
    }

    /// Hub-only shape check (RFC-9 §1D).
    ///
    /// Runs the base [`Registry::validate_shape`] first, then layers on
    /// the additional invariant that a **registry-only platform hub**
    /// must never list itself as a project: any entry with `url: .` is
    /// rejected with a `hub-cannot-be-project` diagnostic. The hub
    /// holds platform-level state (registry, initiative brief, plan,
    /// workspace clones) but is never a code project.
    ///
    /// Callers opt in by checking `project.yaml:hub: true` and
    /// invoking this method in addition to (or instead of) the base
    /// [`Registry::validate_shape`]. Non-hub callers continue to use
    /// the base method unchanged — this is a strictly additive API.
    ///
    /// # Errors
    ///
    /// Returns the first base-shape error if `validate_shape` fails,
    /// or a `hub-cannot-be-project` config error if any entry's `url`
    /// equals `.`.
    pub fn validate_shape_hub(&self) -> Result<(), Error> {
        self.validate_shape()?;
        for (idx, project) in self.projects.iter().enumerate() {
            if project.url == "." {
                return Err(Error::Diag {
                    code: "hub-cannot-be-project",
                    detail: format!(
                        "registry.yaml: projects[{idx}] (`{}`).url is `.`; \
                         a registry-only platform hub must not appear in its own \
                         registry — code projects always live in their own repos",
                        project.name
                    ),
                });
            }
        }
        Ok(())
    }
}

/// RFC-3a C28 — reject malformed `projects[].url` while accepting:
/// `.`, repo-relative paths, `http(s)://`, `git@host:path`, `ssh://`,
/// and `git+http(s)://` / `git+ssh://` forms.
fn validate_project_url(url: &str, idx: usize, project_name: &str) -> Result<(), Error> {
    const ALLOWED_SCHEMES: &[&str] = &["http", "https", "ssh", "git+https", "git+http", "git+ssh"];

    if url.trim().is_empty() {
        return Err(Error::Diag {
            code: "registry-project-url-empty",
            detail: format!(
                "registry.yaml: projects[{idx}] (`{project_name}`).url is empty or whitespace-only"
            ),
        });
    }
    if url != url.trim() {
        return Err(Error::Diag {
            code: "registry-project-url-whitespace",
            detail: format!(
                "registry.yaml: projects[{idx}] (`{project_name}`).url must not have leading or trailing whitespace"
            ),
        });
    }

    if url == "." {
        return Ok(());
    }

    if url.starts_with("git@") {
        return Ok(());
    }

    if let Some(pos) = url.find("://") {
        let scheme = &url[..pos];
        if !ALLOWED_SCHEMES.contains(&scheme) {
            return Err(Error::Diag {
                code: "registry-project-url-scheme-unsupported",
                detail: format!(
                    "registry.yaml: projects[{idx}] (`{project_name}`).url has unsupported URL scheme `{scheme}`: \
                     expected one of http, https, ssh, git+https, git+http, git+ssh, a `git@host:path` remote, `.`, or a relative path"
                ),
            });
        }
        return Ok(());
    }

    if url.contains(':') {
        return Err(Error::Diag {
            code: "registry-project-url-malformed",
            detail: format!(
                "registry.yaml: projects[{idx}] (`{project_name}`).url `{url}` is not valid: \
                 ':' is only allowed in `git@host:path` remotes or in `scheme://` URLs"
            ),
        });
    }

    if url.starts_with('/') {
        return Err(Error::Diag {
            code: "registry-project-url-absolute",
            detail: format!(
                "registry.yaml: projects[{idx}] (`{project_name}`).url must be a relative path, `.`, or a remote URL — absolute filesystem paths are not allowed"
            ),
        });
    }

    #[cfg(windows)]
    if looks_like_windows_drive_path(url) {
        return Err(Error::Diag {
            code: "registry-project-url-absolute",
            detail: format!(
                "registry.yaml: projects[{idx}] (`{project_name}`).url must be a relative path, `.`, or a remote URL — absolute Windows paths are not allowed"
            ),
        });
    }

    Ok(())
}

#[cfg(windows)]
fn looks_like_windows_drive_path(url: &str) -> bool {
    let mut chars = url.chars();
    let Some(c) = chars.next() else {
        return false;
    };
    c.is_ascii_alphabetic() && chars.next() == Some(':')
}
