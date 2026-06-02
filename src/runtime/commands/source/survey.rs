//! `specrun source survey` handler — plan-time lead discovery
//! (RFC-29 D1; DECISIONS.md §"Source operations (D1)").
//!
//! Resolves `<source>` against `plan.yaml.sources.<key>`, runs the
//! shared [`prep`] seam ([`prep::SourceOp::Survey`]) for adapter
//! resolution, brief directory, and the four-root sandbox (scratch at
//! `.specify/.cache/extractions/<adapter>/survey/scratch/`), then
//! branches on the adapter's `execution` mode:
//!
//! - `tool`: single-phase. Probe the extraction cache; on a hit read the
//!   cached `lead-set.md`, on a miss dispatch the declared tool (an M1
//!   seam — no first-party source declares a survey tool yet). Either
//!   way validate the lead set and merge it into `discovery.md`.
//! - `agent`: two-phase (RFC-29 D9; DECISIONS.md §"Adapter execution
//!   mode (D9)"). The CLI never blocks on agent work.
//!   - `--phase prepare` (default): build scratch, emit
//!     `source.execution.agent`, and print the survey handoff envelope
//!     (`{ adapter, version, briefs-dir, source-dir?, scratch-dir,
//!     leads[], execution }` — no `evidence-dir`; survey produces a lead
//!     set, not Evidence). Control returns to the agent.
//!   - `--phase finalize`: validate the agent-produced `lead-set.md`
//!     against `schemas/discovery/lead.schema.json` *before* it becomes
//!     visible, run the extraction-cache fingerprint, emit
//!     `source.survey.cache-hit` / `cache-miss`, and merge the lead set
//!     via `Discovery::merge_survey`. Under the `execution: agent`
//!     forced opt-out this is always a `cache-miss` with
//!     `reason: adapter-opt-out`.

use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_error::{Error, Result};
use specify_model::discovery::Discovery;
use specify_workflow::adapter::cache::{
    self, CacheFingerprint, CacheIndexEntry, FingerprintSource, FingerprintToolVersion,
    LookupOutcome,
};
use specify_workflow::adapter::{CacheLayout, Execution, SourceOperation};
use specify_workflow::change::{Plan, SourceBinding};
use specify_workflow::journal::{self, CacheMissReason, Event, EventKind};
use specify_workflow::schema;

use crate::runtime::commands::source::cli::Phase;
use crate::runtime::commands::source::prep;
use crate::runtime::context::Ctx;

/// Cache-index `slice` lane for the slice-less `survey` operation —
/// mirrors the `survey/` scratch segment (preflight §1) so survey
/// results occupy their own discoverable lane in `index.jsonl`.
const SURVEY_LANE: &str = "survey";

/// Survey handoff envelope printed by the agent `prepare` phase
/// (preflight §2). No `evidence-dir`: survey merges a lead set via
/// `merge_survey`, it does not persist Evidence.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SurveyHandoff {
    adapter: String,
    version: u32,
    briefs_dir: PathBuf,
    /// `$SOURCE_DIR` — absent for value-bound sources (e.g. `intent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    source_dir: Option<PathBuf>,
    scratch_dir: PathBuf,
    /// Existing leads for this source — the re-survey baseline. Always
    /// present (empty on a fresh survey) per preflight §2.
    leads: Vec<String>,
    execution: &'static str,
}

/// Result of a completed survey (tool single-phase, or agent
/// `finalize`): the cache outcome plus the merged lead ids.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SurveyResult {
    adapter: String,
    source: String,
    fingerprint: String,
    /// `hit` | `miss`.
    cache: &'static str,
    /// Populated on a miss; the closed cache-miss reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<CacheMissReason>,
    /// Lead ids merged into `discovery.md`.
    leads: Vec<String>,
    discovery: PathBuf,
}

