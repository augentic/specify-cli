//! Workflow journal events.
//!
//! Append-only newline-delimited JSON at `.specify/journal.jsonl`,
//! shared by every plan-, slice-, propose-, extract-, and synthesis-
//! related signal listed in [workflow §Observability]. One line per
//! [`Event`]; readers tail the file and skip blank lines.
//!
//! Wire format is locked: event ids are dotted kebab-case
//! (`plan.transition.approved`), payload field names are kebab-case
//! (`plan-name`, `slice-name`, …), and the closed `from` / `to`
//! enum is `none | likely | accepted | rejected`. Rust variant
//! names stay `snake_case` and reach the wire through
//! `#[serde(rename = "…")]`.
//!
//! [workflow §Observability]: ../../../../docs/standards/workflow.md#observability

use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use specify_diagnostics::{Diagnostic, FindingStatus, count_status};
use specify_error::Error;

use crate::adapter::operation::SourceOperation;
use crate::change::Divergence;
use crate::config::Layout;

/// Project-relative path the journal lives at.
const JOURNAL_FILE_NAME: &str = "journal.jsonl";

/// One row of the journal. Serialises as `{ timestamp, event,
/// payload }` — workflow §Wire format pins `timestamp` first so a
/// `head -1` on the file is enough to confirm the run window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Second-precision UTC timestamp (`%Y-%m-%dT%H:%M:%SZ`).
    #[serde(with = "specify_error::serde_rfc3339")]
    pub timestamp: Timestamp,
    /// Event id + payload, adjacently tagged so `event` and `payload`
    /// sit side by side in the JSON object.
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    /// Build an [`Event`] at `timestamp` carrying `kind`. Tests pin
    /// the timestamp; production callers pass `Timestamp::now()`.
    #[must_use]
    pub const fn new(timestamp: Timestamp, kind: EventKind) -> Self {
        Self { timestamp, kind }
    }
}

