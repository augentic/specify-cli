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

use serde::Serialize;
use specify_error::{Error, Result};
use specify_model::atomic::bytes_write;
use specify_workflow::adapter::SourceOperation;
use specify_workflow::adapter::cache::{self, LookupOutcome};
use specify_workflow::change::Plan;
use specify_workflow::journal::{CacheMissReason, EventKind};
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

    let flow = ExtractFlow {
        common: op::Common {
            ctx,
            source,
            prepared: &prepared,
            source_path: source_path.as_deref(),
            binding,
            operation: SourceOperation::Extract,
            slice_lane: slice,
            lead: Some(lead),
        },
        lead,
        slice,
    };
    op::run(&flow, phase)
}

/// Extract's operation-specific seam onto the shared [`op::run`] flow:
/// the handoff carries `evidence-dir` (and `source-dir` xor
/// `value-inline`), the commit validates then persists the Evidence,
/// and the cache event is `slice.extract.cache-*`.
struct ExtractFlow<'a> {
    common: op::Common<'a>,
    lead: &'a str,
    slice: &'a str,
}

impl<'a> op::Flow<'a> for ExtractFlow<'a> {
    type Handoff = ExtractHandoff;
    type Outcome = ExtractResult;

    fn common(&self) -> &op::Common<'a> {
        &self.common
    }

    fn handoff(&self, scratch: PathBuf) -> Result<ExtractHandoff> {
        let c = &self.common;
        let (source_dir, value_inline) = c.source_path.map_or_else(
            || (None, c.binding.value.clone()),
            |path| (Some(path.to_path_buf()), None),
        );
        Ok(ExtractHandoff {
            adapter: c.prepared.manifest.name.clone(),
            version: c.prepared.manifest.version,
            briefs_dir: c.prepared.briefs_dir.clone(),
            source_dir,
            value_inline,
            scratch_dir: scratch,
            evidence_dir: evidence_dir(c.prepared)?,
            leads: vec![self.lead.to_string()],
            execution: "agent",
        })
    }

    fn write_handoff(w: &mut dyn Write, body: &ExtractHandoff) -> std::io::Result<()> {
        write_handoff_text(w, body)
    }

    fn commit(
        &self, raw: &str, artifact_source: &Path, lookup: &cache::CacheLookup,
    ) -> Result<ExtractResult> {
        let c = &self.common;
        schema::validate_evidence(raw, artifact_source)?;
        let path = evidence_dir(c.prepared)?.join(format!("{}.yaml", c.source));
        bytes_write(&path, raw.as_bytes())?;
        Ok(self.extract_result(lookup, path))
    }

    fn write_outcome(w: &mut dyn Write, body: &ExtractResult) -> std::io::Result<()> {
        write_result_text(w, body)
    }

    fn cache_event(&self, lookup: &cache::CacheLookup) -> EventKind {
        let c = &self.common;
        let adapter = c.prepared.manifest.name.clone();
        match &lookup.outcome {
            LookupOutcome::Hit { .. } => EventKind::SliceExtractCacheHit {
                slice_name: self.slice.into(),
                source: c.source.to_string(),
                adapter,
                fingerprint: lookup.digest.clone(),
            },
            LookupOutcome::Miss { reason } => EventKind::SliceExtractCacheMiss {
                slice_name: self.slice.into(),
                source: c.source.to_string(),
                adapter,
                fingerprint: lookup.digest.clone(),
                reason: *reason,
            },
        }
    }

    /// M1 ships no first-party extract tool; the WASI extract dispatch
    /// protocol is out of scope for RFC-29 M1. The shared flow is wired
    /// correctly (cache probe, Evidence read, validate-before-visible
    /// persist) so the only seam left is the actual tool invocation.
    fn dispatch_tool(&self) -> Result<()> {
        Err(Error::Diag {
            code: "source-extract-tool-unsupported",
            detail: format!(
                "source adapter `{}` declares `execution: tool`, but M1 ships no `extract` tool \
                 dispatch; no first-party source declares an extract tool",
                self.common.prepared.manifest.name
            ),
        })
    }
}

impl ExtractFlow<'_> {
    fn extract_result(&self, lookup: &cache::CacheLookup, evidence: PathBuf) -> ExtractResult {
        let c = &self.common;
        let (cache, reason) = match &lookup.outcome {
            LookupOutcome::Hit { .. } => ("hit", None),
            LookupOutcome::Miss { reason } => ("miss", Some(*reason)),
        };
        ExtractResult {
            adapter: c.prepared.manifest.name.clone(),
            source: c.source.to_string(),
            slice: self.slice.to_string(),
            lead: self.lead.to_string(),
            fingerprint: lookup.digest.clone(),
            cache,
            reason,
            evidence,
        }
    }
}

/// The scaffolded `.specify/slices/<slice>/evidence/` directory. Always
/// `Some` for the extract op (prep was handed `evidence_root: Some`).
/// Fails closed with a diagnostic rather than panicking if that prep
/// invariant ever regresses.
fn evidence_dir(prepared: &prep::SourcePrep) -> Result<PathBuf> {
    let Some(dir) = prepared.evidence_dir.clone() else {
        return Err(Error::Diag {
            code: "source-extract-prep-missing",
            detail: "extract prep did not scaffold the slice evidence/ directory \
                (evidence_root was None)"
                .to_string(),
        });
    };
    Ok(dir)
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
