//! `slice outcome show` — read phase-outcome bookkeeping from `.metadata.yaml`.

use std::io::Write;
use std::path::Path;

use serde::Serialize;
use specify_domain::config::Layout;
use specify_domain::slice::{Outcome, SliceMetadata};
use specify_error::{Error, Result};

use crate::context::Ctx;

/// Report the stamped `.metadata.yaml.outcome` for `name`.
///
/// Emits a null `outcome` when the slice exists but nothing has been
/// stamped; exits `Exit::Success` in both cases — an unstamped slice is
/// not an error, just an absence.
///
/// Falls back to `.specify/archive/` when the slice is not found
/// under `.specify/slices/`. This handles the post-merge case:
/// `slice merge run` stamps the outcome into `.metadata.yaml` and
/// then archives the slice directory, so the active path no longer
/// exists.
pub(super) fn show(ctx: &Ctx, name: String) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let metadata = if slice_dir.is_dir() {
        SliceMetadata::load(&slice_dir)?
    } else {
        resolve_archived_metadata(&ctx.project_dir, &name)?
    };

    ctx.write(
        &ShowBody {
            name,
            outcome: metadata.outcome.as_ref(),
        },
        write_show_text,
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody<'a> {
    name: String,
    outcome: Option<&'a Outcome>,
}

fn write_show_text(w: &mut dyn Write, body: &ShowBody<'_>) -> std::io::Result<()> {
    match body.outcome {
        None => writeln!(w, "{}: no outcome stamped", body.name),
        Some(o) => writeln!(w, "{}: {}/{} — {}", body.name, o.phase, o.kind, o.summary),
    }
}

/// Scan `.specify/archive/` for directories whose name ends with
/// `-<slice_name>` (the `YYYY-MM-DD-<name>` convention), load each
/// candidate's `.metadata.yaml`, and return the most recent by
/// `created-at`. Used as a fallback when the active slice
/// directory has been archived by `slice merge run`.
fn resolve_archived_metadata(project_dir: &Path, slice_name: &str) -> Result<SliceMetadata> {
    let archive_dir = Layout::new(project_dir).archive_dir();
    let suffix = format!("-{slice_name}");
    let mut candidates: Vec<(Option<jiff::Timestamp>, SliceMetadata)> = Vec::new();

    if archive_dir.is_dir() {
        let entries = std::fs::read_dir(&archive_dir)?;
        for entry in entries {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.ends_with(&suffix) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if let Ok(meta) = SliceMetadata::load(&entry.path()) {
                let created = meta.created_at;
                candidates.push((created, meta));
            }
        }
    }

    match candidates.into_iter().max_by_key(|(created, _)| *created) {
        Some((_, metadata)) => Ok(metadata),
        None => Err(Error::Diag {
            code: "slice-not-found",
            detail: format!("slice '{slice_name}' not found"),
        }),
    }
}
