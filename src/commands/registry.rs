#![allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::option_if_let_else,
    clippy::unnecessary_wraps,
    reason = "Command handlers mirror Clap-owned inputs and keep JSON DTOs close to their emission sites."
)]

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, ProjectConfig, is_kebab};
use specify_change::Plan;
use specify_registry::{Registry, RegistryProject};

use crate::cli::{OutputFormat, RegistryAction};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_registry(ctx: &CommandContext, action: RegistryAction) -> Result<CliResult, Error> {
    match action {
        RegistryAction::Show => show_registry(ctx),
        RegistryAction::Validate => validate_registry(ctx),
        RegistryAction::Add {
            name,
            url,
            schema,
            description,
        } => add_to_registry(ctx, name, url, schema, description),
        RegistryAction::Remove { name } => remove_from_registry(ctx, name),
    }
}

fn show_registry(ctx: &CommandContext) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    match Registry::load(&ctx.project_dir)? {
        None => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct RegistryBody {
                        registry: Value,
                        path: String,
                    }
                    emit_response(RegistryBody {
                        registry: Value::Null,
                        path: registry_path.display().to_string(),
                    })?;
                }
                OutputFormat::Text => {
                    println!("no registry declared at registry.yaml");
                }
            }
            Ok(CliResult::Success)
        }
        Some(registry) => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct RegistryFullBody {
                        registry: Registry,
                        path: String,
                    }
                    emit_response(RegistryFullBody {
                        registry,
                        path: registry_path.display().to_string(),
                    })?;
                }
                OutputFormat::Text => {
                    print_registry_text(&registry, &registry_path);
                }
            }
            Ok(CliResult::Success)
        }
    }
}

fn validate_registry(ctx: &CommandContext) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    // Hub repos opt into the stricter shape via `project.yaml:hub:
    // true`. We tolerate a missing/unparseable project.yaml here —
    // `specify registry validate` is allowed to run before `specify
    // init` in tooling chains, in which case there is no hub flag to
    // honour and the base shape check is the right behaviour.
    let hub_mode = ProjectConfig::load(&ctx.project_dir).is_ok_and(|cfg| cfg.hub);
    let load_result = Registry::load(&ctx.project_dir);
    let load_result = match load_result {
        Ok(Some(registry)) if hub_mode => match registry.validate_shape_hub() {
            Ok(()) => Ok(Some(registry)),
            Err(err) => Err(err),
        },
        other => other,
    };
    match load_result {
        Ok(None) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ValidateEmpty {
                registry: Value,
                path: String,
                ok: bool,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(ValidateEmpty {
                    registry: Value::Null,
                    path: registry_path.display().to_string(),
                    ok: true,
                })?,
                OutputFormat::Text => {
                    println!("no registry declared at registry.yaml");
                }
            }
            Ok(CliResult::Success)
        }
        Ok(Some(registry)) => {
            let count = registry.projects.len();
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ValidateBody {
                registry: Registry,
                path: String,
                ok: bool,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(ValidateBody {
                    registry,
                    path: registry_path.display().to_string(),
                    ok: true,
                })?,
                OutputFormat::Text => {
                    if hub_mode {
                        println!("registry.yaml is well-formed in hub mode ({count} project(s))");
                    } else {
                        println!("registry.yaml is well-formed ({count} project(s))");
                    }
                }
            }
            Ok(CliResult::Success)
        }
        Err(err) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct RegistryValidateErrorResponse {
                path: String,
                ok: bool,
                error: String,
                kind: &'static str,
                exit_code: u8,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(RegistryValidateErrorResponse {
                    path: registry_path.display().to_string(),
                    ok: false,
                    error: err.to_string(),
                    kind: "config",
                    exit_code: CliResult::ValidationFailed.code(),
                })?,
                OutputFormat::Text => eprintln!("error: {err}"),
            }
            Ok(CliResult::ValidationFailed)
        }
    }
}

