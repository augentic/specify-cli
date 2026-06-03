//! Shared source-operation kernel for `survey` and `extract`
//! (REVIEW.md A3 / A6).
//!
//! `survey.rs` and `extract.rs` run the same two-phase agent/tool flow
//! around an adapter-declared brief: resolve the sandbox scratch path,
//! read the staged artifact, build the closed [`CacheFingerprint`], and
//! append the cache index row. The only axis of variation is the
//! [`SourceOperation`] (`survey` vs `extract`) and its `lead` input, so
//! these helpers are parameterised by it and preserve each operation's
//! wire-stable diagnostic codes via a match on the op.
//!
//! [`run`] drives the whole flow: `tool` adapters run single-phase;
//! `agent` adapters split into `prepare` / `finalize` (RFC-29 D9). Each
//! operation supplies its handoff envelope, commit step (lead-set merge
//! / Evidence persist), cache event, and tool-dispatch error through
//! the [`Flow`] trait, while [`Common`] carries the shared inputs.

use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_error::{Error, Result};
use specify_workflow::adapter::cache::{
    self, CacheFingerprint, CacheIndexEntry, FingerprintSource, FingerprintToolVersion,
};
use specify_workflow::adapter::{CacheLayout, CacheMode, Execution, SourceOperation};
use specify_workflow::change::SourceBinding;
use specify_workflow::journal::{self, Event, EventKind};

use crate::runtime::commands::source::cli::Phase;
use crate::runtime::commands::source::prep;
use crate::runtime::context::Ctx;

/// The `$SCRATCH_DIR` host path the prep mounted for this operation.
///
/// `survey` / `extract` prep always mounts a scratch root (preflight
/// §1); a `None` is an unreachable prep-invariant violation surfaced as
/// a diagnostic rather than a panic (REVIEW.md A6).
///
/// # Errors
///
/// Returns `source-scratch-missing` when the prep mounted no scratch
/// root (an unreachable prep-invariant violation).
pub(super) fn scratch_path(prepared: &prep::SourcePrep, op: SourceOperation) -> Result<PathBuf> {
    prepared.layout.scratch.path.clone().ok_or_else(|| Error::Diag {
        code: "source-scratch-missing",
        detail: format!("{op} prep mounted no $SCRATCH_DIR host path"),
    })
}

/// Read the staged artifact (`lead-set.md` / `evidence.yaml`), mapping a
/// missing file to the operation's wire-stable "must write into
/// `$SCRATCH_DIR` before finalize" diagnostic.
///
/// # Errors
///
/// Returns the operation's `*-missing` diagnostic when the artifact is
/// absent, or [`Error::Io`] on any other read failure.
pub(super) fn read_artifact(path: &Path, op: SourceOperation) -> Result<String> {
    std::fs::read_to_string(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            let (code, what, writer) = match op {
                SourceOperation::Survey => {
                    ("survey-lead-set-missing", "lead-set.md", "the survey must write the lead set")
                }
                SourceOperation::Extract => (
                    "extract-evidence-missing",
                    "evidence.yaml",
                    "the extract must write the Evidence",
                ),
            };
            Error::Diag {
                code,
                detail: format!(
                    "no `{what}` at {}; {writer} into $SCRATCH_DIR before finalize",
                    path.display()
                ),
            }
        } else {
            Error::Io(err)
        }
    })
}

/// Build the closed [`CacheFingerprint`] (RFC-27) for a source
/// operation: source identity, `<name>@<version>`, the operation's
/// brief sha256, the declared tool versions, and the optional `lead`
/// input (`None` for `survey`, `Some(<lead>)` for `extract`).
///
/// # Errors
///
/// - the operation's `*-brief-missing` diagnostic when the manifest
///   declares no brief for it.
/// - the operation's `*-brief-read-failed` diagnostic on a brief read
///   error.
/// - propagates [`FingerprintSource::from_path`] failures.
pub(super) fn build_fingerprint(
    prepared: &prep::SourcePrep, source_path: Option<&Path>, binding: &SourceBinding,
    op: SourceOperation, lead: Option<String>,
) -> Result<CacheFingerprint> {
    let source = match source_path {
        Some(path) => FingerprintSource::from_path(path)?,
        None => {
            FingerprintSource::from_value(binding.value.as_deref().unwrap_or_default().as_bytes())
        }
    };
    let adapter = format!("{}@{}", prepared.manifest.name, prepared.manifest.version);

    let (missing_code, read_code, label) = match op {
        SourceOperation::Survey => ("survey-brief-missing", "survey-brief-read-failed", "survey"),
        SourceOperation::Extract => {
            ("extract-brief-missing", "extract-brief-read-failed", "extract")
        }
    };
    let brief_relative = prepared.manifest.briefs.get(&op).ok_or_else(|| Error::Diag {
        code: missing_code,
        detail: format!("source adapter `{}` declares no `{label}` brief", prepared.manifest.name),
    })?;
    let brief_path = prepared.adapter_dir.join(brief_relative);
    let brief_bytes = std::fs::read(&brief_path).map_err(|err| Error::Diag {
        code: read_code,
        detail: format!("failed to read {label} brief {}: {err}", brief_path.display()),
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
        lead,
    ))
}

