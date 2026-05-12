//! `specify capability {resolve, check, pipeline}`.

pub(crate) mod cli;

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify_domain::capability::{Capability, CapabilitySource, Phase};
use specify_domain::validate::ValidationResult;
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

pub(crate) fn resolve(format: Format, capability_value: String, project_dir: &Path) -> Result<()> {
    let (root_dir, source) = Capability::locate(&capability_value, project_dir)?;
    enforce_capability_filename(&root_dir)?;
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

pub(crate) fn pipeline(ctx: &Ctx, phase: Phase, slice: Option<&Path>) -> Result<()> {
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CheckBody {
    passed: bool,
    results: Vec<CheckRow>,
}

fn write_check_text(w: &mut dyn Write, body: &CheckBody) -> std::io::Result<()> {
    if body.passed {
        writeln!(w, "Capability OK")
    } else {
        let fail_count = body.results.iter().filter(|r| matches!(r, CheckRow::Fail { .. })).count();
        writeln!(w, "Capability invalid: {fail_count} errors")?;
        for r in &body.results {
            if let CheckRow::Fail { rule_id, detail, .. } = r {
                writeln!(w, "  [fail] {rule_id}: {detail}")?;
            }
        }
        Ok(())
    }
}

pub(crate) fn check(format: Format, capability_dir: &Path) -> Result<()> {
    let manifest_path =
        Capability::probe_dir(capability_dir).ok_or_else(|| Error::CapabilityManifestMissing {
            dir: capability_dir.to_path_buf(),
        })?;
    let capability = load_manifest(&manifest_path)?;
    let results = capability.validate_structure();
    let passed = !results.iter().any(|r| matches!(r, ValidationResult::Fail { .. }));

    let body = CheckBody {
        passed,
        results: results.iter().map(CheckRow::from).collect(),
    };
    output::write(format, &body, write_check_text)?;
    if passed {
        Ok(())
    } else {
        Err(Error::Diag {
            code: "capability-check-failed",
            detail: format!("capability at {} failed validation", capability_dir.display()),
        })
    }
}

/// Surface a `capability-manifest-missing` diagnostic when `dir` does
/// not carry a `capability.yaml`.
fn enforce_capability_filename(dir: &Path) -> Result<()> {
    Capability::probe_dir(dir).map(|_| ()).ok_or_else(|| Error::CapabilityManifestMissing {
        dir: dir.to_path_buf(),
    })
}

fn load_manifest(manifest_path: &Path) -> Result<Capability> {
    let text = std::fs::read_to_string(manifest_path)?;
    let capability: Capability = serde_saphyr::from_str(&text)?;
    Ok(capability)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "status")]
enum CheckRow {
    #[serde(rename = "pass")]
    Pass { rule_id: String, rule: String },
    #[serde(rename = "fail")]
    Fail { rule_id: String, rule: String, detail: String },
    #[serde(rename = "deferred")]
    Deferred { rule_id: String, rule: String, reason: String },
    #[serde(rename = "unknown")]
    Unknown,
}

impl From<&ValidationResult> for CheckRow {
    fn from(r: &ValidationResult) -> Self {
        match r {
            ValidationResult::Pass { rule_id, rule } => Self::Pass {
                rule_id: rule_id.to_string(),
                rule: rule.to_string(),
            },
            ValidationResult::Fail {
                rule_id,
                rule,
                detail,
            } => Self::Fail {
                rule_id: rule_id.to_string(),
                rule: rule.to_string(),
                detail: detail.clone(),
            },
            ValidationResult::Deferred {
                rule_id,
                rule,
                reason,
            } => Self::Deferred {
                rule_id: rule_id.to_string(),
                rule: rule.to_string(),
                reason: reason.to_string(),
            },
            _ => Self::Unknown,
        }
    }
}
