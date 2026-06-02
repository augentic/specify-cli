//! `specrun source extract` handler — slice-time Evidence extraction
//! (RFC-29 D1; DECISIONS.md §"Source operations (D1)").
//!
//! Resolves `<source>` against `plan.yaml.sources.<key>`, runs the
//! shared [`prep`] seam ([`prep::SourceOp::Extract`]) for adapter
//! resolution, brief directory, the four-root sandbox (scratch at
//! `.specify/.cache/extractions/<adapter>/<slice>/scratch/`), and the
//! `evidence/` output target under `.specify/slices/<slice>/`, then
//! branches on the adapter's `execution` mode:
//!
//! - `tool`: single-phase. Probe the extraction cache; on a hit read
//!   the cached `evidence.yaml`, on a miss dispatch the declared tool
//!   (an M1 seam — no first-party source declares an `extract` tool
//!   yet). Either way validate the Evidence and persist it.
//! - `agent`: two-phase (RFC-29 D9; DECISIONS.md §"Adapter execution
//!   mode (D9)"). The CLI never blocks on agent work.
//!   - `--phase prepare` (default): build scratch + the `evidence/`
//!     target, emit `source.execution.agent`, and print the extract
//!     handoff envelope (`{ adapter, version, briefs-dir, source-dir? |
//!     value-inline?, scratch-dir, evidence-dir, leads:[<lead>],
//!     execution }`). For value-bound sources (e.g. `intent`)
//!     `source-dir` is absent and `value-inline` carries the literal
//!     binding body (preflight §2). Control returns to the agent.
//!   - `--phase finalize`: validate the agent-produced Evidence
//!     against `schemas/evidence.schema.json` *before* it becomes
//!     visible to synthesis, persist it to
//!     `.specify/slices/<slice>/evidence/<source>.yaml`, run the
//!     extraction-cache fingerprint (RFC-27, with the `lead` input),
//!     and emit `slice.extract.cache-hit` / `cache-miss`. Under the
//!     `execution: agent` forced opt-out this is always a `cache-miss`
//!     with `reason: adapter-opt-out`. A validation failure returns
//!     early — no Evidence is persisted and the slice stays
//!     `refining`.
//!
//! The agent writes its Evidence to `$SCRATCH_DIR/evidence.yaml` (the
//! write-only sandbox root, mirroring how `survey` writes
//! `lead-set.md`); the CLI is the only writer of the visible
//! `.specify/slices/<slice>/evidence/<source>.yaml`, so an invalid
//! document never lands on the persisted path.

use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_error::{Error, Result};
use specify_model::atomic::bytes_write;
use specify_workflow::adapter::cache::{self, LookupOutcome};
use specify_workflow::adapter::{CacheLayout, Execution, SourceOperation};
use specify_workflow::change::{Plan, SourceBinding};
use specify_workflow::journal::{self, CacheMissReason, Event, EventKind};
use specify_workflow::schema;

use crate::runtime::commands::source::cli::Phase;
use crate::runtime::commands::source::{op, prep};
use crate::runtime::context::Ctx;

/// Extract handoff envelope printed by the agent `prepare` phase
/// (preflight §2). Carries `evidence-dir` (the slice's `evidence/`
/// target) and exactly one source representation: `source-dir` for
/// path bindings, `value-inline` for value bindings (e.g. `intent`).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExtractHandoff {
    adapter: String,
    version: u32,
    briefs_dir: PathBuf,
    /// `$SOURCE_DIR` — present for path bindings, absent for
    /// value-bound sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_dir: Option<PathBuf>,
    /// Literal `value:` body — present for value-bound sources, absent
    /// for path bindings. The value half of the minimal two-field
    /// source request (preflight §2).
    #[serde(skip_serializing_if = "Option::is_none")]
    value_inline: Option<String>,
    scratch_dir: PathBuf,
    /// `.specify/slices/<slice>/evidence/` — where the CLI persists
    /// the validated `<source>.yaml` in finalize.
    evidence_dir: PathBuf,
    /// The single lead being extracted (preflight §2).
    leads: Vec<String>,
    execution: &'static str,
}

/// Result of a completed extract (tool single-phase, or agent
/// `finalize`): the cache outcome plus the persisted Evidence path.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExtractResult {
    adapter: String,
    source: String,
    slice: String,
    lead: String,
    fingerprint: String,
    /// `hit` | `miss`.
    cache: &'static str,
    /// Populated on a miss; the closed cache-miss reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<CacheMissReason>,
    /// Persisted `.specify/slices/<slice>/evidence/<source>.yaml`.
    evidence: PathBuf,
}