/// The workflow §Observability event set.
///
/// Adjacently-tagged on the wire as `{ event: <id>, payload: {…} }`
/// so the dotted-kebab-case event id is a top-level field consumers
/// can filter on without parsing the payload first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload")]
pub enum EventKind {
    /// Gate 1 cleared — `specrun plan transition <plan-name> approved`.
    #[serde(rename = "plan.transition.approved", rename_all = "kebab-case")]
    PlanTransitionApproved {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
    },
    /// Operator walked one rung backwards on per-entry status via
    /// `specrun plan transition <entry> --undo`. One event per rung
    /// (`done → in-progress` and `in-progress → pending` each fire
    /// individually) so the journal records every step the operator
    /// took and replay traces line up with the forward-direction
    /// `plan.transition.approved` / `slice.transition.*` cadence.
    #[serde(rename = "plan.transition.undone", rename_all = "kebab-case")]
    PlanTransitionUndone {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Entry id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Status the entry held before the undo.
        from: crate::change::Status,
        /// Status the entry holds after the undo.
        to: crate::change::Status,
    },
    /// Stamped `slices[].divergence` via
    /// `specrun plan amend --divergence <likely|accepted|rejected>`.
    /// divergence and writer-ownership contract — the CLI is the single writer. In the
    /// RFC-29 D2 propose flow the `/spec:plan` agent stages `likely`
    /// through this event after `propose --from`; the operator later
    /// flips `accepted` / `rejected` the same way. This is the only
    /// path that writes the `divergence` field.
    #[serde(rename = "plan.amend.divergence", rename_all = "kebab-case")]
    PlanAmendDivergence {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Previous value — may be any of `none | likely | accepted | rejected`.
        /// Callers convert an absent on-disk slice field via
        /// `previous.unwrap_or(Divergence::None)`.
        from: Divergence,
        /// New value — `likely`, `accepted`, or `rejected`. The
        /// implicit `none` default is rejected at the flag-parser
        /// level; omit `--divergence` to leave the field unchanged.
        to: Divergence,
    },
    /// Slice transitioned to `refined` — synthesis finished and the
    /// slice is ready for `/spec:build`.
    #[serde(rename = "slice.transition.refined", rename_all = "kebab-case")]
    SliceTransitionRefined {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// `/spec:refine` finished one source-bound `extract` call. One
    /// event per `(source, slice)` pair. Agent-driven.
    #[serde(rename = "slice.extract.completed", rename_all = "kebab-case")]
    SliceExtractCompleted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Source key from `plan.yaml.sources.<key>`.
        source: String,
    },
    /// `[conflict]` on a requirement in `spec.md` — same-authority
    /// disagreement the operator must reconcile. Emitted by
    /// `specrun slice validate` after a successful run.
    #[serde(rename = "slice.synthesis.conflict", rename_all = "kebab-case")]
    SliceSynthesisConflict {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// `[divergence]` on a requirement in `spec.md` — cross-authority
    /// disagreement preserved as inline commentary. Emitted by
    /// `specrun slice validate` after a successful run.
    #[serde(rename = "slice.synthesis.divergence", rename_all = "kebab-case")]
    SliceSynthesisDivergence {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// `[unknown]` on a requirement in `spec.md` — a gap the operator
    /// must close before the requirement is meaningful. Emitted by
    /// `specrun slice validate` after a successful run.
    #[serde(rename = "slice.synthesis.unknown", rename_all = "kebab-case")]
    SliceSynthesisUnknown {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// Slice synthesis began — `/spec:refine` started folding the
    /// extracted evidence into `proposal.md` / `spec.md` / `design.md`
    /// / `tasks.md` / `model.yaml`. One event per slice (RFC-29c
    /// §"Wire contracts"). Distinct from the per-requirement
    /// `slice.synthesis.*` tag events above — `synthesize` is the
    /// lifecycle verb, `synthesis` is the requirement-tag noun.
    #[serde(rename = "slice.synthesize.started", rename_all = "kebab-case")]
    SliceSynthesizeStarted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// Synthesis dispatched to the agent. Synthesis is always
    /// agent-driven and `cache: opt-out` (RFC-29c §"Synthesis dispatch
    /// (D10)"); this signal fires on the dry-run inputs phase so the
    /// journal records that no cache short-circuit was attempted.
    #[serde(rename = "slice.synthesize.agent", rename_all = "kebab-case")]
    SliceSynthesizeAgent {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// Slice synthesis finished and the artifacts were persisted
    /// (RFC-29c §"Wire contracts"). `artifacts` lists the persisted
    /// relative paths (`proposal.md`, `specs/<unit>/spec.md`,
    /// `design.md`, `tasks.md`, `model.yaml`).
    #[serde(rename = "slice.synthesize.completed", rename_all = "kebab-case")]
    SliceSynthesizeCompleted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Persisted artifact relative paths, in write order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        artifacts: Vec<String>,
    },
    /// Slice synthesis failed before all artifacts were persisted
    /// (RFC-29c §"Wire contracts"). `reason` carries a short human
    /// reason or finding code so the journal records why the slice
    /// stalled.
    #[serde(rename = "slice.synthesize.failed", rename_all = "kebab-case")]
    SliceSynthesizeFailed {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Short human reason / finding code for the failure.
        reason: String,
    },
    /// `/spec:build` started implementing the slice — the target
    /// adapter's `build` brief began running against the refined
    /// artifacts (RFC-29d §"Journal events"). One event per slice.
    #[serde(rename = "slice.build.started", rename_all = "kebab-case")]
    SliceBuildStarted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// `/spec:build` finished implementing the slice — the target
    /// adapter's `build` brief completed and the slice is ready for
    /// `/spec:merge` (RFC-29d §"Journal events"). One event per slice.
    #[serde(rename = "slice.build.succeeded", rename_all = "kebab-case")]
    SliceBuildSucceeded {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// `/spec:build` stopped before the slice was implemented
    /// (RFC-29d §"Journal events"). `reason` carries a short human
    /// reason or finding code so the journal records why the build
    /// stalled.
    #[serde(rename = "slice.build.failed", rename_all = "kebab-case")]
    SliceBuildFailed {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Short human reason / finding code for the failure.
        reason: String,
    },
    /// `specrun slice merge` began folding the slice's deltas into the
    /// baseline (RFC-29d §"Journal events"). The `slice.merge.*` pair
    /// fires on the `specrun slice merge` validator outcome, not on a
    /// merge report. One event per slice.
    #[serde(rename = "slice.merge.started", rename_all = "kebab-case")]
    SliceMergeStarted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// `specrun slice merge` validated and applied the slice's deltas
    /// to the baseline (RFC-29d §"Journal events"). Fires on the
    /// validator outcome, not on a merge report. One event per slice.
    #[serde(rename = "slice.merge.succeeded", rename_all = "kebab-case")]
    SliceMergeSucceeded {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// `specrun slice merge` refused to fold the slice into the
    /// baseline (RFC-29d §"Journal events"). Fires on the validator
    /// outcome, not on a merge report. `reason` carries a short human
    /// reason or finding code so the journal records why the merge
    /// stalled.
    #[serde(rename = "slice.merge.failed", rename_all = "kebab-case")]
    SliceMergeFailed {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Short human reason / finding code for the failure.
        reason: String,
    },
    /// extraction cache fingerprint contract — cache lookup matched and `extract` was *not*
    /// re-run. CI pinning the five fingerprint inputs at a known set
    /// can re-run any prior `/spec:execute` and expect byte-stable
    /// cache hits.
    #[serde(rename = "slice.extract.cache-hit", rename_all = "kebab-case")]
    SliceExtractCacheHit {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Source key from `plan.yaml.sources.<key>`.
        source: String,
        /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
        adapter: String,
        /// sha256 hex digest of the [`crate::adapter::cache::CacheFingerprint`]
        /// inputs the cache layer keyed against.
        fingerprint: String,
    },
    /// extraction cache fingerprint contract — cache lookup missed and `extract` ran. `reason`
    /// is one of the closed [`CacheMissReason`] values; CI observing
    /// any of them knows exactly which input drifted.
    #[serde(rename = "slice.extract.cache-miss", rename_all = "kebab-case")]
    SliceExtractCacheMiss {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Source key from `plan.yaml.sources.<key>`.
        source: String,
        /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
        adapter: String,
        /// sha256 hex digest of the [`crate::adapter::cache::CacheFingerprint`]
        /// inputs the cache layer computed for this run.
        fingerprint: String,
        /// Which fingerprint input drifted (or `no-prior-entry` on
        /// first sight; `adapter-opt-out` when the adapter declared
        /// `cache: opt-out`).
        reason: CacheMissReason,
    },
    /// `survey` cache lookup matched and the operation was *not*
    /// re-run. The plan-time peer of [`Self::SliceExtractCacheHit`];
    /// keyed by the same five-input [`crate::adapter::cache::CacheFingerprint`].
    #[serde(rename = "source.survey.cache-hit", rename_all = "kebab-case")]
    SourceSurveyCacheHit {
        /// Source key from `plan.yaml.sources.<key>`.
        source: String,
        /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
        adapter: String,
        /// sha256 hex digest of the [`crate::adapter::cache::CacheFingerprint`]
        /// inputs the cache layer keyed against.
        fingerprint: String,
    },
    /// `survey` cache lookup missed and the operation ran. The
    /// plan-time peer of [`Self::SliceExtractCacheMiss`]; `reason` is
    /// one of the closed [`CacheMissReason`] values (`adapter-opt-out`
    /// when the adapter ran under forced opt-out).
    #[serde(rename = "source.survey.cache-miss", rename_all = "kebab-case")]
    SourceSurveyCacheMiss {
        /// Source key from `plan.yaml.sources.<key>`.
        source: String,
        /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
        adapter: String,
        /// sha256 hex digest of the [`crate::adapter::cache::CacheFingerprint`]
        /// inputs the cache layer computed for this run.
        fingerprint: String,
        /// Which fingerprint input drifted (or `no-prior-entry` on
        /// first sight; `adapter-opt-out` under forced opt-out).
        reason: CacheMissReason,
    },
    /// A source adapter ran one operation under agent execution
    /// (`execution: agent`). One event per `(source, operation)`
    /// pair; `operation` is the closed [`SourceOperation`] enum
    /// (`survey | extract`).
    #[serde(rename = "source.execution.agent", rename_all = "kebab-case")]
    SourceExecutionAgent {
        /// Source key from `plan.yaml.sources.<key>`.
        source: String,
        /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
        adapter: String,
        /// Which operation ran (`survey` at plan time, `extract` at
        /// slice time).
        operation: SourceOperation,
    },
    /// A target adapter ran one operation under agent execution. The
    /// `build` verb emits this per agent invocation (RFC-29d
    /// §"Journal events"). Unlike [`Self::SourceExecutionAgent`], which
    /// fans out over the `(source, operation)` pair, the build verb
    /// derives `(slice, target)` from the bound project — `build` is
    /// the only agent-dispatched target operation that emits this event
    /// in v1, so the payload stays minimal at `{ slice, target }`.
    #[serde(rename = "target.execution.agent", rename_all = "kebab-case")]
    TargetExecutionAgent {
        /// Slice id under `plan.yaml.slices[].name`.
        slice: String,
        /// Target name (`omnia`, `vectis`, …) the build dispatched to.
        target: String,
    },
    /// runtime capture claim — target's `build` finished replay.
    /// Payload mirrors the `replay:` block written into the
    /// slice's `.metadata.yaml`. Optional in v1 (targets that have
    /// not implemented the hook do not emit this event).
    #[serde(rename = "slice.replay.completed", rename_all = "kebab-case")]
    SliceReplayCompleted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Replay-runner identity (e.g. `omnia-target@1.4 (cargo nextest)`).
        runner: String,
        /// Number of replay scenarios that passed.
        passed: usize,
        /// Number of replay scenarios that failed.
        failed: usize,
        /// Number of replay scenarios the runner skipped.
        skipped: usize,
    },
    /// per-slice authority override — operator set or cleared a per-slice
    /// `authority-override` map at Gate 1. CLI-driven via
    /// `specrun plan create --authority-override`,
    /// `specrun plan amend --authority-override`, or the matching
    /// `--clear-*` flags.
    #[serde(rename = "plan.amend.authority-override", rename_all = "kebab-case")]
    PlanAmendAuthorityOverride {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Closed action discriminator.
        action: AuthorityOverrideAction,
        /// Claim kind the action touched (the closed-enum key under
        /// `slices[].authority-override`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claim_kind: Option<String>,
        /// Source key the override now points at, when `action` is
        /// [`AuthorityOverrideAction::Set`]; absent on clear actions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    /// `specrun plan propose --from` validated the agent reconciliation
    /// response and wrote `plan.yaml.slices[]`. One event per successful
    /// invocation — the `/spec:plan` skill never calls
    /// `specrun journal emit` for D2. (RFC-29 review F8 folded the former
    /// `plan.reconcile.agent` + `plan.reconcile.completed` pair into this
    /// single event: they always co-fired atomically with no failure-mode
    /// gap between them, so one indivisible event carries the whole D2
    /// outcome.)
    #[serde(rename = "plan.reconcile.completed", rename_all = "kebab-case")]
    PlanReconcileCompleted {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Count of `plan.yaml.slices[]` rows written.
        slice_count: usize,
        /// Slice names, in the agent's `slices[]` response order.
        slice_names: Vec<String>,
    },
    /// A slice merged into the baseline and its working directory was
    /// archived. This is the durable **outcome-ledger** entry
    /// (decision-log §"History via git plus an outcome ledger"): the
    /// append-only journal records what merged, when, which baseline
    /// specs it touched, a one-line outcome summary, and the git SHA
    /// the baseline sat at. The archived slice folder under
    /// `.specify/archive/` is a prunable convenience cache
    /// (`specrun archive prune`), not the system of record — this
    /// event plus git history of `.specify/specs/` is.
    #[serde(rename = "slice.archive.created", rename_all = "kebab-case")]
    SliceArchiveCreated {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Baseline spec/composition names this slice merged into, in
        /// the merge engine's `(class, name)` order.
        touched_specs: Vec<String>,
        /// One-line human summary of the merge operations (the same
        /// text stamped into the archived slice's `.metadata.yaml`
        /// merge outcome).
        outcome_summary: String,
        /// Git HEAD SHA after the merge, when the project is a git
        /// repository; absent otherwise (best-effort, never fatal).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        merge_sha: Option<String>,
        /// `DEC-NNNN` ids promoted into the Decision Record catalogue by
        /// this merge (RFC-36), in slug order. Empty stays off the wire;
        /// this is the durable ledger of promoted decisions alongside git
        /// history of `.specify/decisions/`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decisions: Vec<String>,
    },
    /// `specrun lint` finished a scan. The payload carries the scan
    /// scope, wall-clock duration, per-status counts, a
    /// `baseline_present` flag (hard-coded `false` until RFC-33b
    /// lands), and the CLI exit code the scan resolved to. Emission is
    /// wired in the scanner; this variant exists so the taxonomy is
    /// closed even before the emitter ships.
    ///
    /// Field names on the wire are `snake_case` to match the journal
    /// payload example verbatim (`duration_ms`, `baseline_present`,
    /// `false_positive`, `exit_code`); this is the one variant in the
    /// taxonomy that does not project through `rename_all =
    /// "kebab-case"`, because that payload shape is the wire contract
    /// consumers will read.
    #[serde(rename = "lint-completed")]
    LintCompleted(LintCompletedPayload),
}

/// Payload for [`EventKind::LintCompleted`]. The journal event
/// contract pins the field set and the `snake_case` wire names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintCompletedPayload {
    /// Scope of the scan — which target, slice, or artifact the run
    /// was narrowed to. All three sub-fields are optional; a
    /// project-wide scan leaves them `null`.
    pub scope: LintScope,
    /// Wall-clock duration of the scan in milliseconds.
    pub duration_ms: u64,
    /// Per-status counts. While RFC-33b is deferred, the scanner emits
    /// only `open`, `ignored`, and `false_positive`; the additional
    /// `new` / `baselined` buckets land additively with RFC-33b.
    pub counts: LintCounts,
    /// Whether the scan observed a baseline file. Hard-coded `false`
    /// in current emitters; becomes scan-derived when RFC-33b lands.
    pub baseline_present: bool,
    /// CLI exit code the scan resolved to (status-aware severity per
    /// the exit and presentation semantics). `0` on clean
    /// scans, `2` when an `open` finding of `important` or `critical`
    /// severity remains.
    pub exit_code: i32,
}

/// Scan-scope sub-object on [`LintCompletedPayload`]. Each field is
/// optional and serialised as `null` when absent so the wire shape
/// matches the payload example verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintScope {
    /// Target name (`omnia`, `vectis`, …) when the scan was narrowed
    /// to a single target; `None` for project-wide scans.
    pub target: Option<String>,
    /// Slice id from `plan.yaml.slices[].name` when the scan was
    /// narrowed to one slice (e.g. `specrun lint run --slice <name>`).
    pub slice: Option<String>,
    /// Artifact path (relative to project root) when the scan was
    /// narrowed to a single artifact; `None` otherwise.
    pub artifact: Option<String>,
}

/// Per-status finding counts on [`LintCompletedPayload`]. Current
/// emitters fill the three buckets named here; RFC-33b adds `new` and
/// `baselined` additively when it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintCounts {
    /// `status: open` count — findings that block CI by default when
    /// they also carry `severity: critical` or `severity: important`.
    pub open: u32,
    /// `status: ignored` count — findings demoted by a matching
    /// `specify-ignore` directive.
    pub ignored: u32,
    /// `status: false-positive` count — findings demoted by a
    /// `specify-ignore` directive whose rationale begins with
    /// `false-positive:`.
    pub false_positive: u32,
}

/// Closed `reason` enum on [`EventKind::SliceExtractCacheMiss`].
///
/// Each value names one of the five fingerprint inputs from authority and reconciliation contract
/// lint exit mapping (plus `no-prior-entry` for first runs and `adapter-opt-out`
/// for `cache: opt-out` adapters). Operators reading `index.jsonl`
/// see exactly which input drifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CacheMissReason {
    /// No prior cache entry — first run for this fingerprint key.
    NoPriorEntry,
    /// `$SOURCE_DIR` canonical path (or value-binding sha256) changed.
    SourcePathChanged,
    /// `<name>@<version>` from `adapter.yaml` changed.
    AdapterVersionChanged,
    /// sha256 of the brief markdown driving the operation changed.
    BriefShaChanged,
    /// One of the declared-tool versions changed.
    ToolVersionChanged,
    /// The adapter declared `cache: opt-out`; the CLI bypasses the
    /// cache and the matching journal event carries this reason.
    AdapterOptOut,
}

/// Closed `action` enum on [`EventKind::PlanAmendAuthorityOverride`].
///
/// Mirrors the per-kind mutations emitted by the CLI surface
/// (`--authority-override`, `--clear-authority-override`, and the
/// per-kind expansion of `--clear-authority-overrides`).
///
/// Variants are declared in the documented sort order `Set < Clear`
/// so batched `mutate_authority_overrides` callers emit set-then-clear
/// journal events; the `set_sorts_before_clear` test guards drift.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum AuthorityOverrideAction {
    /// `--authority-override <slice> <kind>=<key>` set the value.
    Set,
    /// `--clear-authority-override <slice> <kind>` removed one entry.
    Clear,
}