/// Per-write cache-entry identity bundle, keeping
/// [`write_cache_entry`] under the argument-count budget.
pub(super) struct CacheEntry<'a> {
    pub layout: CacheLayout<'a>,
    pub cache_mode: Option<CacheMode>,
    /// Cache-index `slice` lane (`survey` for the slice-less survey op,
    /// the slice name for extract).
    pub slice_lane: &'a str,
    pub source: &'a str,
    pub adapter: &'a str,
    pub op: SourceOperation,
}

/// Write the cache artifact + `fingerprint.json` + index row for a
/// source operation. Under the forced opt-out the cache layer skips the
/// directory body and appends only the audit index row.
///
/// # Errors
///
/// Propagates [`cache::write`] failures.
pub(super) fn write_cache_entry(
    entry: &CacheEntry<'_>, fingerprint: &CacheFingerprint, artifact_bytes: &[u8],
) -> Result<()> {
    let index = CacheIndexEntry {
        timestamp: Timestamp::now(),
        fingerprint: fingerprint.digest(),
        slice: entry.slice_lane.to_string(),
        source: entry.source.to_string(),
        adapter: entry.adapter.to_string(),
        operation: entry.op,
    };
    cache::write(
        entry.layout,
        fingerprint,
        artifact_bytes,
        entry.op.artifact_name(),
        entry.cache_mode,
        &index,
    )
}

/// Per-invocation inputs shared by every source-operation flow
/// function. Survey and extract differ only in their operation-specific
/// behaviour (the [`Flow`] methods); this bundle carries the rest, so
/// the kernel functions stay under the argument-count budget.
pub(super) struct Common<'a> {
    pub ctx: &'a Ctx,
    /// Plan source key (`plan.yaml.sources.<source>`).
    pub source: &'a str,
    pub prepared: &'a prep::SourcePrep,
    /// Bound `$SOURCE_DIR` host path; `None` for value-bound sources.
    pub source_path: Option<&'a Path>,
    pub binding: &'a SourceBinding,
    /// Operation this flow runs (`survey` / `extract`).
    pub operation: SourceOperation,
    /// Cache-index `slice` lane: `survey` for the slice-less survey op,
    /// the slice name for extract.
    pub slice_lane: &'a str,
    /// `lead` fingerprint input — `None` for survey, `Some` for
    /// extract.
    pub lead: Option<&'a str>,
}

/// Operation-specific seam for the shared survey/extract two-phase
/// flow. The kernel ([`run`]) owns resolution, the cache probe, the
/// journal events, and rendering; each operation supplies its handoff
/// envelope, commit step (lead-set merge / Evidence persist), cache
/// event, and tool-dispatch error here.
pub(super) trait Flow<'a> {
    /// Agent `prepare` handoff envelope body.
    type Handoff: Serialize;
    /// Completed-operation result body (tool / agent `finalize`).
    type Outcome: Serialize;

    /// The shared per-invocation inputs.
    fn common(&self) -> &Common<'a>;

    /// Build the agent handoff envelope for the mounted `scratch` root.
    fn handoff(&self, scratch: PathBuf) -> Result<Self::Handoff>;
    /// Render the handoff envelope text body.
    fn write_handoff(w: &mut dyn Write, body: &Self::Handoff) -> std::io::Result<()>;

    /// Validate the staged artifact and commit it — merge the lead set
    /// into `discovery.md` (survey) or persist the Evidence (extract) —
    /// returning the result body. `artifact_source` is the on-disk path
    /// `raw` was read from (the Evidence schema error references it).
    /// Called before the cache event and entry write, so a validation
    /// failure leaves nothing visible.
    fn commit(
        &self, raw: &str, artifact_source: &Path, lookup: &cache::CacheLookup,
    ) -> Result<Self::Outcome>;
    /// Render the result text body.
    fn write_outcome(w: &mut dyn Write, body: &Self::Outcome) -> std::io::Result<()>;

    /// The cache hit/miss journal event kind for this operation.
    fn cache_event(&self, lookup: &cache::CacheLookup) -> EventKind;

    /// Dispatch the declared WASI tool — an M1 seam that returns the
    /// operation's `*-tool-unsupported` diagnostic.
    fn dispatch_tool(&self) -> Result<()>;
}

/// Drive the shared two-phase source-operation flow: `tool` adapters
/// run single-phase; `agent` adapters split into `prepare` / `finalize`
/// (RFC-29 D9). The CLI never blocks on agent work.
///
/// # Errors
///
/// Propagates the flow's resolution, cache, validation, and commit
/// failures.
pub(super) fn run<'a, F: Flow<'a>>(flow: &F, phase: Phase) -> Result<()> {
    match flow.common().prepared.manifest.execution {
        Some(Execution::Tool) => run_tool(flow),
        _ => match phase {
            Phase::Prepare => prepare(flow),
            Phase::Finalize => finalize(flow),
        },
    }
}

