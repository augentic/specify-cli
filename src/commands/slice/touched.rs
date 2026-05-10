//! `slice touched-specs` and `slice overlap`.

use std::io::Write;

use serde::Serialize;
use specify_config::ProjectConfig;
use specify_error::Error;
use specify_merge::MergeStrategy;
use specify_slice::{SliceMetadata, SpecKind, TouchedSpec, actions as slice_actions};

use super::artifact_classes;
use crate::context::CommandContext;
use crate::output::{CliResult, Render, emit};

pub(super) fn touched_specs(
    ctx: &CommandContext, name: String, scan: bool, set: Vec<String>,
) -> Result<CliResult, Error> {
    let slice_dir = ctx.slices_dir().join(&name);

    let entries = if !set.is_empty() {
        let v = parse_touched_spec_set(&set)?;
        let metadata = slice_actions::write_touched(&slice_dir, v)?;
        metadata.touched_specs
    } else if scan {
        // Classifies a delta as `new` vs `modified` against the omnia
        // ThreeWayMerge baseline. Reach through the omnia synthesiser
        // so any future change to the baseline location flows through
        // one place.
        let classes = artifact_classes(&ctx.project_dir, &slice_dir);
        let baseline_dir = classes
            .iter()
            .find(|c| matches!(c.strategy, MergeStrategy::ThreeWayMerge))
            .map_or_else(
                || ProjectConfig::specify_dir(&ctx.project_dir).join("specs"),
                |c| c.baseline_dir.clone(),
            );
        let scanned = slice_actions::scan_touched(&slice_dir, &baseline_dir)?;
        let metadata = slice_actions::write_touched(&slice_dir, scanned)?;
        metadata.touched_specs
    } else {
        let metadata = SliceMetadata::load(&slice_dir)?;
        metadata.touched_specs
    };

    let touched: Vec<TouchedSpecRow> = entries.iter().map(TouchedSpecRow::from_spec).collect();
    emit(
        ctx.format,
        &TouchedSpecsBody {
            name,
            touched_specs: touched,
        },
    )?;
    Ok(CliResult::Success)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TouchedSpecsBody {
    name: String,
    touched_specs: Vec<TouchedSpecRow>,
}

impl Render for TouchedSpecsBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.touched_specs.is_empty() {
            return writeln!(w, "{}: no touched specs", self.name);
        }
        writeln!(w, "{}:", self.name)?;
        for entry in &self.touched_specs {
            writeln!(w, "  {} ({})", entry.name, entry.r#type)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TouchedSpecRow {
    name: String,
    r#type: String,
}

impl TouchedSpecRow {
    fn from_spec(t: &TouchedSpec) -> Self {
        Self {
            name: t.name.clone(),
            r#type: t.kind.to_string(),
        }
    }
}

fn parse_touched_spec_set(raw: &[String]) -> Result<Vec<TouchedSpec>, Error> {
    let mut out: Vec<TouchedSpec> = Vec::with_capacity(raw.len());
    for entry in raw {
        let (name, kind) = entry.split_once(':').ok_or_else(|| Error::Diag {
            code: "touched-specs-entry-malformed",
            detail: format!(
                "touched-specs entry `{entry}` must be `<name>:new` or `<name>:modified`",
            ),
        })?;
        let kind = match kind {
            "new" => SpecKind::New,
            "modified" => SpecKind::Modified,
            other => {
                return Err(Error::Diag {
                    code: "touched-specs-kind-invalid",
                    detail: format!("touched-specs kind `{other}` must be `new` or `modified`"),
                });
            }
        };
        out.push(TouchedSpec {
            name: name.to_string(),
            kind,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub(super) fn overlap(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    let slices_dir = ctx.slices_dir();
    let overlaps = slice_actions::overlap(&slices_dir, &name)?;
    let rows: Vec<OverlapRow> = overlaps.iter().map(OverlapRow::from_overlap).collect();

    emit(ctx.format, &OverlapBody { name, overlaps: rows })?;
    Ok(CliResult::Success)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OverlapBody {
    name: String,
    overlaps: Vec<OverlapRow>,
}

impl Render for OverlapBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.overlaps.is_empty() {
            return writeln!(w, "{}: no overlapping slices", self.name);
        }
        for o in &self.overlaps {
            writeln!(
                w,
                "{}: also touched by `{}` ({} vs {})",
                o.capability, o.other_slice, o.our_spec_type, o.other_spec_type,
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OverlapRow {
    capability: String,
    other_slice: String,
    our_spec_type: String,
    other_spec_type: String,
}

impl OverlapRow {
    fn from_overlap(o: &specify_slice::Overlap) -> Self {
        Self {
            capability: o.capability.clone(),
            other_slice: o.other.clone(),
            our_spec_type: o.ours.to_string(),
            other_spec_type: o.theirs.to_string(),
        }
    }
}