#[cfg(test)]
mod authority_override_action_tests {
    use super::AuthorityOverrideAction;

    #[test]
    fn set_sorts_before_clear() {
        let mut actions = [AuthorityOverrideAction::Clear, AuthorityOverrideAction::Set];
        actions.sort();
        assert_eq!(
            actions,
            [AuthorityOverrideAction::Set, AuthorityOverrideAction::Clear],
            "Set MUST sort before Clear so batched plan-amend journal writes \
             replay the operator's intent set-then-clear; the wire contract \
             depends on this ordering (see PlanAmendAuthorityOverride)."
        );
    }
}

/// Absolute path to the journal at `<project_dir>/.specify/journal.jsonl`.
#[must_use]
pub fn path(layout: Layout<'_>) -> PathBuf {
    layout.specify_dir().join(JOURNAL_FILE_NAME)
}

/// Read every parseable [`Event`] from the journal at
/// `<project_dir>/.specify/journal.jsonl`, in append (file) order.
///
/// A missing journal yields an empty vector. Blank lines are skipped.
/// Lines that fail to parse as an [`Event`] are skipped rather than
/// failing the whole read, so a journal written by a newer binary
/// (carrying event kinds this binary does not know) still yields the
/// events it does understand — the read stays forward-compatible and,
/// for a given file, deterministic. This is the read side the RFC-36
/// identity projection (`recent[]`) consumes.
///
/// # Errors
///
/// Propagates I/O failures other than a missing file.
pub fn read(layout: Layout<'_>) -> Result<Vec<Event>, Error> {
    let path = path(layout);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::Io(err)),
    };
    Ok(contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Event>(line).ok())
        .collect())
}

