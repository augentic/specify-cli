//! `slice provenance` — project the audit-only provenance view from a
//! slice's single `model.yaml` (RFC-29c §"Provenance projection").
//!
//! Provenance is carried inline in `model.yaml`; this command reshapes
//! it into the per-requirement audit shape on demand. There is no
//! persisted `provenance.yaml`, so the projection cannot drift from the
//! model.

use jiff::Timestamp;
use specify_error::{Error, Result};
use specify_workflow::slice::SliceModel;

use crate::runtime::context::Ctx;

/// Generator label stamped on the projection header.
fn generator() -> String {
    format!("specify@{}", env!("CARGO_PKG_VERSION"))
}

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    let model_path = ctx.slices_dir().join(name).join("model.yaml");
    if !model_path.is_file() {
        return Err(Error::validation_failed(
            "slice-model-missing",
            "a synthesized slice carries model.yaml",
            format!(
                "slice `{name}` has no model.yaml at {}; run `specrun slice synthesize {name}` first",
                model_path.display()
            ),
        ));
    }
    let model = SliceModel::load(&model_path)?;
    let index = model.to_provenance_index(Timestamp::now(), generator())?;

    ctx.write(&index, |w, index| {
        writeln!(w, "slice: {}", index.slice)?;
        for req in &index.requirements {
            writeln!(
                w,
                "  {} [{}] {} ({} claim(s))",
                req.id,
                req.status,
                req.resolution,
                req.contributing_claims.len()
            )?;
        }
        Ok(())
    })
}
