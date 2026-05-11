//! `specify registry *` handlers — `show`, `validate`, `add`, `remove`.

pub(crate) mod cli;

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_change::Plan;
use specify_config::{LayoutExt, ProjectConfig, with_existing_state, with_state};
use specify_error::{Error, Result, is_kebab};
use specify_registry::{Registry, RegistryProject};

use crate::cli::RegistryAction;
use crate::context::Ctx;
use crate::output::{Render, display, serialize_path};

pub(crate) fn run(ctx: &Ctx, action: RegistryAction) -> Result<()> {
    match action {
        RegistryAction::Show => show(ctx),
        RegistryAction::Validate => validate(ctx),
        RegistryAction::Add {
            name,
            url,
            capability,
            description,
        } => add(ctx, name, url, capability, description),
        RegistryAction::Remove { name } => remove(ctx, name),
    }
}

fn show(ctx: &Ctx) -> Result<()> {
    let path = Registry::path(&ctx.project_dir);
    let registry = Registry::load(&ctx.project_dir)?;
    ctx.out().write(&ShowBody { registry, path })?;
    Ok(())
}

fn validate(ctx: &Ctx) -> Result<()> {
    let path = Registry::path(&ctx.project_dir);
    // Hub repos opt into the stricter shape via `project.yaml:hub:
    // true`. Tolerate a missing/unparseable project.yaml here —
    // `specify registry validate` is allowed to run before `specify
    // init`, in which case there is no hub flag to honour and the base
    // shape check is the right behaviour.
    let hub_mode = ProjectConfig::load(&ctx.project_dir).is_ok_and(|cfg| cfg.hub);
    let registry = Registry::load(&ctx.project_dir)?;
    if hub_mode && let Some(reg) = registry.as_ref() {
        reg.validate_shape_hub()?;
    }
    ctx.out().write(&ValidateBody {
        registry,
        path,
        hub_mode,
    })?;
    Ok(())
}

fn add(
    ctx: &Ctx, name: String, url: String, capability: String, description: Option<String>,
) -> Result<()> {
    if !is_kebab(&name) {
        return Err(Error::Diag {
            code: "registry-add-name-not-kebab",
            detail: format!(
                "registry add: project name `{name}` must be kebab-case \
                 (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
            ),
        });
    }
    if capability.trim().is_empty() {
        return Err(Error::Diag {
            code: "registry-add-capability-empty",
            detail: "registry add: --capability must be non-empty (e.g. `omnia@v1`)".into(),
        });
    }

    let path = Registry::path(&ctx.project_dir);
    let hub_mode = ctx.config.hub;
    let candidate = RegistryProject {
        name,
        url,
        capability,
        description: description.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        }),
        contracts: None,
    };

    let body = with_state::<Registry, _, _>(ctx.layout(), move |registry| {
        if registry.projects.iter().any(|p| p.name == candidate.name) {
            return Err(Error::Diag {
                code: "registry-add-name-duplicate",
                detail: format!(
                    "registry add: project `{}` already exists in {}",
                    candidate.name,
                    path.display()
                ),
            });
        }

        registry.projects.push(candidate);

        // Surface validate_shape / validate_shape_hub errors verbatim —
        // their diagnostic codes (`description-missing-multi-repo`,
        // `hub-cannot-be-project`, etc.) are the documented contract.
        // Returning Err here aborts `with_state` before the atomic
        // write, so the on-disk registry is never left in a
        // shape-invalid state.
        if hub_mode {
            registry.validate_shape_hub()?;
        } else {
            registry.validate_shape()?;
        }

        let added = registry
            .projects
            .last()
            .expect("we just pushed an entry; non-empty by construction")
            .clone();
        Ok(AddBody {
            registry: registry.clone(),
            path,
            added,
        })
    })?;

    ctx.out().write(&body)?;
    Ok(())
}

