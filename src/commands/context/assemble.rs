//! Render-input assembly for `specify context *`.
//!
//! Walks the project (capability, registry, slices, root markers) and
//! emits a [`render::ContextRenderInput`] plus the per-input
//! fingerprint set both handlers feed into [`super::fingerprint`] and
//! [`super::render`]. The actual Markdown emission lives in
//! [`super::render`]; this module is purely about input collection.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use specify_capability::{Capability, PipelineView};
use specify_config::{LayoutExt, ProjectConfig};
use specify_error::{Error, Result};
use specify_registry::Registry;
use specify_slice::SliceMetadata;

use super::{detect, fingerprint, render};
use crate::context::Ctx;

pub(super) struct RenderAssembly {
    pub(super) input: render::ContextRenderInput,
    pub(super) inputs: Vec<fingerprint::InputFingerprint>,
}

pub(super) fn assemble_render_input(ctx: &Ctx) -> Result<RenderAssembly> {
    let layout = ctx.project_dir.layout();
    let mut collector = fingerprint::InputCollector::new(&ctx.project_dir);
    collector.add_file(&layout.config_path())?;

    let registry = Registry::load(&ctx.project_dir)?;
    collector.add_file_if_present(&layout.registry_path())?;

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
    let active_slices = active_slice_names(&layout.slices_dir(), &mut collector)?;

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

    let workspace_dir = project_dir.layout().specify_dir().join("workspace");
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
