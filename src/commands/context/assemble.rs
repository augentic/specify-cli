//! Render-input assembly for `specify context *`. Walks the project
//! (adapter, registry, slices, root markers) and emits a
//! [`render::Input`] plus the per-input fingerprint set.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use specify_domain::adapter::{ADAPTER_FILENAME, ResolvedAdapter};
use specify_domain::config::{Layout, ProjectConfig};
use specify_domain::registry::Registry;
use specify_domain::slice::SliceMetadata;
use specify_error::{Error, Result};

use super::{detect, fingerprint, render};
use crate::context::Ctx;

pub(super) struct RenderAssembly {
    pub(super) input: render::Input,
    pub(super) inputs: Vec<fingerprint::InputFingerprint>,
}

pub(super) fn render_input(ctx: &Ctx) -> Result<RenderAssembly> {
    let layout = ctx.layout();
    let mut collector = fingerprint::InputCollector::new(&ctx.project_dir);
    collector.add_file(&layout.config_path())?;

    let registry = Registry::load(&ctx.project_dir)?;
    collector.add_file_if_present(&layout.registry_path())?;

    let adapter = if ctx.config.hub {
        None
    } else {
        let resolved = ctx.resolve_target_adapter()?;
        collect_adapter_inputs(&mut collector, &resolved)?;
        Some(adapter_summary(&resolved))
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

    let input = render::Input {
        project_name: ctx.config.name.clone(),
        is_hub: ctx.config.hub,
        detection,
        domain: ctx.config.domain.clone(),
        adapter,
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

fn collect_adapter_inputs(
    collector: &mut fingerprint::InputCollector, adapter: &ResolvedAdapter,
) -> Result<()> {
    let manifest = adapter.root_dir.join(ADAPTER_FILENAME);
    if manifest.is_file() {
        collector.add_file(&manifest)?;
    }
    for relative in adapter.manifest.briefs.values() {
        let brief_path = adapter.root_dir.join(relative);
        if brief_path.is_file() {
            collector.add_file(&brief_path)?;
        }
    }
    Ok(())
}

fn adapter_summary(adapter: &ResolvedAdapter) -> render::Adapter {
    let mut briefs: Vec<render::Brief> = adapter
        .manifest
        .briefs
        .keys()
        .map(|operation| render::Brief {
            phase: operation.clone(),
            id: operation.clone(),
            description: String::new(),
        })
        .collect();
    briefs.sort_by(|left, right| {
        (&left.phase, &left.id, &left.description).cmp(&(
            &right.phase,
            &right.id,
            &right.description,
        ))
    });

    render::Adapter {
        name: adapter.manifest.name.clone(),
        version: adapter.manifest.version,
        description: adapter.manifest.description.clone().unwrap_or_default(),
        briefs,
    }
}

fn rule_overrides(config: &ProjectConfig) -> Vec<render::Rule> {
    let mut overrides: Vec<render::Rule> = config
        .rules
        .iter()
        .filter(|(_brief_id, path)| !path.is_empty())
        .map(|(brief_id, path)| render::Rule {
            brief_id: brief_id.clone(),
            path: format!(".specify/{path}"),
        })
        .collect();
    overrides
        .sort_by(|left, right| (&left.brief_id, &left.path).cmp(&(&right.brief_id, &right.path)));
    overrides
}

fn declared_tools(config: &ProjectConfig) -> Vec<render::Tool> {
    let mut tools: Vec<render::Tool> = config
        .tools
        .iter()
        .map(|tool| render::Tool {
            name: tool.name.clone(),
            version: tool.version.clone(),
        })
        .collect();
    tools.sort_by(|left, right| (&left.name, &left.version).cmp(&(&right.name, &right.version)));
    tools
}

fn dependency_peers(registry: Option<&Registry>) -> Vec<render::Dep> {
    let Some(registry) = registry else {
        return Vec::new();
    };
    if registry.projects.len() <= 1 {
        return Vec::new();
    }

    let mut peers: Vec<render::Dep> = registry
        .projects
        .iter()
        .map(|project| render::Dep {
            name: project.name.clone(),
            adapter: project.adapter.clone(),
            url: project.url.clone(),
            description: project.description.clone(),
        })
        .collect();
    peers.sort_by(|left, right| {
        (&left.name, &left.adapter, &left.url).cmp(&(&right.name, &right.adapter, &right.url))
    });
    peers
}

fn materialized_workspace_peers(
    registry: Option<&Registry>, project_dir: &Path,
) -> Result<Vec<render::Peer>> {
    let Some(registry) = registry else {
        return Ok(Vec::new());
    };
    if registry.projects.len() <= 1 {
        return Ok(Vec::new());
    }

    let workspace_dir = Layout::new(project_dir).specify_dir().join("workspace");
    let mut peers = Vec::new();
    for project in &registry.projects {
        let path = workspace_dir.join(&project.name);
        match fs::symlink_metadata(path) {
            Ok(_) => peers.push(render::Peer {
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
    for entry in fs::read_dir(slices_dir)? {
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
