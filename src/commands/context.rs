//! `specify context {generate, check}` dispatcher.
//!
//! Heavy lifting lives in submodules: [`generate`] / [`check`] own the
//! per-verb policy, [`assemble`] walks the project to build the render
//! input and fingerprint set, and [`render`] / [`fences`] /
//! [`fingerprint`] / [`lock`] / [`detect`] own the renderer contract.
//! The parent keeps a small set of helpers — `render_document` plus the
//! IO/error-mapping shims — that both [`generate`] and [`check`] reach
//! for through `use super::*`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

mod assemble;
mod check;
pub(crate) mod cli;
mod detect;
mod fences;
mod fingerprint;
mod generate;
mod lock;
mod render;

pub(super) use generate::for_init as generate_for_init;
use specify_domain::config::LayoutExt;
use specify_error::{Error, Result};

use crate::cli::ContextAction;
use crate::context::Ctx;

pub(crate) fn run(ctx: &Ctx, action: &ContextAction) -> Result<()> {
    match action {
        ContextAction::Generate { check, force } => generate::run(ctx, *check, *force),
        ContextAction::Check => check::run(ctx),
    }
}

fn render_document(ctx: &Ctx) -> Result<(String, fingerprint::ContextFingerprint)> {
    let assembly = assemble::assemble_render_input(ctx)?;
    let aggregate = fingerprint::aggregate(env!("CARGO_PKG_VERSION"), assembly.inputs.clone());
    let generated = render::render_document_with_fingerprint(&assembly.input, &aggregate);
    let fenced = fences::parse_document(generated.as_bytes())
        .map_err(|err| Error::Diag {
            code: "context-generated-document-fence-error",
            detail: err.to_string(),
        })?
        .ok_or_else(|| Error::Diag {
            code: "context-generated-document-missing-fences",
            detail: "generated AGENTS.md content must contain a Specify context fence".to_string(),
        })?;
    let context_fingerprint =
        fingerprint::for_context(env!("CARGO_PKG_VERSION"), assembly.inputs, fenced.body());
    Ok((generated, context_fingerprint))
}

fn diag(code: &'static str, detail: impl Into<String>) -> Error {
    Error::Diag {
        code,
        detail: detail.into(),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Io(err)),
    }
}

fn error_from_fence(err: fences::FenceError) -> Error {
    match err {
        fences::FenceError::ExistingUnfencedAgentsMd => Error::Diag {
            code: "context-existing-unfenced-agents-md",
            detail: "AGENTS.md exists without Specify fences".to_string(),
        },
        other => Error::Diag {
            code: "context-fence-error",
            detail: other.to_string(),
        },
    }
}

fn context_lock_path(ctx: &Ctx) -> PathBuf {
    ctx.project_dir.layout().specify_dir().join("context.lock")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use specify_domain::config::ProjectConfig;
    use specify_domain::registry::Registry;
    use tempfile::tempdir;

    use super::*;
    use crate::cli::Format;

    fn write_minimal_capability(project_dir: &Path) {
        let capability_dir = project_dir.join("schemas").join("mini");
        let briefs_dir = capability_dir.join("briefs");
        fs::create_dir_all(&briefs_dir).expect("create capability dirs");
        fs::write(
            capability_dir.join("capability.yaml"),
            "name: mini\nversion: 1\ndescription: Mini capability\npipeline:\n  define:\n    - id: proposal\n      brief: briefs/proposal.md\n  build: []\n  merge: []\n",
        )
        .expect("write capability");
        fs::write(
            briefs_dir.join("proposal.md"),
            "---\nid: proposal\ndescription: Draft the proposal\ngenerates: proposal.md\n---\n",
        )
        .expect("write brief");
    }

    fn sample_config() -> ProjectConfig {
        let mut rules = BTreeMap::new();
        rules.insert("proposal".to_string(), "rules/proposal.md".to_string());
        ProjectConfig {
            name: "demo".to_string(),
            domain: Some("demo domain".to_string()),
            capability: Some("mini".to_string()),
            specify_version: None,
            rules,
            tools: Vec::new(),
            hub: false,
        }
    }

    #[test]
    fn assemble_render_input_reads_existing_metadata_in_sorted_order() {
        let tmp = tempdir().expect("tempdir");
        write_minimal_capability(tmp.path());
        let slices_dir = tmp.path().layout().slices_dir();
        fs::create_dir_all(slices_dir.join("zeta")).expect("create zeta");
        fs::create_dir_all(slices_dir.join("alpha")).expect("create alpha");
        fs::write(slices_dir.join("zeta").join(".metadata.yaml"), "not parsed").expect("zeta meta");
        fs::write(slices_dir.join("alpha").join(".metadata.yaml"), "also not parsed")
            .expect("alpha meta");
        fs::write(
            Registry::path(tmp.path()),
            "version: 1\nprojects:\n  - name: zeta\n    url: ../zeta\n    capability: mini@v1\n    description: Zeta service\n  - name: alpha\n    url: ../alpha\n    capability: mini@v1\n    description: Alpha service\n",
        )
        .expect("write registry");
        let cfg_path = tmp.path().layout().config_path();
        fs::create_dir_all(cfg_path.parent().expect("config parent")).expect("create .specify");
        fs::write(
            &cfg_path,
            "name: demo\ncapability: mini\nrules:\n  proposal: rules/proposal.md\n",
        )
        .expect("write project config");
        let ctx = Ctx {
            format: Format::Text,
            project_dir: tmp.path().to_path_buf(),
            config: sample_config(),
        };

        let assembly = assemble::assemble_render_input(&ctx).expect("assemble render input");
        let input = &assembly.input;

        assert_eq!(input.active_slices, vec!["alpha".to_string(), "zeta".to_string()]);
        assert_eq!(
            input.dependencies.iter().map(|peer| peer.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(
            input.capability.as_ref().map(|capability| capability.name.as_str()),
            Some("mini")
        );
        assert_eq!(
            input.rule_overrides,
            vec![render::RuleOverride {
                brief_id: "proposal".to_string(),
                path: ".specify/rules/proposal.md".to_string(),
            }]
        );
        assert_eq!(
            assembly.inputs.iter().map(|input| input.path.as_str()).collect::<Vec<_>>(),
            vec![
                ".specify/project.yaml",
                ".specify/slices/alpha/.metadata.yaml",
                ".specify/slices/zeta/.metadata.yaml",
                "registry.yaml",
                "schemas/mini/briefs/proposal.md",
                "schemas/mini/capability.yaml",
            ]
        );
    }

    #[test]
    fn assemble_render_input_skips_pipeline_for_hubs() {
        let tmp = tempdir().expect("tempdir");
        let cfg_path = tmp.path().layout().config_path();
        fs::create_dir_all(cfg_path.parent().expect("config parent")).expect("create .specify");
        fs::write(&cfg_path, "name: platform\nhub: true\n").expect("write project config");
        let ctx = Ctx {
            format: Format::Text,
            project_dir: tmp.path().to_path_buf(),
            config: ProjectConfig {
                name: "platform".to_string(),
                domain: None,
                capability: None,
                specify_version: None,
                rules: BTreeMap::new(),
                tools: Vec::new(),
                hub: true,
            },
        };

        let assembly = assemble::assemble_render_input(&ctx).expect("hub assembly");
        let input = &assembly.input;

        assert!(input.is_hub);
        assert!(input.capability.is_none());
        assert!(input.dependencies.is_empty());
        assert_eq!(
            assembly.inputs.iter().map(|input| input.path.as_str()).collect::<Vec<_>>(),
            vec![".specify/project.yaml"]
        );
    }
}