/// Run `specrun source survey <source> [--plan <name>]
/// [--phase prepare|finalize]`.
///
/// # Errors
///
/// - `source-unknown` when `<source>` is not a
///   `plan.yaml.sources` key.
/// - propagates adapter-resolution, schema-validation, fingerprint,
///   and merge failures.
pub fn run(ctx: &Ctx, source: &str, plan_name: Option<&str>, phase: Phase) -> Result<()> {
    let plan = load_plan(ctx, plan_name)?;
    let binding = plan.sources.get(source).ok_or_else(|| Error::Diag {
        code: "source-unknown",
        detail: format!(
            "no source `{source}` in plan.yaml.sources; `specrun source survey` resolves \
             its argument against the plan's source keys, not the adapter name"
        ),
    })?;

    let source_path =
        binding.path.as_deref().map(|raw| prep::resolve_source_path(&ctx.project_dir, raw));

    let prepared = prep::prepare(&prep::PrepRequest {
        adapter: &binding.adapter,
        project_dir: &ctx.project_dir,
        op: prep::SourceOp::Survey,
        source: source_path.as_deref(),
        leads: &[],
        evidence_root: None,
    })?;

    match prepared.manifest.execution {
        Some(Execution::Tool) => run_tool(ctx, source, &prepared, source_path.as_deref(), binding),
        _ => match phase {
            Phase::Prepare => prepare(ctx, source, &prepared, source_path.as_deref()),
            Phase::Finalize => finalize(ctx, source, &prepared, source_path.as_deref(), binding),
        },
    }
}

/// Agent `prepare` phase: build scratch, emit `source.execution.agent`,
/// and print the survey handoff envelope. The CLI returns control to
/// the agent and does not block.
fn prepare(
    ctx: &Ctx, source: &str, prepared: &prep::SourcePrep, source_path: Option<&Path>,
) -> Result<()> {
    let scratch = scratch_path(prepared);
    std::fs::create_dir_all(&scratch).map_err(Error::Io)?;

    let event = Event::new(
        Timestamp::now(),
        EventKind::SourceExecutionAgent {
            source: source.to_string(),
            adapter: prepared.manifest.name.clone(),
            operation: SourceOperation::Survey,
        },
    );
    journal::append_batch(ctx.layout(), std::slice::from_ref(&event))?;

    let handoff = SurveyHandoff {
        adapter: prepared.manifest.name.clone(),
        version: prepared.manifest.version,
        briefs_dir: prepared.briefs_dir.clone(),
        source_dir: source_path.map(Path::to_path_buf),
        scratch_dir: scratch,
        leads: existing_lead_ids(ctx, source)?,
        execution: "agent",
    };
    ctx.write(&handoff, write_handoff_text)
}

/// Agent `finalize` phase: validate the agent-produced lead set, merge
/// it into `discovery.md`, then record the cache outcome.
fn finalize(
    ctx: &Ctx, source: &str, prepared: &prep::SourcePrep, source_path: Option<&Path>,
    binding: &SourceBinding,
) -> Result<()> {
    let scratch = scratch_path(prepared);
    let raw = read_lead_set(&scratch.join(SourceOperation::Survey.artifact_name()))?;

    // Fingerprint first so a missing source path aborts before any
    // discovery.md write; the lookup itself has no side effects.
    let fingerprint = survey_fingerprint(prepared, source_path, binding)?;
    let layout = CacheLayout::new(&ctx.project_dir, &prepared.manifest.name);
    let cache_mode = prepared.manifest.effective_cache_mode();
    let lookup = cache::lookup(
        layout,
        &fingerprint,
        cache_mode,
        SURVEY_LANE,
        source,
        SourceOperation::Survey,
    )?;

    // Validate-before-visible: a schema failure returns here, before the
    // cache event is emitted and before discovery.md is touched.
    let lead_ids = validate_and_merge(ctx, source, &raw)?;

    emit_cache_event(ctx, source, &prepared.manifest.name, &lookup)?;
    write_cache_entry(
        layout,
        &fingerprint,
        raw.as_bytes(),
        cache_mode,
        source,
        &prepared.manifest.name,
    )?;

    ctx.write(
        &survey_result(
            source,
            &prepared.manifest.name,
            &lookup,
            lead_ids,
            ctx.layout().discovery_path(),
        ),
        write_result_text,
    )
}