/// `specify registry add` — RFC-9 §2A. Append a new project entry to
/// `registry.yaml` (at the repo root), creating the file with
/// `version: 1` when absent. Pre-validates the obvious shape errors
/// (kebab-case name, non-empty schema, duplicate name) for friendlier
/// diagnostics, then runs the same `Registry::validate_shape` (or
/// `validate_shape_hub` for platform hubs) the read-only verbs
/// already use, so URL classification and the
/// `description-missing-multi-repo` invariant produce the canonical
/// error messages.
fn add_to_registry(
    ctx: &CommandContext, name: String, url: String, schema: String, description: Option<String>,
) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    let hub_mode = ctx.config.hub;

    if !is_kebab(&name) {
        return Err(Error::Config(format!(
            "registry add: project name `{name}` must be kebab-case \
             (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
        )));
    }
    if schema.trim().is_empty() {
        return Err(Error::Config(
            "registry add: --schema must be non-empty (e.g. `omnia@v1`)".to_string(),
        ));
    }

    // Load the existing registry (if any) without applying hub-mode
    // rejection: we only run `validate_shape_hub` once the candidate
    // entry is appended, so the post-write diagnostic is the one the
    // operator sees.
    let mut registry = match Registry::load(&ctx.project_dir) {
        Ok(Some(reg)) => reg,
        Ok(None) => Registry {
            version: 1,
            projects: Vec::new(),
        },
        Err(err) => return Err(err),
    };

    if registry.projects.iter().any(|p| p.name == name) {
        return Err(Error::Config(format!(
            "registry add: project `{name}` already exists in {}",
            registry_path.display()
        )));
    }

    let new_entry = RegistryProject {
        name: name.clone(),
        url,
        schema,
        description: description.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        }),
        contracts: None,
    };

    registry.projects.push(new_entry);

    // Surface `validate_shape` / `validate_shape_hub` errors verbatim —
    // their diagnostic codes (`description-missing-multi-repo`,
    // `hub-cannot-be-project`, etc.) are the documented contract.
    if hub_mode {
        registry.validate_shape_hub()?;
    } else {
        registry.validate_shape()?;
    }

    write_registry(&registry, &registry_path)?;

    let count = registry.projects.len();
    let added = registry
        .projects
        .last()
        .expect("we just pushed an entry; non-empty by construction")
        .clone();

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct AddBody {
                registry: Registry,
                path: String,
                added: RegistryProject,
                ok: bool,
            }
            emit_response(AddBody {
                registry,
                path: registry_path.display().to_string(),
                added,
                ok: true,
            })?;
        }
        OutputFormat::Text => {
            println!("Added `{name}` to {}", registry_path.display());
            println!("registry now declares {count} project(s)");
        }
    }
    Ok(CliResult::Success)
}

/// `specify registry remove` — RFC-9 §2A. Delete the named project
/// from `registry.yaml` (at the repo root). Validates the resulting
/// shape (a removal can only relax the multi-repo description
/// invariant, so the post-write check should always succeed; we run
/// it anyway to pin the contract). Emits a non-fatal warning when
/// `plan.yaml` references the removed project, naming the affected
/// plan entries so the operator can run `specify change plan amend
/// --project <other>` against each one.
fn remove_from_registry(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    let hub_mode = ctx.config.hub;

    let Some(mut registry) = Registry::load(&ctx.project_dir)? else {
        return Err(Error::Config(format!(
            "registry remove: no registry declared at {}",
            registry_path.display()
        )));
    };

    let position = registry.projects.iter().position(|p| p.name == name).ok_or_else(|| {
        Error::Config(format!(
            "registry remove: project `{name}` not found in {}",
            registry_path.display()
        ))
    })?;
    registry.projects.remove(position);

    if hub_mode {
        registry.validate_shape_hub()?;
    } else {
        registry.validate_shape()?;
    }

    write_registry(&registry, &registry_path)?;

    let warnings = plan_references_for(&ctx.project_dir, &name);

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct RemoveBody {
                registry: Registry,
                path: String,
                removed: String,
                warnings: Vec<String>,
                ok: bool,
            }
            emit_response(RemoveBody {
                registry,
                path: registry_path.display().to_string(),
                removed: name,
                warnings,
                ok: true,
            })?;
        }
        OutputFormat::Text => {
            println!("Removed `{name}` from {}", registry_path.display());
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
        }
    }
    Ok(CliResult::Success)
}

