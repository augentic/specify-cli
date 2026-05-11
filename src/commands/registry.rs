//! `specify registry *` dispatcher.
//!
//! Per-subcommand handlers live in `registry/{show, validate, add, remove}.rs`;
//! the shared response DTOs live in `registry/dto.rs`.

mod add;
pub(crate) mod cli;
mod dto;
mod remove;
mod show;
mod validate;

use specify_error::Result;

use crate::cli::RegistryAction;
use crate::context::Ctx;

pub(crate) fn run(ctx: &Ctx, action: RegistryAction) -> Result<()> {
    match action {
        RegistryAction::Show => show::run(ctx),
        RegistryAction::Validate => validate::run(ctx),
        RegistryAction::Add {
            name,
            url,
            capability,
            description,
        } => add::run(ctx, name, url, capability, description),
        RegistryAction::Remove { name } => remove::run(ctx, name),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use specify_change::{Entry, Plan, Status};
    use specify_config::{LayoutExt, ProjectConfig};
    use specify_registry::Registry;
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
        add::run(
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
        let err = add::run(&ctx, "alpha".to_string(), ".".to_string(), "   ".to_string(), None)
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

        assert_ok(remove::run(&ctx, "beta".to_string()), "remove beta");
        assert_eq!(names(&loaded(&tmp)), vec!["alpha"]);
    }

    #[test]
    fn remove_unknown_project_errors() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", None);

        let err = remove::run(&ctx, "nope".to_string()).expect_err("unknown name rejected");
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn remove_when_absent_errors() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        let err = remove::run(&ctx, "alpha".to_string()).expect_err("absent registry rejected");
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

        let warnings = remove::plan_refs(tmp.path(), "alpha");
        assert_eq!(warnings.len(), 1);
        let warning = &warnings[0];
        assert!(warning.contains("alpha-feature"), "warning lists alpha-feature: {warning}");
        assert!(warning.contains("another-alpha"), "warning lists another-alpha: {warning}");
        assert!(!warning.contains("beta-feature"), "warning excludes beta-feature: {warning}");
        assert!(warning.contains("plan amend"), "warning hints at remediation: {warning}");

        assert_ok(remove::run(&ctx, "alpha".to_string()), "remove alpha");
        assert_eq!(names(&loaded(&tmp)), vec!["beta"]);
    }

    #[test]
    fn remove_emits_no_warning_when_plan_absent() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp, false);
        ok_seed(&ctx, "alpha", ".", None);

        assert!(remove::plan_refs(tmp.path(), "alpha").is_empty());
        assert_ok(remove::run(&ctx, "alpha".to_string()), "remove alpha");
    }
}
