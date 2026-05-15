//! `slice outcome set | show` — phase-outcome bookkeeping on `.metadata.yaml`.

use std::io::Write;
use std::path::Path;

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::capability::Phase;
use specify_domain::config::Layout;
use specify_domain::slice::{Outcome, OutcomeKind, SliceMetadata, actions as slice_actions};
use specify_error::{Error, Result};

use super::cli::{OutcomeKindAction, RegistryAmendmentProposal};
use crate::context::Ctx;

pub(super) fn set(ctx: &Ctx, name: String, phase: Phase, kind: OutcomeKindAction) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let (outcome, summary, context) = lower_kind(kind);

    let metadata = slice_actions::stamp_outcome(
        &slice_dir,
        phase,
        outcome.clone(),
        &summary,
        context.as_deref(),
        Timestamp::now(),
    )?;

    let stamped = metadata
        .outcome
        .as_ref()
        .expect("stamp_outcome action must set metadata.outcome on success");

    ctx.write(
        &PhaseStampBody {
            slice: name,
            phase: phase.to_string(),
            outcome: outcome.to_string(),
            at: stamped.at,
        },
        write_phase_stamp_text,
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PhaseStampBody {
    slice: String,
    phase: String,
    outcome: String,
    #[serde(with = "specify_error::serde_rfc3339")]
    at: Timestamp,
}

fn write_phase_stamp_text(w: &mut dyn Write, body: &PhaseStampBody) -> std::io::Result<()> {
    writeln!(
        w,
        "Stamped outcome '{}' for phase '{}' on slice '{}'.",
        body.outcome, body.phase, body.slice,
    )
}

/// Lower a `slice outcome set` subcommand into the wire `OutcomeKind`,
/// summary, and optional context. clap has already enforced
/// per-variant flag presence; no runtime guard required.
fn lower_kind(kind: OutcomeKindAction) -> (OutcomeKind, String, Option<String>) {
    match kind {
        OutcomeKindAction::Success { summary, context } => (OutcomeKind::Success, summary, context),
        OutcomeKindAction::Failure { summary, context } => (OutcomeKind::Failure, summary, context),
        OutcomeKindAction::Deferred { summary, context } => {
            (OutcomeKind::Deferred, summary, context)
        }
        OutcomeKindAction::RegistryAmendmentRequired {
            summary,
            context,
            proposal:
                RegistryAmendmentProposal {
                    proposed_name,
                    proposed_url,
                    proposed_capability,
                    proposed_description,
                    rationale,
                },
        } => {
            let summary =
                summary.unwrap_or_else(|| format!("registry-amendment-required: {proposed_name}"));
            let outcome = OutcomeKind::RegistryAmendmentRequired {
                proposed_name,
                proposed_url,
                proposed_capability,
                proposed_description,
                rationale,
            };
            (outcome, summary, context)
        }
    }
}

/// Report the stamped `.metadata.yaml.outcome` for `name`.
///
/// Symmetric with [`set`]: this is the read verb `/change:execute`
/// consumes after a phase returns. Emits a null `outcome` when the
/// slice exists but nothing has been stamped; exits
/// `Exit::Success` in both cases — an unstamped slice is not an
/// error, just an absence.
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
        Some(o) => {
            writeln!(w, "{}: {}/{} — {}", body.name, o.phase, o.kind, o.summary)?;
            if let OutcomeKind::RegistryAmendmentRequired {
                proposed_name,
                proposed_url,
                proposed_capability,
                proposed_description,
                rationale,
            } = &o.kind
            {
                writeln!(w, "  proposed-name: {proposed_name}")?;
                writeln!(w, "  proposed-url: {proposed_url}")?;
                writeln!(w, "  proposed-capability: {proposed_capability}")?;
                if let Some(desc) = proposed_description {
                    writeln!(w, "  proposed-description: {desc}")?;
                }
                writeln!(w, "  rationale: {rationale}")?;
            }
            Ok(())
        }
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
    let mut candidates: Vec<(Option<Timestamp>, SliceMetadata)> = Vec::new();

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

    if candidates.is_empty() {
        return Err(Error::SliceNotFound {
            name: slice_name.to_string(),
        });
    }

    let (_, metadata) = candidates
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0))
        .expect("candidates is non-empty (checked above)");
    Ok(metadata)
}
