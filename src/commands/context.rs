//! `specify context {generate, check}` command surface.
//!
//! The parent owns the verb-level dispatcher and the shared infrastructure
//! that both subcommands lean on: input fingerprint assembly, the document
//! render wrapper, and a couple of tiny IO helpers. Sub-handlers under
//! `context/` carry the per-verb policy:
//! [`generate`] writes AGENTS.md plus `.specify/context.lock`,
//! [`check`] compares the live render against the lock and reports drift.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

mod check;
pub mod cli;
mod detect;
mod fences;
mod fingerprint;
mod generate;
mod lock;
mod render;

pub(super) use generate::for_init as generate_for_init;
use specify_capability::{Capability, PipelineView};
use specify_config::ProjectConfig;
use specify_error::{Error, Result};
use specify_registry::Registry;
use specify_slice::SliceMetadata;

use crate::cli::ContextAction;
use crate::context::Ctx;
use crate::output::CliResult;

pub fn run(ctx: &Ctx, action: ContextAction) -> Result<CliResult> {
    match action {
        ContextAction::Generate { check, force } => generate::run(ctx, check, force),
        ContextAction::Check => check::run(ctx),
    }
}

fn render_document(ctx: &Ctx) -> Result<(String, fingerprint::ContextFingerprint)> {
    let assembly = assemble_render_input(ctx)?;
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

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Io(err)),
    }
}

fn error_from_fence(err: fences::FenceError) -> Error {
    match err {
        fences::FenceError::ExistingUnfencedAgentsMd => Error::ContextUnfenced,
        other => Error::Diag {
            code: "context-fence-error",
            detail: other.to_string(),
        },
    }
}

fn context_lock_path(ctx: &Ctx) -> std::path::PathBuf {
    ProjectConfig::specify_dir(&ctx.project_dir).join("context.lock")
}

struct RenderAssembly {
    input: render::ContextRenderInput,
    inputs: Vec<fingerprint::InputFingerprint>,
}

fn assemble_render_input(ctx: &Ctx) -> Result<RenderAssembly> {
    let mut collector = fingerprint::InputCollector::new(&ctx.project_dir);
    collector.add_file(&ProjectConfig::config_path(&ctx.project_dir))?;

    let registry = Registry::load(&ctx.project_dir)?;
    collector.add_file_if_present(&ProjectConfig::registry_path(&ctx.project_dir))?;

    let capability = if ctx.config.hub {
        None
    } else {
        let pipeline = ctx.load_pipeline()?;
        collect_capability_inputs(&mut collector, &pipeline)?;
        Some(capability_summary(&pipeline))
    };
    let detection = if ctx.config.hub {
        detect::Detection::default()
    } else {
        detect::detect_root_markers(&ctx.project_dir)
    };
    emit_detection_warnings(&detection.warnings);
    collector.add_relative_files(detection.input_paths.iter().map(String::as_str))?;

    // The renderer's `plan.yaml` guidance is unconditional navigation text. It
    // does not inspect `plan.yaml` existence or content, so that file is not
    // fingerprinted unless a future renderer contract actually reads it.
    let active_slices =
        active_slice_names(&ProjectConfig::slices_dir(&ctx.project_dir), &mut collector)?;

    let input = render::ContextRenderInput {
        project_name: ctx.config.name.clone(),
        is_hub: ctx.config.hub,
        detection,
        domain: ctx.config.domain.clone(),
        capability,
        rule_overrides: rule_overrides(&ctx.config),
        declared_tools: declared_tools(&ctx.config),
        active_slices,
        workspace_peers: materialized_workspace_peers(registry.as_ref(), &ctx.project_dir)?,
        dependencies: dependency_peers(registry.as_ref()),
    };
    Ok(RenderAssembly {
        input,
        inputs: collector.finalize()?,
    })
}

fn collect_capability_inputs(
    collector: &mut fingerprint::InputCollector, pipeline: &PipelineView,
) -> Result<()> {
    if let Some(path) = Capability::probe_dir(&pipeline.capability.root_dir) {
        collector.add_file(&path)?;
    }
    for (_phase, brief) in &pipeline.briefs {
        collector.add_file(&brief.path)?;
    }
    Ok(())
}

fn capability_summary(pipeline: &PipelineView) -> render::CapabilitySummary {
    let mut briefs: Vec<render::BriefSummary> = pipeline
        .briefs
        .iter()
        .map(|(phase, brief)| render::BriefSummary {
            phase: phase.to_string(),
            id: brief.frontmatter.id.clone(),
            description: brief.frontmatter.description.clone(),
        })
        .collect();
    briefs.sort_by(|left, right| {
        (&left.phase, &left.id, &left.description).cmp(&(
            &right.phase,
            &right.id,
            &right.description,
        ))
    });

    render::CapabilitySummary {
        name: pipeline.capability.manifest.name.clone(),
        version: pipeline.capability.manifest.version,
        description: pipeline.capability.manifest.description.clone(),
        briefs,
    }
}