fn remove(ctx: &Ctx, name: String) -> Result<()> {
    let path = Registry::path(&ctx.project_dir);
    let hub_mode = ctx.config.hub;

    // Pre-flight: surface the legacy `registry-remove-no-registry`
    // diagnostic when the file is absent. `with_existing_state` would
    // emit the generic `Error::ArtifactNotFound`; the registry-specific
    // diag is part of the wire contract.
    if !path.exists() {
        return Err(Error::Diag {
            code: "registry-remove-no-registry",
            detail: format!("registry remove: no registry declared at {}", path.display()),
        });
    }

    let body =
        with_existing_state::<Registry, _, _>(ctx.layout(), "registry.yaml", move |registry| {
            let position =
                registry.projects.iter().position(|p| p.name == name).ok_or_else(|| {
                    Error::Diag {
                        code: "registry-remove-not-found",
                        detail: format!(
                            "registry remove: project `{name}` not found in {}",
                            path.display()
                        ),
                    }
                })?;
            registry.projects.remove(position);

            // A removal can only relax the multi-repo description
            // invariant, so the post-write check should always
            // succeed; we run it anyway to pin the contract.
            if hub_mode {
                registry.validate_shape_hub()?;
            } else {
                registry.validate_shape()?;
            }

            let warnings = plan_refs(&ctx.project_dir, &name);
            Ok(RemoveBody {
                registry: registry.clone(),
                path,
                removed: name,
                warnings,
            })
        })?;

    ctx.out().write(&body)?;
    Ok(())
}