/// Single-phase `tool` execution: probe the cache, produce the lead set
/// (cached hit or freshly dispatched), validate, and merge.
fn run_tool(
    ctx: &Ctx, source: &str, prepared: &prep::SourcePrep, source_path: Option<&Path>,
    binding: &SourceBinding,
) -> Result<()> {
    let scratch = scratch_path(prepared);
    std::fs::create_dir_all(&scratch).map_err(Error::Io)?;

    let fingerprint = survey_fingerprint(prepared, source_path, binding)?;
    let layout = CacheLayout::new(&ctx.project_dir, &prepared.manifest.name);
    let cache_mode = prepared.manifest.effective_cache_mode();
    let lookup = cache::lookup(
        layout,
        &fingerprint,
        cache_mode,
        SURVEY_LANE,
        source,
        SourceOperation::Survey,
    )?;

    let artifact = SourceOperation::Survey.artifact_name();
    let raw = match &lookup.outcome {
        LookupOutcome::Hit { cache_dir } => read_lead_set(&cache_dir.join(artifact))?,
        LookupOutcome::Miss { .. } => {
            dispatch_survey_tool(prepared)?;
            read_lead_set(&scratch.join(artifact))?
        }
    };

    let lead_ids = validate_and_merge(ctx, source, &raw)?;
    emit_cache_event(ctx, source, &prepared.manifest.name, &lookup)?;
    if matches!(lookup.outcome, LookupOutcome::Miss { .. }) {
        write_cache_entry(
            layout,
            &fingerprint,
            raw.as_bytes(),
            cache_mode,
            source,
            &prepared.manifest.name,
        )?;
    }

    ctx.write(
        &survey_result(
            source,
            &prepared.manifest.name,
            &lookup,
            lead_ids,
            ctx.layout().discovery_path(),
        ),
        write_result_text,
    )
}

/// Dispatch the declared `survey` WASI tool / built-in Rust path.
///
/// M1 ships no first-party survey tool; the WASI survey dispatch
/// protocol is out of scope for RFC-29 M1. The control flow above is
/// wired correctly (cache probe, lead-set read, validate-before-visible
/// merge) so the only seam left is the actual tool invocation.
fn dispatch_survey_tool(prepared: &prep::SourcePrep) -> Result<()> {
    Err(Error::Diag {
        code: "source-survey-tool-unsupported",
        detail: format!(
            "source adapter `{}` declares `execution: tool`, but M1 ships no `survey` tool \
             dispatch; no first-party source declares a survey tool",
            prepared.manifest.name
        ),
    })
}

/// Parse, schema-validate, and merge a lead set into `discovery.md`.
/// Returns the merged lead ids. The schema check gates the merge, so an
/// invalid lead set leaves `discovery.md` untouched (RFC-29 D1).
fn validate_and_merge(ctx: &Ctx, source: &str, raw: &str) -> Result<Vec<String>> {
    let mut leads = Discovery::parse_lead_set(raw)?.into_leads();
    if leads.is_empty() && !raw.trim().is_empty() {
        return Err(Error::Diag {
            code: "survey-lead-set-empty",
            detail: "lead-set.md contains text but no leads were parsed; each lead must be a \
                     `### <lead>` heading followed by `lead:` and `synopsis:` bullets using \
                     `-` or `*` markers"
                .to_string(),
        });
    }
    // Attribution is CLI-owned: a `survey` for `source` produces
    // `source`'s leads, so stamp every lead before the schema check
    // (which requires `source`) and the merge.
    for lead in &mut leads {
        lead.source = source.to_string();
    }
    schema::validate_leads(&leads)?;
    let lead_ids: Vec<String> = leads.iter().map(|lead| lead.lead.clone()).collect();

    let discovery_path = ctx.layout().discovery_path();
    let mut discovery = load_or_empty_discovery(&discovery_path)?;
    discovery.merge_survey(source, leads, &discovery_path)?;
    Ok(lead_ids)
}