/// Persist `registry` to `path` using `serde_saphyr`. Mirrors the
/// `fs::write` posture used by `init --hub` (RFC-9 §1D) — the schema
/// crate does not expose its own `Registry::save` and the writes are
/// short enough that a temp-and-rename dance would buy little. Callers
/// must run `validate_shape` / `validate_shape_hub` *before* calling
/// this helper so the on-disk file is always shape-valid.
fn write_registry(registry: &Registry, path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let yaml = serde_saphyr::to_string(registry)?;
    fs::write(path, yaml)?;
    Ok(())
}

/// Scan `plan.yaml` (when present) for plan entries whose
/// `project` field equals `removed_name`. Returns one human-readable
/// warning string per affected entry. Best-effort: any parse error is
/// surfaced as a single advisory string instead of failing the
/// remove (the registry write has already landed, so the operator
/// needs to learn about both halves).
fn plan_references_for(project_dir: &Path, removed_name: &str) -> Vec<String> {
    let plan_path = ProjectConfig::plan_path(project_dir);
    if !plan_path.exists() {
        return Vec::new();
    }
    match Plan::load(&plan_path) {
        Ok(plan) => {
            let referencing: Vec<&str> = plan
                .changes
                .iter()
                .filter(|entry| entry.project.as_deref() == Some(removed_name))
                .map(|entry| entry.name.as_str())
                .collect();
            if referencing.is_empty() {
                Vec::new()
            } else {
                vec![format!(
                    "plan.yaml has {n} entry(ies) still referencing project `{removed_name}`: {entries}. \
                     Run `specify change plan amend <change> --project <other>` to rewire them.",
                    n = referencing.len(),
                    entries = referencing.join(", "),
                )]
            }
        }
        Err(err) => vec![format!(
            "plan.yaml present but unreadable; cannot check for stale references to `{removed_name}`: {err}"
        )],
    }
}