fn rule_overrides(config: &ProjectConfig) -> Vec<render::RuleOverride> {
    let mut overrides: Vec<render::RuleOverride> = config
        .rules
        .iter()
        .filter(|(_brief_id, path)| !path.is_empty())
        .map(|(brief_id, path)| render::RuleOverride {
            brief_id: brief_id.clone(),
            path: format!(".specify/{path}"),
        })
        .collect();
    overrides
        .sort_by(|left, right| (&left.brief_id, &left.path).cmp(&(&right.brief_id, &right.path)));
    overrides
}

fn declared_tools(config: &ProjectConfig) -> Vec<render::DeclaredTool> {
    let mut tools: Vec<render::DeclaredTool> = config
        .tools
        .iter()
        .map(|tool| render::DeclaredTool {
            name: tool.name.clone(),
            version: tool.version.clone(),
        })
        .collect();
    tools.sort_by(|left, right| (&left.name, &left.version).cmp(&(&right.name, &right.version)));
    tools
}

fn dependency_peers(registry: Option<&Registry>) -> Vec<render::DependencyPeer> {
    let Some(registry) = registry else {
        return Vec::new();
    };
    if registry.projects.len() <= 1 {
        return Vec::new();
    }

    let mut peers: Vec<render::DependencyPeer> = registry
        .projects
        .iter()
        .map(|project| render::DependencyPeer {
            name: project.name.clone(),
            capability: project.capability.clone(),
            url: project.url.clone(),
            description: project.description.clone(),
        })
        .collect();
    peers.sort_by(|left, right| {
        (&left.name, &left.capability, &left.url).cmp(&(&right.name, &right.capability, &right.url))
    });
    peers
}

fn materialized_workspace_peers(
    registry: Option<&Registry>, project_dir: &Path,
) -> Result<Vec<render::WorkspacePeer>> {
    let Some(registry) = registry else {
        return Ok(Vec::new());
    };
    if registry.projects.len() <= 1 {
        return Ok(Vec::new());
    }

    let workspace_dir = ProjectConfig::specify_dir(project_dir).join("workspace");
    let mut peers = Vec::new();
    for project in &registry.projects {
        let path = workspace_dir.join(&project.name);
        match fs::symlink_metadata(path) {
            Ok(_) => peers.push(render::WorkspacePeer {
                name: project.name.clone(),
                path: format!(".specify/workspace/{}/", project.name),
            }),
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(Error::Io(err)),
        }
    }
    peers.sort_by(|left, right| (&left.path, &left.name).cmp(&(&right.path, &right.name)));
    Ok(peers)
}

fn active_slice_names(
    slices_dir: &Path, collector: &mut fingerprint::InputCollector,
) -> Result<Vec<String>> {
    if !slices_dir.exists() {
        return Ok(Vec::new());
    }

    let mut names = Vec::new();
    for entry in std::fs::read_dir(slices_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let candidate = entry.path();
        let metadata_path = SliceMetadata::path(&candidate);
        if !metadata_path.is_file() {
            continue;
        }
        collector.add_file(&metadata_path)?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn emit_detection_warnings(warnings: &[detect::DetectionWarning]) {
    for warning in warnings {
        eprintln!("warning: {}: {}", warning.path, warning.message);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;
    use crate::cli::OutputFormat;

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
        let slices_dir = ProjectConfig::slices_dir(tmp.path());
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
        fs::create_dir_all(ProjectConfig::config_path(tmp.path()).parent().expect("config parent"))
            .expect("create .specify");
        fs::write(
            ProjectConfig::config_path(tmp.path()),
            "name: demo\ncapability: mini\nrules:\n  proposal: rules/proposal.md\n",
        )
        .expect("write project config");
        let ctx = Ctx {
            format: OutputFormat::Text,
            project_dir: tmp.path().to_path_buf(),
            config: sample_config(),
        };

        let assembly = assemble_render_input(&ctx).expect("assemble render input");
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
        fs::create_dir_all(ProjectConfig::config_path(tmp.path()).parent().expect("config parent"))
            .expect("create .specify");
        fs::write(ProjectConfig::config_path(tmp.path()), "name: platform\nhub: true\n")
            .expect("write project config");
        let ctx = Ctx {
            format: OutputFormat::Text,
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

        let assembly = assemble_render_input(&ctx).expect("hub assembly");
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