/// Agent `prepare`: build scratch, emit `source.execution.agent`, and
/// print the handoff envelope. Control returns to the agent.
fn prepare<'a, F: Flow<'a>>(flow: &F) -> Result<()> {
    let c = flow.common();
    let scratch = scratch_path(c.prepared, c.operation)?;
    std::fs::create_dir_all(&scratch).map_err(Error::Io)?;
    emit_execution_agent(c)?;
    let handoff = flow.handoff(scratch)?;
    c.ctx.write(&handoff, F::write_handoff)
}

/// Agent `finalize`: read the staged artifact, probe the cache,
/// validate-before-visible, commit, then record the cache outcome. The
/// fingerprint is built (inside [`probe`]) before the commit, so a
/// missing source path aborts before any visible write.
fn finalize<'a, F: Flow<'a>>(flow: &F) -> Result<()> {
    let c = flow.common();
    let scratch = scratch_path(c.prepared, c.operation)?;
    let artifact_source = scratch.join(c.operation.artifact_name());
    let raw = read_artifact(&artifact_source, c.operation)?;

    let probe = probe(c)?;
    let outcome = flow.commit(&raw, &artifact_source, &probe.lookup)?;
    emit_cache_event(c, flow, &probe.lookup)?;
    commit_cache_entry(c, &probe, raw.as_bytes())?;
    c.ctx.write(&outcome, F::write_outcome)
}

/// Single-phase `tool` execution: probe the cache, produce the artifact
/// (cached hit or freshly dispatched), validate-before-visible, commit,
/// then record the cache outcome (the entry write only on a miss).
fn run_tool<'a, F: Flow<'a>>(flow: &F) -> Result<()> {
    let c = flow.common();
    let scratch = scratch_path(c.prepared, c.operation)?;
    std::fs::create_dir_all(&scratch).map_err(Error::Io)?;

    let probe = probe(c)?;
    let artifact = c.operation.artifact_name();
    let (raw, artifact_source) = match &probe.lookup.outcome {
        cache::LookupOutcome::Hit { cache_dir } => {
            let path = cache_dir.join(artifact);
            (read_artifact(&path, c.operation)?, path)
        }
        cache::LookupOutcome::Miss { .. } => {
            flow.dispatch_tool()?;
            let path = scratch.join(artifact);
            (read_artifact(&path, c.operation)?, path)
        }
    };

    let outcome = flow.commit(&raw, &artifact_source, &probe.lookup)?;
    emit_cache_event(c, flow, &probe.lookup)?;
    if matches!(probe.lookup.outcome, cache::LookupOutcome::Miss { .. }) {
        commit_cache_entry(c, &probe, raw.as_bytes())?;
    }
    c.ctx.write(&outcome, F::write_outcome)
}

/// Fingerprint + cache lookup for one source operation. The fingerprint
/// is built first so a missing source path aborts before the lookup;
/// the lookup itself has no side effects.
struct Probe<'a> {
    fingerprint: CacheFingerprint,
    layout: CacheLayout<'a>,
    cache_mode: Option<CacheMode>,
    lookup: cache::CacheLookup,
}

fn probe<'a>(c: &Common<'a>) -> Result<Probe<'a>> {
    let fingerprint = build_fingerprint(
        c.prepared,
        c.source_path,
        c.binding,
        c.operation,
        c.lead.map(str::to_string),
    )?;
    let layout = CacheLayout::new(&c.ctx.project_dir, &c.prepared.manifest.name);
    let cache_mode = c.prepared.manifest.effective_cache_mode();
    let lookup =
        cache::lookup(layout, &fingerprint, cache_mode, c.slice_lane, c.source, c.operation)?;
    Ok(Probe {
        fingerprint,
        layout,
        cache_mode,
        lookup,
    })
}

/// Write the cache artifact + `fingerprint.json` + index row for a
/// completed probe.
fn commit_cache_entry(c: &Common<'_>, probe: &Probe<'_>, artifact_bytes: &[u8]) -> Result<()> {
    write_cache_entry(
        &CacheEntry {
            layout: probe.layout,
            cache_mode: probe.cache_mode,
            slice_lane: c.slice_lane,
            source: c.source,
            adapter: &c.prepared.manifest.name,
            op: c.operation,
        },
        &probe.fingerprint,
        artifact_bytes,
    )
}

/// Emit the `source.execution.agent` event marking the agent `prepare`
/// handoff.
fn emit_execution_agent(c: &Common<'_>) -> Result<()> {
    let event = Event::new(
        Timestamp::now(),
        EventKind::SourceExecutionAgent {
            source: c.source.to_string(),
            adapter: c.prepared.manifest.name.clone(),
            operation: c.operation,
        },
    );
    journal::append_batch(c.ctx.layout(), std::slice::from_ref(&event))
}

/// Emit the operation's cache hit/miss journal event for `lookup`.
fn emit_cache_event<'a, F: Flow<'a>>(
    c: &Common<'a>, flow: &F, lookup: &cache::CacheLookup,
) -> Result<()> {
    let event = Event::new(Timestamp::now(), flow.cache_event(lookup));
    journal::append_batch(c.ctx.layout(), std::slice::from_ref(&event))
}