/// Run `specrun source extract <source> <lead> --slice <slice>
/// [--phase prepare|finalize]`.
///
/// # Errors
///
/// - `source-unknown` when `<source>` is not a
///   `plan.yaml.sources` key.
/// - propagates adapter-resolution, schema-validation, fingerprint,
///   and persist failures.
pub fn run(ctx: &Ctx, source: &str, lead: &str, slice: &str, phase: Phase) -> Result<()> {
    let plan = Plan::load(&ctx.layout().plan_path())?;
    let binding = plan.sources.get(source).ok_or_else(|| Error::Diag {
        code: "source-unknown",
        detail: format!(
            "no source `{source}` in plan.yaml.sources; `specrun source extract` resolves \
             its argument against the plan's source keys, not the adapter name"
        ),
    })?;

    let source_path =
        binding.path.as_deref().map(|raw| prep::resolve_source_path(&ctx.project_dir, raw));

    let slice_dir = ctx.slices_dir().join(slice);
    let leads = [lead.to_string()];
    let prepared = prep::prepare(&prep::PrepRequest {
        adapter: &binding.adapter,
        project_dir: &ctx.project_dir,
        op: prep::SourceOp::Extract {
            slice: slice.to_string(),
        },
        source: source_path.as_deref(),
        leads: &leads,
        evidence_root: Some(&slice_dir),
    })?;

    let cx = ExtractCtx {
        ctx,
        source,
        lead,
        slice,
        prepared: &prepared,
        source_path: source_path.as_deref(),
        binding,
    };

    match prepared.manifest.execution {
        Some(Execution::Tool) => run_tool(&cx),
        _ => match phase {
            Phase::Prepare => prepare(&cx),
            Phase::Finalize => finalize(&cx),
        },
    }
}

/// Bundle of the per-invocation extract inputs threaded through the
/// phase functions, so each stays under the argument-count budget.
struct ExtractCtx<'a> {
    ctx: &'a Ctx,
    source: &'a str,
    lead: &'a str,
    slice: &'a str,
    prepared: &'a prep::SourcePrep,
    source_path: Option<&'a Path>,
    binding: &'a SourceBinding,
}

/// Agent `prepare` phase: build scratch, emit `source.execution.agent`,
/// and print the extract handoff envelope. The CLI returns control to
/// the agent and does not block.
fn prepare(cx: &ExtractCtx<'_>) -> Result<()> {
    let scratch = op::scratch_path(cx.prepared, SourceOperation::Extract)?;
    std::fs::create_dir_all(&scratch).map_err(Error::Io)?;

    let event = Event::new(
        Timestamp::now(),
        EventKind::SourceExecutionAgent {
            source: cx.source.to_string(),
            adapter: cx.prepared.manifest.name.clone(),
            operation: SourceOperation::Extract,
        },
    );
    journal::append_batch(cx.ctx.layout(), std::slice::from_ref(&event))?;

    let (source_dir, value_inline) = cx
        .source_path
        .map_or_else(|| (None, cx.binding.value.clone()), |path| (Some(path.to_path_buf()), None));
    let handoff = ExtractHandoff {
        adapter: cx.prepared.manifest.name.clone(),
        version: cx.prepared.manifest.version,
        briefs_dir: cx.prepared.briefs_dir.clone(),
        source_dir,
        value_inline,
        scratch_dir: scratch,
        evidence_dir: evidence_dir(cx.prepared),
        leads: vec![cx.lead.to_string()],
        execution: "agent",
    };
    cx.ctx.write(&handoff, write_handoff_text)
}

/// Agent `finalize` phase: validate the agent-produced Evidence, persist
/// it, then record the cache outcome.
fn finalize(cx: &ExtractCtx<'_>) -> Result<()> {
    let scratch = op::scratch_path(cx.prepared, SourceOperation::Extract)?;
    let staged = scratch.join(SourceOperation::Extract.artifact_name());
    let raw = op::read_artifact(&staged, SourceOperation::Extract)?;
    complete(cx, &raw, &staged)
}