/// Build the closed survey [`CacheFingerprint`] (RFC-27, no `lead`):
/// source identity, `<name>@<version>`, the `survey` brief sha256, and
/// the declared tool versions.
fn survey_fingerprint(
    prepared: &prep::SourcePrep, source_path: Option<&Path>, binding: &SourceBinding,
) -> Result<CacheFingerprint> {
    let source = match source_path {
        Some(path) => FingerprintSource::from_path(path)?,
        None => {
            FingerprintSource::from_value(binding.value.as_deref().unwrap_or_default().as_bytes())
        }
    };
    let adapter = format!("{}@{}", prepared.manifest.name, prepared.manifest.version);

    let brief_relative =
        prepared.manifest.briefs.get(&SourceOperation::Survey).ok_or_else(|| Error::Diag {
            code: "survey-brief-missing",
            detail: format!(
                "source adapter `{}` declares no `survey` brief",
                prepared.manifest.name
            ),
        })?;
    let brief_path = prepared.adapter_dir.join(brief_relative);
    let brief_bytes = std::fs::read(&brief_path).map_err(|err| Error::Diag {
        code: "survey-brief-read-failed",
        detail: format!("failed to read survey brief {}: {err}", brief_path.display()),
    })?;

    let tool_versions = prepared
        .manifest
        .tools
        .iter()
        .map(|tool| FingerprintToolVersion {
            name: tool.name.clone(),
            version: tool.version.clone(),
        })
        .collect();

    Ok(CacheFingerprint::new(
        source,
        adapter,
        cache::sha256_prefixed(&brief_bytes),
        tool_versions,
        None,
    ))
}

/// Emit the `source.survey.cache-hit` / `cache-miss` journal event for
/// `lookup`.
fn emit_cache_event(
    ctx: &Ctx, source: &str, adapter: &str, lookup: &cache::CacheLookup,
) -> Result<()> {
    let kind = match &lookup.outcome {
        LookupOutcome::Hit { .. } => EventKind::SourceSurveyCacheHit {
            source: source.to_string(),
            adapter: adapter.to_string(),
            fingerprint: lookup.digest.clone(),
        },
        LookupOutcome::Miss { reason } => EventKind::SourceSurveyCacheMiss {
            source: source.to_string(),
            adapter: adapter.to_string(),
            fingerprint: lookup.digest.clone(),
            reason: *reason,
        },
    };
    let event = Event::new(Timestamp::now(), kind);
    journal::append_batch(ctx.layout(), std::slice::from_ref(&event))
}

/// Write the cache artifact + `fingerprint.json` + index row. Under the
/// forced opt-out the cache layer skips the directory body and appends
/// only the audit index row.
fn write_cache_entry(
    layout: CacheLayout<'_>, fingerprint: &CacheFingerprint, artifact_bytes: &[u8],
    cache_mode: Option<specify_workflow::adapter::CacheMode>, source: &str, adapter: &str,
) -> Result<()> {
    let entry = CacheIndexEntry {
        timestamp: Timestamp::now(),
        fingerprint: fingerprint.digest(),
        slice: SURVEY_LANE.to_string(),
        source: source.to_string(),
        adapter: adapter.to_string(),
        operation: SourceOperation::Survey,
    };
    cache::write(
        layout,
        fingerprint,
        artifact_bytes,
        SourceOperation::Survey.artifact_name(),
        cache_mode,
        &entry,
    )
}

