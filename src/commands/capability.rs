//! `specify capability {resolve, pipeline}`.

pub mod cli;

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify_domain::capability::{Capability, CapabilitySource, Phase};
use specify_error::{Error, Result};

use crate::cli::Format;
use crate::context::Ctx;
use crate::output;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ResolveBody {
    capability_value: String,
    resolved_path: String,
    source: &'static str,
}

fn write_resolve_text(w: &mut dyn Write, body: &ResolveBody) -> std::io::Result<()> {
    writeln!(w, "{}", body.resolved_path)
}

pub fn resolve(format: Format, capability_value: String, project_dir: &Path) -> Result<()> {
    let (root_dir, source) = Capability::locate(&capability_value, project_dir)?;
    Capability::probe_dir(&root_dir).ok_or_else(|| Error::Diag {
        code: "capability-manifest-missing",
        detail: format!("no `capability.yaml` at {}", root_dir.display()),
    })?;
    let (source_label, path) = match &source {
        CapabilitySource::Local(p) => ("local", p.clone()),
        CapabilitySource::Cached(p) => ("cached", p.clone()),
        _ => ("unknown", PathBuf::new()),
    };

    output::write(
        format,
        &ResolveBody {
            capability_value,
            resolved_path: path.display().to_string(),
            source: source_label,
        },
        write_resolve_text,
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct BriefRow {
    id: String,
    description: String,
    path: String,
    needs: Vec<String>,
    generates: Option<String>,
    tracks: Option<String>,
    present: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PipelineBody {
    phase: String,
    slice: Option<String>,
    briefs: Vec<BriefRow>,
}

fn write_pipeline_text(w: &mut dyn Write, body: &PipelineBody) -> std::io::Result<()> {
    writeln!(w, "phase: {}", body.phase)?;
    for b in &body.briefs {
        let present_label = match &b.present {
            Value::Bool(true) => " [x]",
            Value::Bool(false) => " [ ]",
            _ => "",
        };
        writeln!(w, "  {}{present_label}", b.id)?;
        if let Some(g) = &b.generates {
            writeln!(w, "    generates: {g}")?;
        }
        if !b.needs.is_empty() {
            writeln!(w, "    needs: {}", b.needs.join(", "))?;
        }
        if let Some(t) = &b.tracks {
            writeln!(w, "    tracks: {t}")?;
        }
    }
    Ok(())
}

pub fn pipeline(ctx: &Ctx, phase: Phase, slice: Option<&Path>) -> Result<()> {
    let pipeline = ctx.load_pipeline()?;
    let order = pipeline.topo_order(phase)?;
    let completion = slice.map(|slice_dir| pipeline.completion_for(phase, slice_dir));

    let briefs = order
        .iter()
        .map(|b| {
            let present = completion.as_ref().and_then(|c| c.get(&b.frontmatter.id));
            BriefRow {
                id: b.frontmatter.id.clone(),
                description: b.frontmatter.description.clone(),
                path: b.path.display().to_string(),
                needs: b.frontmatter.needs.clone(),
                generates: b.frontmatter.generates.clone(),
                tracks: b.frontmatter.tracks.clone(),
                present: present.copied().map_or(Value::Null, Value::from),
            }
        })
        .collect();

    ctx.write(
        &PipelineBody {
            phase: phase.to_string(),
            slice: slice.map(|p| p.display().to_string()),
            briefs,
        },
        write_pipeline_text,
    )?;
    Ok(())
}