/// Scan `plan.yaml` (when present) for plan entries whose `project`
/// field equals `removed`. Returns one human-readable warning per
/// affected entry. Best-effort: any parse error is surfaced as a
/// single advisory string instead of failing the remove (the registry
/// write has already landed, so the operator needs to learn about
/// both halves).
fn plan_refs(project_dir: &Path, removed: &str) -> Vec<String> {
    let plan_path = project_dir.layout().plan_path();
    if !plan_path.exists() {
        return Vec::new();
    }
    match Plan::load(&plan_path) {
        Ok(plan) => {
            let referencing: Vec<&str> = plan
                .entries
                .iter()
                .filter(|entry| entry.project.as_deref() == Some(removed))
                .map(|entry| entry.name.as_str())
                .collect();
            if referencing.is_empty() {
                Vec::new()
            } else {
                vec![format!(
                    "plan.yaml has {n} entry(ies) still referencing project `{removed}`: {entries}. \
                     Run `specify change plan amend <change> --project <other>` to rewire them.",
                    n = referencing.len(),
                    entries = referencing.join(", "),
                )]
            }
        }
        Err(err) => vec![format!(
            "plan.yaml present but unreadable; cannot check for stale references to `{removed}`: {err}"
        )],
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody {
    registry: Option<Registry>,
    #[serde(serialize_with = "serialize_path")]
    path: PathBuf,
}

impl Render for ShowBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let Some(reg) = self.registry.as_ref() else {
            return writeln!(w, "no registry declared at registry.yaml");
        };
        writeln!(w, "registry.yaml: {}", display(&self.path))?;
        writeln!(w, "version: {}", reg.version)?;
        if reg.projects.is_empty() {
            return writeln!(w, "projects: (none)");
        }
        writeln!(w, "projects:")?;
        for project in &reg.projects {
            writeln!(w, "  - name: {}", project.name)?;
            writeln!(w, "    url: {}", project.url)?;
            writeln!(w, "    capability: {}", project.capability)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidateBody {
    registry: Option<Registry>,
    #[serde(serialize_with = "serialize_path")]
    path: PathBuf,
    #[serde(skip)]
    hub_mode: bool,
}

impl Render for ValidateBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let Some(reg) = self.registry.as_ref() else {
            return writeln!(w, "no registry declared at registry.yaml");
        };
        let count = reg.projects.len();
        if self.hub_mode {
            writeln!(w, "registry.yaml is well-formed in hub mode ({count} project(s))")
        } else {
            writeln!(w, "registry.yaml is well-formed ({count} project(s))")
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AddBody {
    registry: Registry,
    #[serde(serialize_with = "serialize_path")]
    path: PathBuf,
    added: RegistryProject,
}

impl Render for AddBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Added `{}` to {}", self.added.name, display(&self.path))?;
        writeln!(w, "registry now declares {} project(s)", self.registry.projects.len())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct RemoveBody {
    registry: Registry,
    #[serde(serialize_with = "serialize_path")]
    path: PathBuf,
    removed: String,
    warnings: Vec<String>,
}

impl Render for RemoveBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Removed `{}` from {}", self.removed, display(&self.path))?;
        for warning in &self.warnings {
            writeln!(w, "warning: {warning}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use specify_change::{Entry, Status};
    use tempfile::TempDir;

    use super::*;
    use crate::cli::Format;

    /// Panic with a descriptive message when a handler returned an
    /// error. Handlers in this module return `Result<()>` (the
    /// success path is unconditional), so the only thing left to
    /// assert at a test site is "no error".
    #[track_caller]
    fn assert_ok(result: Result<()>, what: &str) {
        result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
    }

    fn ctx_for(tmp: &TempDir, hub: bool) -> Ctx {
        let specify_dir = tmp.path().join(".specify");
        fs::create_dir_all(&specify_dir).expect("create .specify");
        let cfg = ProjectConfig {
            name: "demo".to_string(),
            domain: None,
            capability: if hub { None } else { Some("omnia".to_string()) },
            specify_version: None,
            rules: BTreeMap::new(),
            tools: Vec::new(),
            hub,
        };
        let cfg_path = tmp.path().layout().config_path();
        let serialised = serde_saphyr::to_string(&cfg).expect("serialise project.yaml");
        fs::write(&cfg_path, serialised).expect("write project.yaml");

        Ctx {
            format: Format::Json,
            project_dir: tmp.path().to_path_buf(),
            config: cfg,
        }
    }

    fn loaded(tmp: &TempDir) -> Registry {
        Registry::load(tmp.path()).expect("load").expect("present")
    }

    fn names(reg: &Registry) -> Vec<&str> {
        reg.projects.iter().map(|p| p.name.as_str()).collect()
    }

    /// Helper for tests: invoke `add` with a fixed `omnia@v1` capability.
    fn seed(ctx: &Ctx, name: &str, url: &str, description: Option<&str>) -> Result<()> {
        add(
            ctx,
            name.to_string(),
            url.to_string(),
            "omnia@v1".to_string(),
            description.map(str::to_string),
        )
    }

    fn ok_seed(ctx: &Ctx, name: &str, url: &str, description: Option<&str>) {
        assert_ok(seed(ctx, name, url, description), &format!("seed {name}"));
    }

    #[test]
    fn add_rejects_non_kebab_name() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = seed(&ctx, "BadName", "git@github.com:org/bad-name.git", None)
            .expect_err("non-kebab name must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("kebab-case"), "diagnostic must mention kebab-case: {msg}");
        assert!(msg.contains("BadName"), "diagnostic must echo the bad name: {msg}");
        assert!(!Registry::path(tmp.path()).exists(), "rejected add must not create registry.yaml");
    }

    #[test]
    fn add_rejects_underscore_name() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = seed(&ctx, "snake_name", ".", None).expect_err("snake_case rejected");
        assert!(err.to_string().contains("kebab-case"));
    }

    #[test]
    fn add_rejects_empty_capability() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = add(&ctx, "alpha".to_string(), ".".to_string(), "   ".to_string(), None)
            .expect_err("empty capability rejected");
        assert!(err.to_string().contains("--capability"));
    }

    #[test]
    fn add_rejects_unsupported_url_scheme() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err =
            seed(&ctx, "alpha", "ftp://example.com/repo", None).expect_err("ftp scheme rejected");
        let msg = err.to_string();
        assert!(msg.contains("ftp"), "msg: {msg}");
        assert!(msg.contains("scheme"), "msg: {msg}");
    }

    #[test]
    fn add_rejects_absolute_path_url() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = seed(&ctx, "alpha", "/absolute/path", None).expect_err("absolute path rejected");
        assert!(err.to_string().contains("relative"));
    }

    #[test]
    fn add_creates_registry_when_absent_and_round_trips() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert!(!Registry::path(tmp.path()).exists());
        ok_seed(&ctx, "alpha", ".", None);

        let registry = loaded(&tmp);
        assert_eq!(registry.version, 1);
        assert_eq!(registry.projects.len(), 1);
        assert_eq!(registry.projects[0].name, "alpha");
        assert_eq!(registry.projects[0].url, ".");
        assert_eq!(registry.projects[0].capability, "omnia@v1");
        assert!(registry.projects[0].description.is_none());
    }

    #[test]
    fn add_appends_to_existing() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", None);

        // Adding a second entry now requires a description on both
        // entries (description-missing-multi-repo). Pre-edit: stomp
        // the seed file to give it a description, then add.
        let mut seeded = loaded(&tmp);
        seeded.projects[0].description = Some("Alpha service".to_string());
        specify_slice::atomic::yaml_write(&Registry::path(tmp.path()), &seeded).unwrap();

        ok_seed(&ctx, "beta", "../beta", Some("Beta service"));
        assert_eq!(names(&loaded(&tmp)), vec!["alpha", "beta"]);
    }

    #[test]
    fn add_rejects_duplicate_name() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", None);
        let err = seed(&ctx, "alpha", "../other", None).expect_err("duplicate name rejected");
        let msg = err.to_string();
        assert!(msg.contains("already exists"), "msg: {msg}");
        assert!(msg.contains("alpha"), "msg: {msg}");
    }

    #[test]
    fn add_enforces_description_missing_multi_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", None);

        let err = seed(&ctx, "beta", "../beta", Some("Beta service"))
            .expect_err("description-missing-multi-repo rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("description-missing-multi-repo"),
            "must surface the stable diagnostic code: {msg}"
        );
        assert!(msg.contains("alpha"), "must name the offending entry: {msg}");
    }

    #[test]
    fn add_with_descriptions_passes_multi_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", Some("Alpha service"));
        ok_seed(&ctx, "beta", "../beta", Some("Beta service"));

        let registry = loaded(&tmp);
        assert_eq!(registry.projects.len(), 2);
        assert_eq!(registry.projects[0].description.as_deref(), Some("Alpha service"));
        assert_eq!(registry.projects[1].description.as_deref(), Some("Beta service"));
    }

    #[test]
    fn add_hub_rejects_dot_url_with_hub_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, true);
        let err = seed(&ctx, "platform", ".", None).expect_err("hub mode rejects url: .");
        let msg = err.to_string();
        assert!(msg.contains("hub-cannot-be-project"), "msg: {msg}");
        assert!(msg.contains("platform"), "msg: {msg}");
    }

    #[test]
    fn add_hub_accepts_remote_url() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, true);
        ok_seed(&ctx, "alpha", "git@github.com:org/alpha.git", None);
    }

    #[test]
    fn add_treats_whitespace_only_description_as_none() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", Some("   "));
        assert!(loaded(&tmp).projects[0].description.is_none());
    }

    #[test]
    fn remove_succeeds_and_round_trips() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", Some("Alpha service"));
        ok_seed(&ctx, "beta", "../beta", Some("Beta service"));

        assert_ok(remove(&ctx, "beta".to_string()), "remove beta");
        assert_eq!(names(&loaded(&tmp)), vec!["alpha"]);
    }

    #[test]
    fn remove_unknown_project_errors() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", None);

        let err = remove(&ctx, "nope".to_string()).expect_err("unknown name rejected");
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn remove_when_absent_errors() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = remove(&ctx, "alpha".to_string()).expect_err("absent registry rejected");
        assert!(err.to_string().contains("no registry declared"));
    }

    fn entry(name: &str, project: &str) -> Entry {
        Entry {
            name: name.to_string(),
            project: Some(project.to_string()),
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }
    }

    #[test]
    fn remove_warns_when_plan_references_project() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", Some("Alpha service"));
        ok_seed(&ctx, "beta", "../beta", Some("Beta service"));

        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            entries: vec![
                entry("alpha-feature", "alpha"),
                entry("beta-feature", "beta"),
                entry("another-alpha", "alpha"),
            ],
        };
        plan.save(&tmp.path().layout().plan_path()).expect("save plan");

        let warnings = plan_refs(tmp.path(), "alpha");
        assert_eq!(warnings.len(), 1);
        let warning = &warnings[0];
        assert!(warning.contains("alpha-feature"), "warning lists alpha-feature: {warning}");
        assert!(warning.contains("another-alpha"), "warning lists another-alpha: {warning}");
        assert!(!warning.contains("beta-feature"), "warning excludes beta-feature: {warning}");
        assert!(warning.contains("plan amend"), "warning hints at remediation: {warning}");

        assert_ok(remove(&ctx, "alpha".to_string()), "remove alpha");
        assert_eq!(names(&loaded(&tmp)), vec!["beta"]);
    }

    #[test]
    fn remove_emits_no_warning_when_plan_absent() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", None);

        assert!(plan_refs(tmp.path(), "alpha").is_empty());
        assert_ok(remove(&ctx, "alpha".to_string()), "remove alpha");
    }
}