fn survey_result(
    source: &str, adapter: &str, lookup: &cache::CacheLookup, leads: Vec<String>,
    discovery: PathBuf,
) -> SurveyResult {
    let (cache, reason) = match &lookup.outcome {
        LookupOutcome::Hit { .. } => ("hit", None),
        LookupOutcome::Miss { reason } => ("miss", Some(*reason)),
    };
    SurveyResult {
        adapter: adapter.to_string(),
        source: source.to_string(),
        fingerprint: lookup.digest.clone(),
        cache,
        reason,
        leads,
        discovery,
    }
}

/// Existing lead ids for `source`, read from `discovery.md` when
/// present — the re-survey baseline echoed into the handoff envelope.
fn existing_lead_ids(ctx: &Ctx, source: &str) -> Result<Vec<String>> {
    let discovery_path = ctx.layout().discovery_path();
    if !discovery_path.exists() {
        return Ok(Vec::new());
    }
    let discovery = Discovery::load(&discovery_path)?;
    Ok(discovery
        .leads()
        .iter()
        .filter(|lead| lead.source == source)
        .map(|lead| lead.lead.clone())
        .collect())
}

/// Load `discovery.md`, or start from an empty document when the file
/// is absent so the first survey can author the inventory.
fn load_or_empty_discovery(path: &Path) -> Result<Discovery> {
    if path.exists() { Discovery::load(path) } else { Discovery::parse("") }
}

/// Read the `lead-set.md` artifact, mapping a missing file to the
/// `survey-lead-set-missing` diagnostic.
fn read_lead_set(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::Diag {
                code: "survey-lead-set-missing",
                detail: format!(
                    "no `lead-set.md` at {}; the survey must write the lead set into \
                     $SCRATCH_DIR before finalize",
                    path.display()
                ),
            }
        } else {
            Error::Io(err)
        }
    })
}

fn load_plan(ctx: &Ctx, plan_name: Option<&str>) -> Result<Plan> {
    let plan = Plan::load(&ctx.layout().plan_path())?;
    if let Some(name) = plan_name
        && name != plan.name
    {
        return Err(Error::Argument {
            flag: "--plan",
            detail: format!(
                "--plan `{name}` does not match the active plan `{}` at plan.yaml",
                plan.name
            ),
        });
    }
    Ok(plan)
}

/// The `$SCRATCH_DIR` host path the prep mounted for this survey.
/// Always `Some` for the survey op (preflight §1); the `expect` pins
/// that invariant.
fn scratch_path(prepared: &prep::SourcePrep) -> PathBuf {
    prepared
        .layout
        .scratch
        .path
        .clone()
        .expect("survey prep always mounts a $SCRATCH_DIR host path")
}

fn write_handoff_text(w: &mut dyn Write, body: &SurveyHandoff) -> std::io::Result<()> {
    writeln!(w, "adapter: {} v{}", body.adapter, body.version)?;
    writeln!(w, "execution: {}", body.execution)?;
    writeln!(w, "briefs-dir: {}", body.briefs_dir.display())?;
    if let Some(source_dir) = &body.source_dir {
        writeln!(w, "source-dir: {}", source_dir.display())?;
    }
    writeln!(w, "scratch-dir: {}", body.scratch_dir.display())?;
    if body.leads.is_empty() {
        writeln!(w, "leads: (none)")?;
    } else {
        writeln!(w, "leads: {}", body.leads.join(", "))?;
    }
    Ok(())
}

fn write_result_text(w: &mut dyn Write, body: &SurveyResult) -> std::io::Result<()> {
    writeln!(w, "adapter: {}", body.adapter)?;
    writeln!(w, "source: {}", body.source)?;
    write!(w, "cache: {}", body.cache)?;
    if let Some(reason) = body.reason {
        write!(w, " ({reason})")?;
    }
    writeln!(w)?;
    writeln!(w, "fingerprint: {}", body.fingerprint)?;
    writeln!(w, "discovery: {}", body.discovery.display())?;
    if body.leads.is_empty() {
        writeln!(w, "leads: (none)")?;
    } else {
        writeln!(w, "leads: {}", body.leads.join(", "))?;
    }
    Ok(())
}