fn print_registry_text(registry: &Registry, registry_path: &Path) {
    println!("registry.yaml: {}", registry_path.display());
    println!("version: {}", registry.version);
    if registry.projects.is_empty() {
        println!("projects: (none)");
        return;
    }
    println!("projects:");
    for project in &registry.projects {
        println!("  - name: {}", project.name);
        println!("    url: {}", project.url);
        println!("    schema: {}", project.schema);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use specify_change::{Entry, Status};
    use tempfile::TempDir;

    use super::*;
    use crate::cli::OutputFormat;

    /// Assert the handler returned `CliResult::Success`. Wrapping the
    /// `must_use` value here keeps each test site ergonomic without
    /// silently discarding non-success exit codes.
    #[track_caller]
    fn assert_ok(result: Result<CliResult, Error>, what: &str) {
        let value = result.unwrap_or_else(|err| panic!("{what} failed: {err}"));
        assert_eq!(value, CliResult::Success, "{what} should yield Success, got {value:?}");
    }

    fn ctx_for(tmp: &TempDir, hub: bool) -> CommandContext {
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
        let cfg_path = ProjectConfig::config_path(tmp.path());
        let serialised = serde_saphyr::to_string(&cfg).expect("serialise project.yaml");
        fs::write(&cfg_path, serialised).expect("write project.yaml");

        CommandContext {
            format: OutputFormat::Json,
            project_dir: tmp.path().to_path_buf(),
            config: cfg,
        }
    }

    fn read_registry(tmp: &TempDir) -> Registry {
        Registry::load(tmp.path()).expect("load").expect("present")
    }

    // ---------- kebab-case ----------

    #[test]
    fn add_rejects_non_kebab_name() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = add_to_registry(
            &ctx,
            "BadName".to_string(),
            "git@github.com:org/bad-name.git".to_string(),
            "omnia@v1".to_string(),
            None,
        )
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
        let err = add_to_registry(
            &ctx,
            "snake_name".to_string(),
            ".".to_string(),
            "omnia@v1".to_string(),
            None,
        )
        .expect_err("snake_case rejected");
        assert!(err.to_string().contains("kebab-case"));
    }

    #[test]
    fn add_rejects_empty_schema() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err =
            add_to_registry(&ctx, "alpha".to_string(), ".".to_string(), "   ".to_string(), None)
                .expect_err("empty schema rejected");
        assert!(err.to_string().contains("--schema"));
    }

    // ---------- URL classification (delegated to validate_shape) ----------

    #[test]
    fn add_rejects_unsupported_url_scheme() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = add_to_registry(
            &ctx,
            "alpha".to_string(),
            "ftp://example.com/repo".to_string(),
            "omnia@v1".to_string(),
            None,
        )
        .expect_err("ftp scheme rejected");
        let msg = err.to_string();
        assert!(msg.contains("ftp"), "msg: {msg}");
        assert!(msg.contains("scheme"), "msg: {msg}");
    }

    #[test]
    fn add_rejects_absolute_path_url() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = add_to_registry(
            &ctx,
            "alpha".to_string(),
            "/absolute/path".to_string(),
            "omnia@v1".to_string(),
            None,
        )
        .expect_err("absolute path rejected");
        assert!(err.to_string().contains("relative"));
    }

    // ---------- create-on-first-add + round-trip ----------

    #[test]
    fn add_creates_registry_when_absent_and_round_trips() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert!(!Registry::path(tmp.path()).exists());

        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                None,
            ),
            "add to fresh project",
        );

        let registry = read_registry(&tmp);
        assert_eq!(registry.version, 1);
        assert_eq!(registry.projects.len(), 1);
        assert_eq!(registry.projects[0].name, "alpha");
        assert_eq!(registry.projects[0].url, ".");
        assert_eq!(registry.projects[0].schema, "omnia@v1");
        assert!(registry.projects[0].description.is_none());
    }

    #[test]
    fn add_appends_to_existing_registry() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                None,
            ),
            "seed alpha",
        );

        // Adding a second entry now requires a description on both
        // entries (description-missing-multi-repo). Pre-edit: stomp
        // the seed file to give it a description, then add.
        let mut seeded = read_registry(&tmp);
        seeded.projects[0].description = Some("Alpha service".to_string());
        write_registry(&seeded, &Registry::path(tmp.path())).unwrap();

        assert_ok(
            add_to_registry(
                &ctx,
                "beta".to_string(),
                "../beta".to_string(),
                "omnia@v1".to_string(),
                Some("Beta service".to_string()),
            ),
            "append beta",
        );

        let registry = read_registry(&tmp);
        let names: Vec<&str> = registry.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn add_rejects_duplicate_name() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                None,
            ),
            "seed alpha",
        );

        let err = add_to_registry(
            &ctx,
            "alpha".to_string(),
            "../other".to_string(),
            "omnia@v1".to_string(),
            None,
        )
        .expect_err("duplicate name rejected");
        let msg = err.to_string();
        assert!(msg.contains("already exists"), "msg: {msg}");
        assert!(msg.contains("alpha"), "msg: {msg}");
    }

    // ---------- description-missing-multi-repo invariant ----------

    #[test]
    fn add_enforces_description_missing_multi_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                None,
            ),
            "seed alpha (no description)",
        );

        // Adding beta promotes the registry to multi-project; the
        // missing description on alpha must trigger the canonical
        // diagnostic.
        let err = add_to_registry(
            &ctx,
            "beta".to_string(),
            "../beta".to_string(),
            "omnia@v1".to_string(),
            Some("Beta service".to_string()),
        )
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
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                Some("Alpha service".to_string()),
            ),
            "seed alpha with description",
        );
        assert_ok(
            add_to_registry(
                &ctx,
                "beta".to_string(),
                "../beta".to_string(),
                "omnia@v1".to_string(),
                Some("Beta service".to_string()),
            ),
            "append beta with description",
        );

        let registry = read_registry(&tmp);
        assert_eq!(registry.projects.len(), 2);
        assert_eq!(registry.projects[0].description.as_deref(), Some("Alpha service"));
        assert_eq!(registry.projects[1].description.as_deref(), Some("Beta service"));
    }

    // ---------- hub mode ----------

    #[test]
    fn add_hub_rejects_dot_url_with_hub_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, true);
        let err = add_to_registry(
            &ctx,
            "platform".to_string(),
            ".".to_string(),
            "omnia@v1".to_string(),
            None,
        )
        .expect_err("hub mode rejects url: .");
        let msg = err.to_string();
        assert!(msg.contains("hub-cannot-be-project"), "msg: {msg}");
        assert!(msg.contains("platform"), "msg: {msg}");
    }

    #[test]
    fn add_hub_accepts_remote_url() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, true);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                "git@github.com:org/alpha.git".to_string(),
                "omnia@v1".to_string(),
                None,
            ),
            "hub registry accepts remote-url entries",
        );
    }

    // ---------- whitespace-only description normalisation ----------

    #[test]
    fn add_treats_whitespace_only_description_as_none() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                Some("   ".to_string()),
            ),
            "single-project add tolerates whitespace description",
        );
        let registry = read_registry(&tmp);
        assert!(registry.projects[0].description.is_none());
    }

    // ---------- remove ----------

    #[test]
    fn remove_succeeds_and_round_trips() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                Some("Alpha service".to_string()),
            ),
            "seed alpha",
        );
        assert_ok(
            add_to_registry(
                &ctx,
                "beta".to_string(),
                "../beta".to_string(),
                "omnia@v1".to_string(),
                Some("Beta service".to_string()),
            ),
            "seed beta",
        );

        assert_ok(remove_from_registry(&ctx, "beta".to_string()), "remove beta");
        let registry = read_registry(&tmp);
        let names: Vec<&str> = registry.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha"]);
    }

    #[test]
    fn remove_unknown_project_errors() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                None,
            ),
            "seed alpha",
        );

        let err =
            remove_from_registry(&ctx, "nope".to_string()).expect_err("unknown name rejected");
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn remove_when_registry_absent_errors() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err =
            remove_from_registry(&ctx, "alpha".to_string()).expect_err("absent registry rejected");
        assert!(err.to_string().contains("no registry declared"));
    }

    #[test]
    fn remove_warns_when_plan_references_project() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                Some("Alpha service".to_string()),
            ),
            "seed alpha",
        );
        assert_ok(
            add_to_registry(
                &ctx,
                "beta".to_string(),
                "../beta".to_string(),
                "omnia@v1".to_string(),
                Some("Beta service".to_string()),
            ),
            "seed beta",
        );

        // Author a plan with two entries pointing at alpha.
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![
                Entry {
                    name: "alpha-feature".to_string(),
                    project: Some("alpha".to_string()),
                    schema: None,
                    status: Status::Pending,
                    depends_on: vec![],
                    sources: vec![],
                    context: vec![],
                    description: None,
                    status_reason: None,
                },
                Entry {
                    name: "beta-feature".to_string(),
                    project: Some("beta".to_string()),
                    schema: None,
                    status: Status::Pending,
                    depends_on: vec![],
                    sources: vec![],
                    context: vec![],
                    description: None,
                    status_reason: None,
                },
                Entry {
                    name: "another-alpha".to_string(),
                    project: Some("alpha".to_string()),
                    schema: None,
                    status: Status::Pending,
                    depends_on: vec![],
                    sources: vec![],
                    context: vec![],
                    description: None,
                    status_reason: None,
                },
            ],
        };
        plan.save(&ProjectConfig::plan_path(tmp.path())).expect("save plan");

        // Removing alpha keeps the registry shape valid (beta still
        // has its description) and surfaces a warning naming both
        // entries.
        let warnings = plan_references_for(tmp.path(), "alpha");
        assert_eq!(warnings.len(), 1);
        let warning = &warnings[0];
        assert!(warning.contains("alpha-feature"), "warning lists alpha-feature: {warning}");
        assert!(warning.contains("another-alpha"), "warning lists another-alpha: {warning}");
        assert!(!warning.contains("beta-feature"), "warning excludes beta-feature: {warning}");
        assert!(warning.contains("plan amend"), "warning hints at remediation: {warning}");

        // The remove call itself still succeeds (warning is non-fatal)
        // and the registry no longer lists alpha.
        assert_ok(
            remove_from_registry(&ctx, "alpha".to_string()),
            "remove alpha despite plan reference",
        );
        let registry = read_registry(&tmp);
        let names: Vec<&str> = registry.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["beta"]);
    }

    #[test]
    fn remove_emits_no_warning_when_plan_absent() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        assert_ok(
            add_to_registry(
                &ctx,
                "alpha".to_string(),
                ".".to_string(),
                "omnia@v1".to_string(),
                None,
            ),
            "seed alpha",
        );

        assert!(plan_references_for(tmp.path(), "alpha").is_empty());
        assert_ok(remove_from_registry(&ctx, "alpha".to_string()), "remove alpha");
    }

}
