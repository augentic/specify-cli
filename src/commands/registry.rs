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

    use specify_domain::config::{LayoutExt, ProjectConfig};
    use specify_domain::registry::Registry;
    use tempfile::TempDir;

    use super::*;
    use crate::cli::Format;

    fn ctx_for(tmp: &TempDir) -> Ctx {
        let specify_dir = tmp.path().join(".specify");
        fs::create_dir_all(&specify_dir).expect("create .specify");
        let cfg = ProjectConfig {
            name: "demo".to_string(),
            domain: None,
            capability: Some("omnia".to_string()),
            specify_version: None,
            rules: BTreeMap::new(),
            tools: Vec::new(),
            hub: false,
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

    /// Argv-parser / handler regression: a non-kebab project name must
    /// be rejected before any registry write happens. The full add /
    /// remove flow (success + every other rejection branch) is covered
    /// end-to-end by `tests/cli.rs::registry_*` against the built
    /// binary; this is the only unit test retained here so the
    /// kebab-case guardrail stays close to `add::run` for fast feedback
    /// when someone edits the handler.
    #[test]
    fn add_rejects_non_kebab_name() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp);
        let err = add::run(
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
}