/// Single-phase `tool` execution: probe the cache, produce the Evidence
/// (cached hit or freshly dispatched), validate, and persist.
fn run_tool(cx: &ExtractCtx<'_>) -> Result<()> {
    let scratch = op::scratch_path(cx.prepared, SourceOperation::Extract)?;
    std::fs::create_dir_all(&scratch).map_err(Error::Io)?;

    let fingerprint = extract_fingerprint(cx)?;
    let layout = CacheLayout::new(&cx.ctx.project_dir, &cx.prepared.manifest.name);
    let cache_mode = cx.prepared.manifest.effective_cache_mode();
    let lookup = cache::lookup(
        layout,
        &fingerprint,
        cache_mode,
        cx.slice,
        cx.source,
        SourceOperation::Extract,
    )?;

    let artifact = SourceOperation::Extract.artifact_name();
    let (raw, source) = match &lookup.outcome {
        LookupOutcome::Hit { cache_dir } => {
            let path = cache_dir.join(artifact);
            (op::read_artifact(&path, SourceOperation::Extract)?, path)
        }
        LookupOutcome::Miss { .. } => {
            dispatch_extract_tool(cx.prepared)?;
            let path = scratch.join(artifact);
            (op::read_artifact(&path, SourceOperation::Extract)?, path)
        }
    };

    schema::validate_evidence(&raw, &source)?;
    persist(cx, raw.as_bytes())?;
    emit_cache_event(cx, &lookup)?;
    if matches!(lookup.outcome, LookupOutcome::Miss { .. }) {
        write_cache_entry(cx, layout, &fingerprint, raw.as_bytes(), cache_mode)?;
    }
    cx.ctx.write(&extract_result(cx, &lookup), write_result_text)
}

/// Shared finalize tail: fingerprint + lookup, validate-before-visible,
/// persist, then the cache event + entry. The lookup itself has no side
/// effects, so a missing source path aborts before any Evidence write.
fn complete(cx: &ExtractCtx<'_>, raw: &str, source: &Path) -> Result<()> {
    let fingerprint = extract_fingerprint(cx)?;
    let layout = CacheLayout::new(&cx.ctx.project_dir, &cx.prepared.manifest.name);
    let cache_mode = cx.prepared.manifest.effective_cache_mode();
    let lookup = cache::lookup(
        layout,
        &fingerprint,
        cache_mode,
        cx.slice,
        cx.source,
        SourceOperation::Extract,
    )?;

    // Validate-before-visible: a schema failure returns here, before the
    // Evidence is persisted and before any cache event is emitted.
    schema::validate_evidence(raw, source)?;

    persist(cx, raw.as_bytes())?;
    emit_cache_event(cx, &lookup)?;
    write_cache_entry(cx, layout, &fingerprint, raw.as_bytes(), cache_mode)?;
    cx.ctx.write(&extract_result(cx, &lookup), write_result_text)
}

/// Dispatch the declared `extract` WASI tool / built-in Rust path.
///
/// M1 ships no first-party extract tool; the WASI extract dispatch
/// protocol is out of scope for RFC-29 M1. The control flow above is
/// wired correctly (cache probe, Evidence read, validate-before-visible
/// persist) so the only seam left is the actual tool invocation.
fn dispatch_extract_tool(prepared: &prep::SourcePrep) -> Result<()> {
    Err(Error::Diag {
        code: "source-extract-tool-unsupported",
        detail: format!(
            "source adapter `{}` declares `execution: tool`, but M1 ships no `extract` tool \
             dispatch; no first-party source declares an extract tool",
            prepared.manifest.name
        ),
    })
}

/// Persist the validated Evidence to
/// `.specify/slices/<slice>/evidence/<source>.yaml` (atomic
/// tempfile-rename). The directory was scaffolded by [`prep::prepare`].
fn persist(cx: &ExtractCtx<'_>, bytes: &[u8]) -> Result<()> {
    let path = evidence_dir(cx.prepared).join(format!("{}.yaml", cx.source));
    bytes_write(&path, bytes)
}

/// Build the closed extract [`CacheFingerprint`] (RFC-27, with `lead`)
/// via the shared [`op::build_fingerprint`] kernel.
fn extract_fingerprint(cx: &ExtractCtx<'_>) -> Result<cache::CacheFingerprint> {
    op::build_fingerprint(
        cx.prepared,
        cx.source_path,
        cx.binding,
        SourceOperation::Extract,
        Some(cx.lead.to_string()),
    )
}