/// Append a sequence of [`Event`]s to the project journal in a
/// single write call.
///
/// Opens `<project_dir>/.specify/journal.jsonl` in append mode,
/// creating the file (and the `.specify/` directory) on first
/// write. All events are serialised, concatenated as
/// newline-terminated JSON lines, and pushed through one
/// `write_all` followed by one `sync_all`. Either every line
/// lands on disk or none does — downstream consumers never
/// observe a partial-state batch. A POSIX `O_APPEND` write of
/// ≤ `PIPE_BUF` bytes is atomic against concurrent writers on
/// local filesystems, which is the safety envelope a workflow
/// journal needs — the workflow contract emits one event per CLI verb
/// invocation, well below the limit.
///
/// Used by CLI verbs that own more than one journal emit per
/// invocation (e.g. `specrun plan create --auto-approve
/// --authority-override`, which stages both `plan.transition.approved`
/// and `plan.amend.authority-override` in the same Gate-1 consent), and
/// equally by single-event callers via
/// `append_batch(layout, std::slice::from_ref(&event))`.
///
/// Empty `events` is a no-op; the journal file is not created on
/// disk and `Ok(())` is returned. This lets callers compose the
/// batch unconditionally (collecting events into a `Vec` and
/// passing the slice in) without an outer `is_empty` check.
///
/// # Errors
///
/// Propagates I/O failures from the directory create / open /
/// write / fsync chain, plus JSON serialisation failures as
/// `journal-event-serialise-failed`.
pub fn append_batch(layout: Layout<'_>, events: &[Event]) -> Result<(), Error> {
    if events.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(layout.specify_dir())?;
    let path = path(layout);
    let mut payload = String::new();
    for event in events {
        let line = serde_json::to_string(event).map_err(|err| Error::Diag {
            code: "journal-event-serialise-failed",
            detail: format!("failed to serialise journal event: {err}"),
        })?;
        payload.push_str(&line);
        payload.push('\n');
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(payload.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Append a `lint-completed` event to `<project_dir>/.specify/journal.jsonl`.
///
/// Best-effort: serialise/IO failures log to stderr with the supplied
/// `command_label` prefix and never override the scan's exit code.
pub fn emit_lint_completed(
    layout: Layout<'_>, scope: LintScope, findings: &[Diagnostic], duration_ms: u128,
    exit_code: i32, command_label: &str,
) {
    let counts = LintCounts {
        open: count_status(findings, None),
        ignored: count_status(findings, Some(FindingStatus::Ignored)),
        false_positive: count_status(findings, Some(FindingStatus::FalsePositive)),
    };
    let payload = LintCompletedPayload {
        scope,
        duration_ms: u64::try_from(duration_ms).unwrap_or(u64::MAX),
        counts,
        baseline_present: false,
        exit_code,
    };
    let event = Event::new(Timestamp::now(), EventKind::LintCompleted(payload));
    if let Err(err) = append_batch(layout, std::slice::from_ref(&event)) {
        eprintln!("{command_label}: failed to append lint-completed journal event: {err}");
    }
}

/// Parses a fixed RFC3339 timestamp for test fixtures.
#[cfg(test)]
pub(crate) fn test_timestamp(raw: &str) -> Timestamp {
    raw.parse().expect("valid rfc3339 timestamp in test fixture")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn read_lines(layout: Layout<'_>) -> Vec<String> {
        let raw = std::fs::read_to_string(path(layout)).expect("read journal");
        raw.lines().map(str::to_owned).collect()
    }

    #[test]
    fn append_creates_specify_dir_when_missing() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        assert!(!layout.specify_dir().exists(), "precondition: .specify must not exist yet");

        let event = Event::new(
            test_timestamp("2026-05-21T20:02:00Z"),
            EventKind::SliceTransitionRefined {
                slice_name: "checkout".to_string(),
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");

        assert!(layout.specify_dir().is_dir(), ".specify/ must exist after first append");
        assert!(path(layout).is_file(), "journal.jsonl must exist after first append");
    }

    #[test]
    fn append_batch_writes_in_order() {
        // auto-approve Gate-1 contract: `specrun plan create --auto-approve
        // --authority-override` may emit both `plan.transition.approved`
        // and `plan.amend.authority-override` in a single fsynced append.
        // Exercise the batched helper to lock ordering.
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let events = vec![
            Event::new(
                test_timestamp("2026-05-22T13:30:00Z"),
                EventKind::PlanTransitionApproved {
                    plan_name: "fresh".to_string(),
                },
            ),
            Event::new(
                test_timestamp("2026-05-22T13:30:00Z"),
                EventKind::PlanAmendAuthorityOverride {
                    plan_name: "fresh".to_string(),
                    slice_name: "checkout".to_string(),
                    action: AuthorityOverrideAction::Set,
                    claim_kind: Some("criterion".to_string()),
                    source: Some("runtime".to_string()),
                },
            ),
        ];
        append_batch(layout, &events).expect("append_batch ok");

        let lines = read_lines(layout);
        assert_eq!(lines.len(), 2, "expected two journal lines, got {}", lines.len());
        assert!(
            lines[0].contains(r#""event":"plan.transition.approved""#),
            "first line must be plan.transition.approved, got:\n{}",
            lines[0]
        );
        assert!(
            lines[1].contains(r#""event":"plan.amend.authority-override""#),
            "second line must be plan.amend.authority-override, got:\n{}",
            lines[1]
        );
    }

    #[test]
    fn append_batch_empty_slice_is_no_op() {
        // Callers (e.g. `plan create` without `--auto-approve` and
        // without `--authority-override`) build the batch
        // unconditionally; an empty input must not create the
        // journal file on disk.
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        append_batch(layout, &[]).expect("empty batch ok");
        assert!(
            !path(layout).exists(),
            "empty append_batch must not create journal.jsonl, found: {}",
            path(layout).display()
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Single table pins every payload-bearing variant's wire shape; splitting hides the contract."
    )]
    fn event_wire_shapes_match_contract() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let rows: &[(EventKind, &[&str])] = &[
            (
                EventKind::SliceExtractCacheHit {
                    slice_name: "identity-user-registration".to_string(),
                    source: "runtime".to_string(),
                    adapter: "captures".to_string(),
                    fingerprint: "sha256:cafef00d".to_string(),
                },
                &[
                    r#"{"timestamp":"2026-05-22T13:15:00Z","event":"slice.extract.cache-hit","payload":{"slice-name":"identity-user-registration","source":"runtime","adapter":"captures","fingerprint":"sha256:cafef00d"}}"#,
                ],
            ),
            (
                EventKind::SliceExtractCacheMiss {
                    slice_name: "identity-user-registration".to_string(),
                    source: "runtime".to_string(),
                    adapter: "captures".to_string(),
                    fingerprint: "sha256:beef".to_string(),
                    reason: CacheMissReason::AdapterVersionChanged,
                },
                &[
                    r#""event":"slice.extract.cache-miss""#,
                    r#""reason":"adapter-version-changed""#,
                    r#""source":"runtime""#,
                ],
            ),
            (
                EventKind::SliceReplayCompleted {
                    slice_name: "identity-user-registration".to_string(),
                    runner: "omnia-target@1.4 (cargo nextest)".to_string(),
                    passed: 47,
                    failed: 0,
                    skipped: 0,
                },
                &[
                    r#""event":"slice.replay.completed""#,
                    r#""passed":47"#,
                    r#""failed":0"#,
                    r#""skipped":0"#,
                    r#""runner":"omnia-target@1.4 (cargo nextest)""#,
                ],
            ),
            (
                EventKind::PlanAmendAuthorityOverride {
                    plan_name: "identity-revamp".to_string(),
                    slice_name: "identity-user-registration".to_string(),
                    action: AuthorityOverrideAction::Set,
                    claim_kind: Some("criterion".to_string()),
                    source: Some("runtime".to_string()),
                },
                &[
                    r#""event":"plan.amend.authority-override""#,
                    r#""action":"set""#,
                    r#""claim-kind":"criterion""#,
                    r#""source":"runtime""#,
                ],
            ),
            (
                EventKind::SourceSurveyCacheHit {
                    source: "runtime".to_string(),
                    adapter: "captures".to_string(),
                    fingerprint: "sha256:cafef00d".to_string(),
                },
                &[
                    r#"{"timestamp":"2026-05-22T13:15:00Z","event":"source.survey.cache-hit","payload":{"source":"runtime","adapter":"captures","fingerprint":"sha256:cafef00d"}}"#,
                ],
            ),
            (
                EventKind::SourceSurveyCacheMiss {
                    source: "runtime".to_string(),
                    adapter: "captures".to_string(),
                    fingerprint: "sha256:beef".to_string(),
                    reason: CacheMissReason::AdapterOptOut,
                },
                &[
                    r#""event":"source.survey.cache-miss""#,
                    r#""reason":"adapter-opt-out""#,
                    r#""source":"runtime""#,
                    r#""fingerprint":"sha256:beef""#,
                ],
            ),
            (
                EventKind::SourceExecutionAgent {
                    source: "runtime".to_string(),
                    adapter: "captures".to_string(),
                    operation: SourceOperation::Survey,
                },
                &[
                    r#""event":"source.execution.agent""#,
                    r#""operation":"survey""#,
                    r#""source":"runtime""#,
                    r#""adapter":"captures""#,
                ],
            ),
            (
                EventKind::PlanReconcileCompleted {
                    plan_name: "identity-revamp".to_string(),
                    slice_count: 3,
                    slice_names: vec![
                        "identity-contracts".to_string(),
                        "identity-service".to_string(),
                        "password-reset".to_string(),
                    ],
                },
                &[
                    r#""event":"plan.reconcile.completed""#,
                    r#""plan-name":"identity-revamp""#,
                    r#""slice-count":3"#,
                    r#""slice-names":["identity-contracts","identity-service","password-reset"]"#,
                ],
            ),
            (
                EventKind::SliceArchiveCreated {
                    slice_name: "identity-service".to_string(),
                    touched_specs: vec!["identity".to_string()],
                    outcome_summary: "identity: 2 modified".to_string(),
                    merge_sha: Some("a1b2c3d".to_string()),
                    decisions: Vec::new(),
                },
                &[
                    r#""event":"slice.archive.created""#,
                    r#""slice-name":"identity-service""#,
                    r#""touched-specs":["identity"]"#,
                    r#""outcome-summary":"identity: 2 modified""#,
                    r#""merge-sha":"a1b2c3d""#,
                ],
            ),
        ];

        for (kind, required) in rows {
            let event = Event::new(test_timestamp("2026-05-22T13:15:00Z"), kind.clone());
            append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
            let line = read_lines(layout).pop().expect("at least one line");
            if required.len() == 1 && required[0].starts_with('{') {
                assert_eq!(line, required[0]);
            } else {
                for needle in *required {
                    assert!(line.contains(needle), "line must contain `{needle}`, got:\n{line}");
                }
            }
        }
    }

    #[test]
    fn cache_miss_reason_round_trips() {
        for (variant, wire) in [
            (CacheMissReason::NoPriorEntry, "no-prior-entry"),
            (CacheMissReason::SourcePathChanged, "source-path-changed"),
            (CacheMissReason::AdapterVersionChanged, "adapter-version-changed"),
            (CacheMissReason::BriefShaChanged, "brief-sha-changed"),
            (CacheMissReason::ToolVersionChanged, "tool-version-changed"),
            (CacheMissReason::AdapterOptOut, "adapter-opt-out"),
        ] {
            assert_eq!(serde_json::to_string(&variant).expect("serialise"), format!("\"{wire}\""));
        }
    }

    #[test]
    fn plan_reconcile_event_round_trips() {
        // `specrun plan propose --from` emits one `plan.reconcile.completed`
        // event (RFC-29 review F8 folded the former agent/completed pair
        // into this single indivisible event); lock its wire shape.
        let completed = Event::new(
            test_timestamp("2026-05-22T13:15:00Z"),
            EventKind::PlanReconcileCompleted {
                plan_name: "identity-revamp".to_string(),
                slice_count: 3,
                slice_names: vec![
                    "identity-contracts".to_string(),
                    "identity-service".to_string(),
                    "password-reset".to_string(),
                ],
            },
        );
        let completed_json = serde_json::to_string(&completed).expect("serialise completed");
        for needle in [
            r#""event":"plan.reconcile.completed""#,
            r#""plan-name":"identity-revamp""#,
            r#""slice-count":3"#,
            r#""slice-names":["identity-contracts","identity-service","password-reset"]"#,
        ] {
            assert!(
                completed_json.contains(needle),
                "completed wire form must contain `{needle}`; got:\n{completed_json}"
            );
        }
        let completed_round: Event =
            serde_json::from_str(&completed_json).expect("deserialise completed");
        assert_eq!(completed_round, completed, "completed round-trip must preserve every field");
    }

    #[test]
    fn slice_synthesize_events_round_trip() {
        // RFC-29c §"Wire contracts": the four M2b lifecycle events
        // serialise to their dotted-kebab ids with kebab-case payload
        // fields, and round-trip back preserving every field. Distinct
        // from the per-requirement `slice.synthesis.*` tag events.
        let rows: &[(EventKind, &[&str])] = &[
            (
                EventKind::SliceSynthesizeStarted {
                    slice_name: "identity-user-registration".to_string(),
                },
                &[
                    r#""event":"slice.synthesize.started""#,
                    r#""slice-name":"identity-user-registration""#,
                ],
            ),
            (
                EventKind::SliceSynthesizeAgent {
                    slice_name: "identity-user-registration".to_string(),
                },
                &[
                    r#""event":"slice.synthesize.agent""#,
                    r#""slice-name":"identity-user-registration""#,
                ],
            ),
            (
                EventKind::SliceSynthesizeCompleted {
                    slice_name: "identity-user-registration".to_string(),
                    artifacts: vec![
                        "proposal.md".to_string(),
                        "specs/identity/spec.md".to_string(),
                        "design.md".to_string(),
                        "tasks.md".to_string(),
                        "model.yaml".to_string(),
                    ],
                },
                &[
                    r#""event":"slice.synthesize.completed""#,
                    r#""slice-name":"identity-user-registration""#,
                    r#""artifacts":["proposal.md","specs/identity/spec.md","design.md","tasks.md","model.yaml"]"#,
                ],
            ),
            (
                EventKind::SliceSynthesizeFailed {
                    slice_name: "identity-user-registration".to_string(),
                    reason: "spec-requirement-missing-sources".to_string(),
                },
                &[
                    r#""event":"slice.synthesize.failed""#,
                    r#""slice-name":"identity-user-registration""#,
                    r#""reason":"spec-requirement-missing-sources""#,
                ],
            ),
        ];

        for (kind, required) in rows {
            let event = Event::new(test_timestamp("2026-05-22T13:15:00Z"), kind.clone());
            let json = serde_json::to_string(&event).expect("serialise synthesize event");
            for needle in *required {
                assert!(json.contains(needle), "wire form must contain `{needle}`; got:\n{json}");
            }
            let round: Event = serde_json::from_str(&json).expect("deserialise synthesize event");
            assert_eq!(round, event, "synthesize round-trip must preserve every field");
        }
    }

    #[test]
    fn slice_build_merge_events_round_trip() {
        // RFC-29d §"Journal events": the M3 build/merge lifecycle
        // events and `target.execution.agent` serialise to their
        // dotted-kebab ids with kebab-case payload fields, and
        // round-trip back preserving every field. The `*.failed`
        // variants carry a `reason`; `target.execution.agent` carries
        // the minimal `{ slice, target }` derived at build time.
        let rows: &[(EventKind, &[&str])] = &[
            (
                EventKind::SliceBuildStarted {
                    slice_name: "identity-user-registration".to_string(),
                },
                &[
                    r#""event":"slice.build.started""#,
                    r#""slice-name":"identity-user-registration""#,
                ],
            ),
            (
                EventKind::SliceBuildSucceeded {
                    slice_name: "identity-user-registration".to_string(),
                },
                &[
                    r#""event":"slice.build.succeeded""#,
                    r#""slice-name":"identity-user-registration""#,
                ],
            ),
            (
                EventKind::SliceBuildFailed {
                    slice_name: "identity-user-registration".to_string(),
                    reason: "cargo-check-failed".to_string(),
                },
                &[
                    r#""event":"slice.build.failed""#,
                    r#""slice-name":"identity-user-registration""#,
                    r#""reason":"cargo-check-failed""#,
                ],
            ),
            (
                EventKind::SliceMergeStarted {
                    slice_name: "identity-user-registration".to_string(),
                },
                &[
                    r#""event":"slice.merge.started""#,
                    r#""slice-name":"identity-user-registration""#,
                ],
            ),
            (
                EventKind::SliceMergeSucceeded {
                    slice_name: "identity-user-registration".to_string(),
                },
                &[
                    r#""event":"slice.merge.succeeded""#,
                    r#""slice-name":"identity-user-registration""#,
                ],
            ),
            (
                EventKind::SliceMergeFailed {
                    slice_name: "identity-user-registration".to_string(),
                    reason: "baseline-conflict".to_string(),
                },
                &[
                    r#""event":"slice.merge.failed""#,
                    r#""slice-name":"identity-user-registration""#,
                    r#""reason":"baseline-conflict""#,
                ],
            ),
            (
                EventKind::TargetExecutionAgent {
                    slice: "identity-user-registration".to_string(),
                    target: "omnia".to_string(),
                },
                &[
                    r#""event":"target.execution.agent""#,
                    r#""slice":"identity-user-registration""#,
                    r#""target":"omnia""#,
                ],
            ),
        ];

        for (kind, required) in rows {
            let event = Event::new(test_timestamp("2026-05-22T13:15:00Z"), kind.clone());
            let json = serde_json::to_string(&event).expect("serialise build/merge event");
            for needle in *required {
                assert!(json.contains(needle), "wire form must contain `{needle}`; got:\n{json}");
            }
            let round: Event = serde_json::from_str(&json).expect("deserialise build/merge event");
            assert_eq!(round, event, "build/merge round-trip must preserve every field");
        }
    }

    #[test]
    fn slice_synthesize_completed_omits_empty_artifacts() {
        // `artifacts` carries `skip_serializing_if = "Vec::is_empty"`
        // so an empty list does not reach the wire at all.
        let event = Event::new(
            test_timestamp("2026-05-22T13:15:00Z"),
            EventKind::SliceSynthesizeCompleted {
                slice_name: "identity-user-registration".to_string(),
                artifacts: vec![],
            },
        );
        let json = serde_json::to_string(&event).expect("serialise");
        assert!(
            !json.contains("artifacts"),
            "empty artifacts must not reach the wire; got:\n{json}"
        );
        let round: Event = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(round, event, "round-trip must preserve the empty artifacts list");
    }

    #[test]
    fn lint_completed_round_trips() {
        // The lint-completed payload uses snake_case wire fields
        // (`duration_ms`, `baseline_present`, `false_positive`,
        // `exit_code`) so the JSON matches the payload example
        // verbatim. The wire id itself stays dotted-kebab.
        let event = Event::new(
            test_timestamp("2026-05-22T13:15:00Z"),
            EventKind::LintCompleted(LintCompletedPayload {
                scope: LintScope {
                    target: Some("omnia".to_string()),
                    slice: None,
                    artifact: None,
                },
                duration_ms: 824,
                counts: LintCounts {
                    open: 12,
                    ignored: 4,
                    false_positive: 0,
                },
                baseline_present: false,
                exit_code: 2,
            }),
        );

        let json = serde_json::to_string(&event).expect("serialise lint-completed");
        let round_tripped: Event = serde_json::from_str(&json).expect("deserialise lint-completed");
        assert_eq!(round_tripped, event, "round-trip must preserve every field");

        for needle in [
            r#""event":"lint-completed""#,
            r#""scope":{"target":"omnia","slice":null,"artifact":null}"#,
            r#""duration_ms":824"#,
            r#""open":12"#,
            r#""ignored":4"#,
            r#""false_positive":0"#,
            r#""baseline_present":false"#,
            r#""exit_code":2"#,
        ] {
            assert!(
                json.contains(needle),
                "lint-completed wire form must contain `{needle}`; got:\n{json}"
            );
        }

        // Guard against an accidental rename_all = "kebab-case" on the
        // payload structs — those would flip the snake_case fields to
        // hyphenated names and silently break the RFC example.
        for forbidden in
            [r#""duration-ms""#, r#""baseline-present""#, r#""false-positive""#, r#""exit-code""#]
        {
            assert!(
                !json.contains(forbidden),
                "lint-completed wire form must NOT contain kebab-case `{forbidden}`; got:\n{json}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Single sweep covers every payload-bearing variant; splitting hides the wire-format coverage discipline."
    )]
    fn no_snake_case_leaks_to_wire() {
        // workflow §Wire format: snake_case lifecycle values are never
        // produced on disk. Exercise every variant that carries an
        // enum-shaped or hyphenable field name.
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        for kind in [
            EventKind::PlanTransitionApproved {
                plan_name: "p".to_string(),
            },
            EventKind::PlanAmendDivergence {
                plan_name: "p".to_string(),
                slice_name: "s".to_string(),
                from: Divergence::None,
                to: Divergence::Accepted,
            },
            EventKind::SliceTransitionRefined {
                slice_name: "s".to_string(),
            },
            EventKind::SliceSynthesizeStarted {
                slice_name: "s".to_string(),
            },
            EventKind::SliceSynthesizeAgent {
                slice_name: "s".to_string(),
            },
            EventKind::SliceSynthesizeCompleted {
                slice_name: "s".to_string(),
                artifacts: vec!["proposal.md".to_string()],
            },
            EventKind::SliceSynthesizeFailed {
                slice_name: "s".to_string(),
                reason: "spec-requirement-missing-sources".to_string(),
            },
            EventKind::SliceBuildStarted {
                slice_name: "s".to_string(),
            },
            EventKind::SliceBuildSucceeded {
                slice_name: "s".to_string(),
            },
            EventKind::SliceBuildFailed {
                slice_name: "s".to_string(),
                reason: "cargo-check-failed".to_string(),
            },
            EventKind::SliceMergeStarted {
                slice_name: "s".to_string(),
            },
            EventKind::SliceMergeSucceeded {
                slice_name: "s".to_string(),
            },
            EventKind::SliceMergeFailed {
                slice_name: "s".to_string(),
                reason: "baseline-conflict".to_string(),
            },
            EventKind::TargetExecutionAgent {
                slice: "s".to_string(),
                target: "omnia".to_string(),
            },
            EventKind::SliceExtractCompleted {
                slice_name: "s".to_string(),
                source: "k".to_string(),
            },
            EventKind::SourceSurveyCacheHit {
                source: "k".to_string(),
                adapter: "captures".to_string(),
                fingerprint: "sha256:beef".to_string(),
            },
            EventKind::SourceSurveyCacheMiss {
                source: "k".to_string(),
                adapter: "captures".to_string(),
                fingerprint: "sha256:beef".to_string(),
                reason: CacheMissReason::AdapterOptOut,
            },
            EventKind::SourceExecutionAgent {
                source: "k".to_string(),
                adapter: "captures".to_string(),
                operation: SourceOperation::Extract,
            },
            EventKind::PlanReconcileCompleted {
                plan_name: "p".to_string(),
                slice_count: 1,
                slice_names: vec!["s".to_string()],
            },
            EventKind::SliceArchiveCreated {
                slice_name: "s".to_string(),
                touched_specs: vec!["identity".to_string()],
                outcome_summary: "identity: 1 modified".to_string(),
                merge_sha: Some("abc1234".to_string()),
                decisions: Vec::new(),
            },
        ] {
            append_batch(
                layout,
                std::slice::from_ref(&Event::new(test_timestamp("2026-05-21T20:05:00Z"), kind)),
            )
            .expect("append ok");
        }
        let raw = std::fs::read_to_string(path(layout)).expect("read journal");
        for needle in [
            "plan_name",
            "slice_name",
            "slice_count",
            "slice_names",
            "requirement_id",
            "in_progress",
            "touched_specs",
            "outcome_summary",
            "merge_sha",
        ] {
            assert!(
                !raw.contains(needle),
                "snake_case `{needle}` must not appear on the wire; raw:\n{raw}"
            );
        }
    }
}
