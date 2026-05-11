//! `slice outcome set | show` — phase-outcome bookkeeping on `.metadata.yaml`.

use std::io::Write;
use std::path::Path;

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use specify_capability::Phase;
use specify_config::ProjectConfig;
use specify_error::{Error, Result};
use specify_slice::{Outcome, Rfc3339Stamp, SliceMetadata, actions as slice_actions};

use crate::cli::{OutcomeKindAction, RegistryAmendmentArgs};
use crate::context::CommandContext;
use crate::output::{Render, Stream, emit};

pub(super) fn set(
    ctx: &CommandContext, name: String, phase: Phase, kind: OutcomeKindAction,
) -> Result<()> {
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
        Utc::now(),
    )?;

    let stamped = metadata
        .outcome
        .as_ref()
        .expect("stamp_outcome action must set metadata.outcome on success");

    emit(
        Stream::Stdout,
        ctx.format,
        &PhaseStampBody {
            slice: name,
            phase: phase.to_string(),
            outcome: outcome.discriminant().to_string(),
            at: stamped.at.to_string(),
        },
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PhaseStampBody {
    slice: String,
    phase: String,
    outcome: String,
    at: String,
}

impl Render for PhaseStampBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            w,
            "Stamped outcome '{}' for phase '{}' on slice '{}'.",
            self.outcome, self.phase, self.slice,
        )
    }
}

/// Lower a `slice outcome set` subcommand into the wire `Outcome`,
/// summary, and optional context. clap has already enforced
/// per-variant flag presence; no runtime guard required.
fn lower_kind(kind: OutcomeKindAction) -> (Outcome, String, Option<String>) {
    match kind {
        OutcomeKindAction::Success { summary, context } => (Outcome::Success, summary, context),
        OutcomeKindAction::Failure { summary, context } => (Outcome::Failure, summary, context),
        OutcomeKindAction::Deferred { summary, context } => (Outcome::Deferred, summary, context),
        OutcomeKindAction::RegistryAmendmentRequired(RegistryAmendmentArgs {
            summary,
            context,
            proposed_name,
            proposed_url,
            proposed_capability,
            proposed_description,
            rationale,
        }) => {
            let summary =
                summary.unwrap_or_else(|| format!("registry-amendment-required: {proposed_name}"));
            let outcome = Outcome::RegistryAmendmentRequired {
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
/// `CliResult::Success` in both cases — an unstamped slice is not an
/// error, just an absence.
///
/// Falls back to `.specify/archive/` when the slice is not found
/// under `.specify/slices/`. This handles the post-merge case:
/// `slice merge run` stamps the outcome into `.metadata.yaml` and
/// then archives the slice directory, so the active path no longer
/// exists.
pub(super) fn show(ctx: &CommandContext, name: String) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let metadata = if slice_dir.is_dir() {
        SliceMetadata::load(&slice_dir)?
    } else {
        resolve_archived_metadata(&ctx.project_dir, &name)?
    };

    let outcome = metadata.outcome.as_ref().map(OutcomeRow::from);
    emit(Stream::Stdout, ctx.format, &OutcomeShowBody { name, outcome })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OutcomeShowBody {
    name: String,
    outcome: Option<OutcomeRow>,
}

impl Render for OutcomeShowBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match &self.outcome {
            None => writeln!(w, "{}: no outcome stamped", self.name),
            Some(o) => {
                writeln!(w, "{}: {}/{} — {}", self.name, o.phase, o.outcome, o.summary)?;
                if let Some(p) = &o.proposal {
                    writeln!(w, "  proposed-name: {}", p.proposed_name)?;
                    writeln!(w, "  proposed-url: {}", p.proposed_url)?;
                    writeln!(w, "  proposed-capability: {}", p.proposed_capability)?;
                    if let Some(desc) = &p.proposed_description {
                        writeln!(w, "  proposed-description: {desc}")?;
                    }
                    writeln!(w, "  rationale: {}", p.rationale)?;
                }
                Ok(())
            }
        }
    }
}

/// One stamped phase outcome.
///
/// On disk the metadata nests the registry-amendment proposal under
/// `outcome.outcome.registry-amendment-required.*`; the CLI shape is
/// flatter — `outcome.outcome` stays a kebab-case string and the
/// structured payload is hoisted into a sibling `outcome.proposal`
/// object so existing consumers that only read `.outcome.outcome`
/// keep working unchanged.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OutcomeRow {
    phase: String,
    outcome: String,
    at: Rfc3339Stamp,
    summary: String,
    context: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal: Option<RegistryProposalRow>,
}

impl From<&specify_slice::PhaseOutcome> for OutcomeRow {
    fn from(o: &specify_slice::PhaseOutcome) -> Self {
        Self {
            phase: o.phase.to_string(),
            outcome: o.outcome.discriminant().to_string(),
            at: o.at.clone(),
            summary: o.summary.clone(),
            context: o.context.clone().map_or(Value::Null, Value::from),
            proposal: RegistryProposalRow::from_kind(&o.outcome),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct RegistryProposalRow {
    proposed_name: String,
    proposed_url: String,
    proposed_capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposed_description: Option<String>,
    rationale: String,
}

impl RegistryProposalRow {
    // Filters on `Outcome::RegistryAmendmentRequired`; returns `Option<Self>`
    // rather than `Self`, so a `From` impl would be a poor fit. Kept as a
    // named constructor.
    fn from_kind(outcome: &Outcome) -> Option<Self> {
        if let Outcome::RegistryAmendmentRequired {
            proposed_name,
            proposed_url,
            proposed_capability,
            proposed_description,
            rationale,
        } = outcome
        {
            Some(Self {
                proposed_name: proposed_name.clone(),
                proposed_url: proposed_url.clone(),
                proposed_capability: proposed_capability.clone(),
                proposed_description: proposed_description.clone(),
                rationale: rationale.clone(),
            })
        } else {
            None
        }
    }
}

/// Scan `.specify/archive/` for directories whose name ends with
/// `-<slice_name>` (the `YYYY-MM-DD-<name>` convention), load each
/// candidate's `.metadata.yaml`, and return the most recent by
/// `created-at`. Used as a fallback when the active slice
/// directory has been archived by `slice merge run`.
fn resolve_archived_metadata(project_dir: &Path, slice_name: &str) -> Result<SliceMetadata> {
    let archive_dir = ProjectConfig::archive_dir(project_dir);
    let suffix = format!("-{slice_name}");
    let mut candidates: Vec<(String, SliceMetadata)> = Vec::new();

    if archive_dir.is_dir() {
        let entries = std::fs::read_dir(&archive_dir)?;
        for entry in entries {
            let entry = entry?;
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.ends_with(&suffix) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if let Ok(meta) = SliceMetadata::load(&entry.path()) {
                let created = meta.created_at.as_deref().unwrap_or("").to_string();
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
