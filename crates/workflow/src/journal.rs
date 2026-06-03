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

use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use specify_diagnostics::{Diagnostic, FindingStatus, count_status};
use specify_error::Error;

use crate::adapter::operation::SourceOperation;
use crate::change::Divergence;
use crate::config::Layout;
use crate::name::{PlanName, SliceName};

/// Project-relative path the journal lives at.
const JOURNAL_FILE_NAME: &str = "journal.jsonl";

/// Project-relative path of the dropped-event sidecar. A best-effort
/// append failure (see [`emit_best_effort`]) gets a second, recoverable
/// home here so an `O_APPEND` hiccup to the primary journal is never a
/// silent loss.
const DROPPED_FILE_NAME: &str = "journal.dropped";

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
        plan_name: PlanName,
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
        plan_name: PlanName,
        /// Entry id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// Status the entry held before the undo.
        from: crate::change::Status,
        /// Status the entry holds after the undo.
        to: crate::change::Status,
    },
    /// Stamped `slices[].divergence` via
    /// `specrun plan amend --divergence <likely|accepted|rejected>`.
    /// The CLI is the single writer. In the propose flow the
    /// `/spec:plan` agent stages `likely`
    /// through this event after `propose --from`; the operator later
    /// flips `accepted` / `rejected` the same way. This is the only
    /// path that writes the `divergence` field.
    #[serde(rename = "plan.amend.divergence", rename_all = "kebab-case")]
    PlanAmendDivergence {
        /// Plan name from `plan.yaml.name`.
        plan_name: PlanName,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
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
        slice_name: SliceName,
    },
    /// `/spec:refine` finished one source-bound `extract` call. One
    /// event per `(source, slice)` pair. Agent-driven.
    #[serde(rename = "slice.extract.completed", rename_all = "kebab-case")]
    SliceExtractCompleted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// Source key from `plan.yaml.sources.<key>`.
        source: String,
    },
    /// `[conflict]` on a requirement in `spec.md` — same-authority
    /// disagreement the operator must reconcile. Emitted by
    /// `specrun slice validate` after a successful run.
    #[serde(rename = "slice.synthesis.conflict", rename_all = "kebab-case")]
    SliceSynthesisConflict {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// `[divergence]` on a requirement in `spec.md` — cross-authority
    /// disagreement preserved as inline commentary. Emitted by
    /// `specrun slice validate` after a successful run.
    #[serde(rename = "slice.synthesis.divergence", rename_all = "kebab-case")]
    SliceSynthesisDivergence {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// `[unknown]` on a requirement in `spec.md` — a gap the operator
    /// must close before the requirement is meaningful. Emitted by
    /// `specrun slice validate` after a successful run.
    #[serde(rename = "slice.synthesis.unknown", rename_all = "kebab-case")]
    SliceSynthesisUnknown {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// Slice synthesis began — `/spec:refine` started folding the
    /// extracted evidence into `proposal.md` / `spec.md` / `design.md`
    /// / `tasks.md` / `model.yaml`. One event per slice. Distinct from the per-requirement
    /// `slice.synthesis.*` tag events above — `synthesize` is the
    /// lifecycle verb, `synthesis` is the requirement-tag noun.
    #[serde(rename = "slice.synthesize.started", rename_all = "kebab-case")]
    SliceSynthesizeStarted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
    },
    /// Synthesis dispatched to the agent. Synthesis is always
    /// agent-driven and `cache: opt-out`; this signal fires on the dry-run inputs phase so the
    /// journal records that no cache short-circuit was attempted.
    #[serde(rename = "slice.synthesize.agent", rename_all = "kebab-case")]
    SliceSynthesizeAgent {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
    },
    /// Slice synthesis finished and the artifacts were persisted.
    /// `artifacts` lists the persisted
    /// relative paths (`proposal.md`, `specs/<unit>/spec.md`,
    /// `design.md`, `tasks.md`, `model.yaml`).
    #[serde(rename = "slice.synthesize.completed", rename_all = "kebab-case")]
    SliceSynthesizeCompleted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// Persisted artifact relative paths, in write order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        artifacts: Vec<String>,
    },
    /// Slice synthesis failed before all artifacts were persisted.
    /// `reason` carries a short human
    /// reason or finding code so the journal records why the slice
    /// stalled.
    #[serde(rename = "slice.synthesize.failed", rename_all = "kebab-case")]
    SliceSynthesizeFailed {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// Short human reason / finding code for the failure.
        reason: String,
    },
    /// `/spec:build` started implementing the slice — the target
    /// adapter's `build` brief began running against the refined
    /// artifacts. One event per slice.
    #[serde(rename = "slice.build.started", rename_all = "kebab-case")]
    SliceBuildStarted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
    },
    /// `/spec:build` finished implementing the slice — the target
    /// adapter's `build` brief completed and the slice is ready for
    /// `/spec:merge`. One event per slice.
    #[serde(rename = "slice.build.succeeded", rename_all = "kebab-case")]
    SliceBuildSucceeded {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
    },
    /// `/spec:build` stopped before the slice was implemented.
    /// `reason` carries a short human
    /// reason or finding code so the journal records why the build
    /// stalled.
    #[serde(rename = "slice.build.failed", rename_all = "kebab-case")]
    SliceBuildFailed {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
        /// Short human reason / finding code for the failure.
        reason: String,
    },
    /// `specrun slice merge` began folding the slice's deltas into the
    /// baseline. The `slice.merge.*` pair
    /// fires on the `specrun slice merge` validator outcome, not on a
    /// merge report. One event per slice.
    #[serde(rename = "slice.merge.started", rename_all = "kebab-case")]
    SliceMergeStarted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
    },
    /// `specrun slice merge` validated and applied the slice's deltas
    /// to the baseline. Fires on the
    /// validator outcome, not on a merge report. One event per slice.
    #[serde(rename = "slice.merge.succeeded", rename_all = "kebab-case")]
    SliceMergeSucceeded {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
    },
    /// `specrun slice merge` refused to fold the slice into the
    /// baseline. Fires on the validator
    /// outcome, not on a merge report. `reason` carries a short human
    /// reason or finding code so the journal records why the merge
    /// stalled.
    #[serde(rename = "slice.merge.failed", rename_all = "kebab-case")]
    SliceMergeFailed {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
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
        slice_name: SliceName,
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
        slice_name: SliceName,
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
    /// `build` verb emits this per agent invocation.
    /// Unlike [`Self::SourceExecutionAgent`], which
    /// fans out over the `(source, operation)` pair, the build verb
    /// derives `(slice, target)` from the bound project — `build` is
    /// the only agent-dispatched target operation that emits this event
    /// in v1, so the payload stays minimal at `{ slice, target }`.
    #[serde(rename = "target.execution.agent", rename_all = "kebab-case")]
    TargetExecutionAgent {
        /// Slice id under `plan.yaml.slices[].name`.
        slice: SliceName,
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
        slice_name: SliceName,
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
        plan_name: PlanName,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: SliceName,
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
    /// response and wrote `plan.yaml.slices[]`. One indivisible event
    /// per successful invocation — the `/spec:plan` skill never calls
    /// `specrun journal emit` here.
    #[serde(rename = "plan.reconcile.completed", rename_all = "kebab-case")]
    PlanReconcileCompleted {
        /// Plan name from `plan.yaml.name`.
        plan_name: PlanName,
        /// Count of `plan.yaml.slices[]` rows written.
        slice_count: usize,
        /// Slice names, in the agent's `slices[]` response order.
        slice_names: Vec<SliceName>,
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
        slice_name: SliceName,
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
        /// this merge, in slug order. Empty stays off the wire;
        /// this is the durable ledger of promoted decisions alongside git
        /// history of `.specify/decisions/`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decisions: Vec<String>,
    },
    /// `specrun upgrade` self-updated the CLI binary. The new binary
    /// writes the event; `from` is the version observed before the
    /// upgrade, `to` the version now running, `channel` the resolved
    /// install channel (`cargo | brew | binary`).
    #[serde(rename = "cli.upgraded", rename_all = "kebab-case")]
    CliUpgraded {
        /// Version observed before the upgrade.
        from: String,
        /// Version now running.
        to: String,
        /// Resolved install channel (`cargo | brew | binary`).
        channel: String,
    },
    /// `specrun plugins refresh` invalidated the Cursor plugin cache.
    /// `deleted_paths` are the cache directories removed (wire:
    /// `deleted-paths`); `marketplace` is the resolved marketplace file
    /// path whose top-level `name` scoped the deletion.
    #[serde(rename = "plugins.refreshed", rename_all = "kebab-case")]
    PluginsRefreshed {
        /// Cache directories removed (wire: `deleted-paths`).
        deleted_paths: Vec<String>,
        /// Resolved marketplace file path whose top-level `name` scoped
        /// the deletion.
        marketplace: String,
    },
    /// `specrun migrate` applied a registered migrator. `kind` is the
    /// stable migrator id (e.g. `v1-to-v2`); the counts (wire:
    /// `files-rewritten`, `files-moved`) summarise the applied plan.
    #[serde(rename = "migration.applied", rename_all = "kebab-case")]
    MigrationApplied {
        /// Stable migrator id (e.g. `v1-to-v2`).
        kind: String,
        /// Count of files rewritten in place (wire: `files-rewritten`).
        files_rewritten: usize,
        /// Count of files moved (wire: `files-moved`).
        files_moved: usize,
    },
    /// `specrun migrate` staged a migrator but left the project
    /// untouched (atomic rollback). `kind` is the migrator id; `reason`
    /// is a short diagnostic (e.g. `staged-validation-failed`).
    #[serde(rename = "migration.skipped", rename_all = "kebab-case")]
    MigrationSkipped {
        /// Stable migrator id (e.g. `v1-to-v2`).
        kind: String,
        /// Short diagnostic (e.g. `staged-validation-failed`).
        reason: String,
    },
    /// `specrun lint` finished a scan. The payload carries the scan
    /// scope, wall-clock duration, per-status counts, a
    /// `baseline_present` flag (currently hard-coded `false`), and the
    /// CLI exit code the scan resolved to. Emission is
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
    /// Per-status counts. The scanner emits `open`, `ignored`, and
    /// `false_positive`.
    pub counts: LintCounts,
    /// Whether the scan observed a baseline file. Hard-coded `false`
    /// in current emitters.
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
/// emitters fill the three buckets named here.
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
/// for a given file, deterministic.
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

/// Byte window the backward tail reader pulls per `read`/`seek`. One
/// `O_APPEND` journal line stays well under this, so the common case of a
/// few recent matches resolves in a single window.
const TAIL_CHUNK: usize = 8192;

/// Read the most recent journal [`Event`]s that `select` maps to a value,
/// returning at most `limit` of them in append (file) order.
///
/// Tails the journal backward (see [`for_each_line_rev`]) and stops as
/// soon as `limit` matches are collected, so the bytes touched are bounded
/// by how far back the `limit`-th match sits — not by total history. This
/// keeps the projection cost flat as the journal grows.
///
/// Blank lines are skipped and lines that fail to parse as an [`Event`]
/// are skipped rather than failing the read — identical leniency to
/// [`read`], so a journal written by a newer binary still yields the
/// matches this binary understands. A missing journal yields an empty
/// vector. This is the read side the identity projection
/// (`recent[]`) consumes.
///
/// # Errors
///
/// Propagates I/O failures other than a missing file.
pub(crate) fn read_recent<T>(
    layout: Layout<'_>, limit: usize, mut select: impl FnMut(Event) -> Option<T>,
) -> Result<Vec<T>, Error> {
    let mut newest_first: Vec<T> = Vec::new();
    if limit == 0 {
        return Ok(newest_first);
    }
    for_each_line_rev(&path(layout), TAIL_CHUNK, |line| {
        if line.trim().is_empty() {
            return true;
        }
        if let Ok(event) = serde_json::from_str::<Event>(line)
            && let Some(item) = select(event)
        {
            newest_first.push(item);
            if newest_first.len() >= limit {
                return false;
            }
        }
        true
    })
    .map_err(Error::Io)?;
    newest_first.reverse();
    Ok(newest_first)
}

/// Visit the complete lines of the file at `path` newest-first, invoking
/// `visit` for each; `visit` returns `false` to stop early (the unread
/// head of the file is then never read).
///
/// The file is read backward in `chunk`-byte windows, so only the tail the
/// consumer scans is touched. Line boundaries follow [`str::lines`]: a
/// single trailing newline is a terminator (no empty final line) while
/// interior blank lines are preserved. Splitting happens on `b'\n'`
/// boundaries — multi-byte UTF-8 sequences spanning a chunk edge are
/// reassembled before decoding, and every emitted line spans from just
/// after a newline (or file start) to just before the next newline (or
/// end), which are always character boundaries in a valid UTF-8 journal.
///
/// A missing file yields no visits (`Ok(())`), mirroring [`read`].
fn for_each_line_rev(
    path: &Path, chunk: usize, mut visit: impl FnMut(&str) -> bool,
) -> std::io::Result<()> {
    debug_assert!(chunk > 0, "tail chunk size must be non-zero");
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut pos = file.seek(SeekFrom::End(0))?;
    if pos == 0 {
        return Ok(());
    }
    let chunk = u64::try_from(chunk).unwrap_or(u64::MAX);
    // `carry` holds the leading partial segment of the window read so far
    // (the bytes before its first newline); its true start lies in an
    // as-yet-unread earlier chunk, so it is only decoded once `pos` hits 0.
    let mut carry: Vec<u8> = Vec::new();
    let mut first = true;
    while pos > 0 {
        let take = pos.min(chunk);
        pos -= take;
        file.seek(SeekFrom::Start(pos))?;
        let mut window = vec![0_u8; usize::try_from(take).unwrap_or(usize::MAX)];
        file.read_exact(&mut window)?;
        window.extend_from_slice(&carry);
        if first {
            first = false;
            // Drop a single trailing newline so a terminator does not yield
            // an empty final line (str::lines parity).
            if window.last() == Some(&b'\n') {
                window.pop();
            }
        }
        // Emit every line after the first newline (newest-first); retain
        // the pre-first-newline head as the next `carry`.
        while let Some(idx) = window.iter().rposition(|&byte| byte == b'\n') {
            let keep_going = visit(String::from_utf8_lossy(&window[idx + 1..]).as_ref());
            window.truncate(idx);
            if !keep_going {
                return Ok(());
            }
        }
        carry = window;
    }
    // `pos == 0`: the remaining bytes are the file's first line.
    visit(String::from_utf8_lossy(&carry).as_ref());
    Ok(())
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

/// Best-effort append of a single lifecycle [`Event`] carrying `kind`.
///
/// Timestamped `Timestamp::now()`. The journal is observability, not the
/// source of truth, so a failed append is **intentionally swallowed** —
/// it can never change the calling verb's exit code (a journaling I/O
/// hiccup must not fail an otherwise-successful slice merge / build). The
/// lifecycle brackets in `slice merge` / `slice build` emit through this.
///
/// The swallow is intentional but **not silent**: `record_dropped`
/// routes a structured `warning:` line to stderr (naming `scope`, the
/// journal path, and the I/O error) through the same operator-warning
/// surface other best-effort failures use, and appends the dropped event
/// to the `<project_dir>/.specify/journal.dropped` sidecar as a
/// recoverable audit trail. The mitigation is itself best-effort and
/// never panics.
pub fn emit_best_effort(layout: Layout<'_>, kind: EventKind, scope: &str) {
    let event = Event::new(Timestamp::now(), kind);
    if let Err(err) = append_batch(layout, std::slice::from_ref(&event)) {
        record_dropped(layout, scope, &event, &err);
    }
}

/// Surface a dropped journal [`Event`] so the best-effort swallow in
/// [`emit_best_effort`] / [`emit_lint_completed`] is observable and
/// recoverable rather than silent.
///
/// Emits an operator-visible `warning:` line to stderr — matching the
/// repo's established best-effort warning idiom — and attempts to append
/// the event to the `<project_dir>/.specify/journal.dropped` sidecar (a
/// second chance at durability when the primary append failed for a
/// path-local reason). The sidecar write is itself best-effort: if it
/// too fails the stderr warning still surfaces the drop, and neither path
/// changes the calling verb's exit code or panics.
fn record_dropped(layout: Layout<'_>, scope: &str, event: &Event, err: &Error) {
    let journal = path(layout);
    let sidecar = layout.specify_dir().join(DROPPED_FILE_NAME);
    if append_dropped(layout, event).is_ok() {
        eprintln!(
            "warning: {scope}: failed to append journal event to {} ({err}); \
             recorded the dropped event in {} for recovery",
            journal.display(),
            sidecar.display(),
        );
    } else {
        eprintln!(
            "warning: {scope}: failed to append journal event to {} ({err}); \
             the dropped event could not be written to the {} sidecar either",
            journal.display(),
            sidecar.display(),
        );
    }
}

/// Append `event` as one newline-terminated JSON line to the
/// `<project_dir>/.specify/journal.dropped` sidecar.
///
/// Mirrors [`append_batch`]'s open/append shape but is reserved for
/// events the primary journal append dropped. Returns the I/O or
/// serialisation error to the caller, which discards it ([`record_dropped`]
/// has already warned on stderr) — the helper itself never panics.
fn append_dropped(layout: Layout<'_>, event: &Event) -> Result<(), Error> {
    let line = serde_json::to_string(event).map_err(|err| Error::Diag {
        code: "journal-event-serialise-failed",
        detail: format!("failed to serialise dropped journal event: {err}"),
    })?;
    std::fs::create_dir_all(layout.specify_dir())?;
    let sidecar = layout.specify_dir().join(DROPPED_FILE_NAME);
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&sidecar)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Append a `lint-completed` event to `<project_dir>/.specify/journal.jsonl`.
///
/// Best-effort: a serialise/IO failure is intentionally swallowed so it
/// never overrides the scan's exit code. The swallow is not silent —
/// `record_dropped` warns on stderr under `command_label` and records
/// the dropped event in the `.specify/journal.dropped` sidecar.
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
        record_dropped(layout, command_label, &event, &err);
    }
}

/// Parses a fixed RFC3339 timestamp for test fixtures.
#[cfg(test)]
pub(crate) fn test_timestamp(raw: &str) -> Timestamp {
    raw.parse().expect("valid rfc3339 timestamp in test fixture")
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod wire_shapes;