/// Emit the `slice.extract.cache-hit` / `cache-miss` journal event for
/// `lookup` (the existing RFC-27 extract cache events).
fn emit_cache_event(cx: &ExtractCtx<'_>, lookup: &cache::CacheLookup) -> Result<()> {
    let adapter = cx.prepared.manifest.name.clone();
    let kind = match &lookup.outcome {
        LookupOutcome::Hit { .. } => EventKind::SliceExtractCacheHit {
            slice_name: cx.slice.into(),
            source: cx.source.to_string(),
            adapter,
            fingerprint: lookup.digest.clone(),
        },
        LookupOutcome::Miss { reason } => EventKind::SliceExtractCacheMiss {
            slice_name: cx.slice.into(),
            source: cx.source.to_string(),
            adapter,
            fingerprint: lookup.digest.clone(),
            reason: *reason,
        },
    };
    let event = Event::new(Timestamp::now(), kind);
    journal::append_batch(cx.ctx.layout(), std::slice::from_ref(&event))
}

/// Write the cache artifact + `fingerprint.json` + index row via the
/// shared [`op::write_cache_entry`] kernel.
fn write_cache_entry(
    cx: &ExtractCtx<'_>, layout: CacheLayout<'_>, fingerprint: &cache::CacheFingerprint,
    artifact_bytes: &[u8], cache_mode: Option<specify_workflow::adapter::CacheMode>,
) -> Result<()> {
    op::write_cache_entry(
        &op::CacheEntry {
            layout,
            cache_mode,
            slice_lane: cx.slice,
            source: cx.source,
            adapter: &cx.prepared.manifest.name,
            op: SourceOperation::Extract,
        },
        fingerprint,
        artifact_bytes,
    )
}

fn extract_result(cx: &ExtractCtx<'_>, lookup: &cache::CacheLookup) -> ExtractResult {
    let (cache, reason) = match &lookup.outcome {
        LookupOutcome::Hit { .. } => ("hit", None),
        LookupOutcome::Miss { reason } => ("miss", Some(*reason)),
    };
    ExtractResult {
        adapter: cx.prepared.manifest.name.clone(),
        source: cx.source.to_string(),
        slice: cx.slice.to_string(),
        lead: cx.lead.to_string(),
        fingerprint: lookup.digest.clone(),
        cache,
        reason,
        evidence: evidence_dir(cx.prepared).join(format!("{}.yaml", cx.source)),
    }
}

/// The scaffolded `.specify/slices/<slice>/evidence/` directory. Always
/// `Some` for the extract op (prep was handed `evidence_root: Some`);
/// the `expect` pins that invariant.
fn evidence_dir(prepared: &prep::SourcePrep) -> PathBuf {
    prepared
        .evidence_dir
        .clone()
        .expect("extract prep always scaffolds the slice evidence/ directory")
}

fn write_handoff_text(w: &mut dyn Write, body: &ExtractHandoff) -> std::io::Result<()> {
    writeln!(w, "adapter: {} v{}", body.adapter, body.version)?;
    writeln!(w, "execution: {}", body.execution)?;
    writeln!(w, "briefs-dir: {}", body.briefs_dir.display())?;
    if let Some(source_dir) = &body.source_dir {
        writeln!(w, "source-dir: {}", source_dir.display())?;
    }
    if let Some(value_inline) = &body.value_inline {
        writeln!(w, "value-inline: {value_inline}")?;
    }
    writeln!(w, "scratch-dir: {}", body.scratch_dir.display())?;
    writeln!(w, "evidence-dir: {}", body.evidence_dir.display())?;
    writeln!(w, "leads: {}", body.leads.join(", "))?;
    Ok(())
}

fn write_result_text(w: &mut dyn Write, body: &ExtractResult) -> std::io::Result<()> {
    writeln!(w, "adapter: {}", body.adapter)?;
    writeln!(w, "source: {}", body.source)?;
    writeln!(w, "slice: {}", body.slice)?;
    writeln!(w, "lead: {}", body.lead)?;
    write!(w, "cache: {}", body.cache)?;
    if let Some(reason) = body.reason {
        write!(w, " ({reason})")?;
    }
    writeln!(w)?;
    writeln!(w, "fingerprint: {}", body.fingerprint)?;
    writeln!(w, "evidence: {}", body.evidence.display())?;
    Ok(())
}
