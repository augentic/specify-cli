//! On-disk representation of `.specify/plan.yaml` and the in-memory
//! [`Plan`] state machine that wraps it.
//!
//! See `rfcs/rfc-2-execution.md` §"Library Implementation" for the canonical
//! type surface and `rfcs/rfc-2-execution.md` §"The Plan" for the reference
//! YAML fixture exercised by the round-trip tests.
//!
//! ## Single-writer invariant for `PlanChange::status`
//!
//! The only path that mutates an existing [`PlanChange::status`] is
//! [`Plan::transition`]. This is not just a convention — it's enforced
//! by the shape of the API:
//!
//!   - [`Plan::create`] appends a new entry and forces its `status` to
//!     [`PlanStatus::Pending`]; any other value the caller supplied is
//!     silently overwritten and `status_reason` is cleared.
//!   - [`Plan::amend`] takes a [`PlanChangePatch`] which structurally
//!     has no `status` (or `status_reason`) field — a type-system
//!     guarantee that `amend` cannot mutate lifecycle state.
//!   - [`Plan::transition`] delegates to [`PlanStatus::transition`]
//!     for edge-legality and is the only place that writes
//!     `entry.status` or `entry.status_reason`.
//!
//! Any future writer of `status` should route through `Plan::transition`
//! rather than poking the field directly, or add a new `Plan::*`
//! method that does. Bypassing this invariant undoes the state-machine
//! guarantees exercised by the L1.B transition tests.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use petgraph::Direction;
use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use specify_error::Error;

/// Lifecycle state of a single entry in [`Plan::changes`].
///
/// The enum is `Copy + Eq + Hash` so it can appear in `HashSet`s,
/// `match` guards, and hash-keyed lookups without clones. This mirrors
/// the derives already used on `LifecycleStatus` in the parent module.
/// Transition-table methods (`can_transition_to`, `transition`) land in
/// Change L1.B and intentionally do not exist yet.
#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Deserialize,
    Serialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Done,
    Blocked,
    Failed,
    Skipped,
}

impl PlanStatus {
    /// Every variant in declaration order. Used by exhaustive transition
    /// tests here and by validation/topological code in L1.D/E that
    /// needs to enumerate states without depending on `strum`.
    pub const ALL: [PlanStatus; 6] = [
        PlanStatus::Pending,
        PlanStatus::InProgress,
        PlanStatus::Done,
        PlanStatus::Blocked,
        PlanStatus::Failed,
        PlanStatus::Skipped,
    ];

    /// Whether `self -> target` is a legal edge in the plan-entry state
    /// machine. See `rfc-2-execution.md` §"Transition Rules" for the canonical
    /// table; the 10 edges enumerated below are the *only* legal ones.
    /// `Done` is terminal: every edge with `Done` on the left is `false`.
    pub fn can_transition_to(&self, target: &PlanStatus) -> bool {
        use PlanStatus::*;
        matches!(
            (self, target),
            (Pending, InProgress)
                | (Pending, Blocked)
                | (Pending, Skipped)
                | (InProgress, Done)
                | (InProgress, Failed)
                | (InProgress, Blocked)
                | (Blocked, Pending)
                | (Failed, Pending)
                | (Failed, Skipped)
                | (Skipped, Pending)
        )
    }

    /// Return `target` if the edge is legal, otherwise an
    /// `Error::PlanTransition` carrying both endpoints by their `Debug`
    /// representation. Mirrors `LifecycleStatus::transition`.
    pub fn transition(&self, target: PlanStatus) -> Result<PlanStatus, Error> {
        if self.can_transition_to(&target) {
            Ok(target)
        } else {
            Err(Error::PlanTransition {
                from: format!("{self:?}"),
                to: format!("{target:?}"),
            })
        }
    }
}

/// In-memory model of `.specify/plan.yaml`.
///
/// A `Plan` is an ordered, dependency-aware list of [`PlanChange`]s plus
/// a named map of [`Plan::sources`] (local paths or git URLs) that the
/// entries draw from.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Plan {
    /// Human-readable plan name, e.g. `platform-v2`.
    pub name: String,
    /// Named source locations referenced by [`PlanChange::sources`].
    /// Optional in the YAML; defaults to an empty map.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Ordered list of plan entries. Order is the *intended* execution
    /// order; the authoritative dependency-respecting order comes from
    /// [`Plan::topological_order`].
    pub changes: Vec<PlanChange>,
}

/// One entry in [`Plan::changes`].
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PlanChange {
    /// Stable identifier (kebab-case) unique within the plan.
    pub name: String,
    /// Current lifecycle state of this entry.
    pub status: PlanStatus,
    /// Names of other plan entries that must reach `done` before this
    /// entry is eligible.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Names of other plan entries this one logically *affects* (e.g.
    /// a bug-fix change whose scope modifies an already-done entry).
    #[serde(default)]
    pub affects: Vec<String>,
    /// Source keys (into [`Plan::sources`]) this entry draws from.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Per-source scope narrowing `/spec:extract`'s view of each
    /// source. Keys are source names drawn from this entry's
    /// [`PlanChange::sources`] list; values are [`Scope`]s carrying
    /// either gitignore-style `include`/`exclude` globs or a
    /// `manifest` pointer (the two forms are mutually exclusive per
    /// entry — enforced on deserialization and on construction via
    /// [`Scope::try_new`]). Empty on every plan that predates RFC-3a,
    /// skipped on serialize when empty so such plans round-trip
    /// byte-for-byte. See RFC-3a §*The `scope` field* and
    /// §*Manifest shape*. Cross-key referential integrity (scope
    /// keys must also appear in `sources`) is enforced by
    /// [`Plan::validate`], not this type.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scope: BTreeMap<String, Scope>,
    /// Free-form human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Operational explanation for the current non-terminal/terminal
    /// status (`failed`, `blocked`, or `skipped`). Overwritten on each
    /// status transition; cleared when the entry returns to `pending`,
    /// `in-progress`, or `done`. See §Fields.
    #[serde(default)]
    pub status_reason: Option<String>,
}

/// One source's scope under [`PlanChange::scope`]. Carries either a
/// pair of gitignore-style `include`/`exclude` glob lists *or* a
/// `manifest` pointer to an enumerated file list — never both.
///
/// # Invariant
///
/// `manifest` ⊕ `(include | exclude)`: a `Scope` that has a
/// `manifest` set cannot also carry `include` or `exclude`, and
/// vice-versa. Enforced at every entry point:
///
///   - [`Scope::try_new`] — programmatic construction; returns
///     [`Error::InvalidPlanScope`] on violation.
///   - `Deserialize` — routed through a private shadow struct so
///     the same invariant applies to YAML/JSON input; violations
///     surface through [`Error::Yaml`] at the outer call site with
///     a message mentioning the rejected combination.
///
/// An empty `Scope` (`{}`) is valid and semantically equivalent to
/// omitting the entry entirely — see RFC-3a §*The `scope` field*.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Scope {
    /// Gitignore-style glob patterns selecting which files
    /// `/spec:extract` reads. Paths are resolved relative to the
    /// source's mapped path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Gitignore-style glob patterns subtracted from `include` (or
    /// from the full source tree when `include` is empty). Cannot
    /// hide sentinel files — sentinel discovery ignores `exclude`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Escape-hatch pointer to a per-slice manifest file enumerating
    /// files explicitly. Mutually exclusive with `include`/`exclude`
    /// for this source key (see the type-level invariant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
}

/// Private shadow struct used as the deserialization target for
/// [`Scope`]. The `impl<'de> Deserialize<'de> for Scope` below
/// funnels YAML/JSON input through `ScopeShape` → [`Scope::try_new`]
/// so the `manifest` ⊕ `(include|exclude)` invariant check lives in
/// exactly one place and applies uniformly to programmatic and
/// on-the-wire construction.
///
/// `deny_unknown_fields` mirrors `additionalProperties: false` in
/// the plan JSON schema — without it, typos (e.g. `manifests:`)
/// would silently round-trip as no-ops, letting the two surfaces
/// drift.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ScopeShape {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    manifest: Option<String>,
}

impl Scope {
    /// Build a [`Scope`] enforcing the `manifest` ⊕
    /// `(include|exclude)` invariant. Returns
    /// [`Error::InvalidPlanScope`] with an actionable message
    /// naming both field names when the combination is rejected.
    pub fn try_new(
        include: Vec<String>, exclude: Vec<String>, manifest: Option<String>,
    ) -> Result<Self, Error> {
        if manifest.is_some() && (!include.is_empty() || !exclude.is_empty()) {
            return Err(Error::InvalidPlanScope(format!(
                "scope entry cannot combine `manifest` with `include`/`exclude` \
                 (got manifest={:?}, include={} entries, exclude={} entries)",
                manifest.as_deref().unwrap_or(""),
                include.len(),
                exclude.len(),
            )));
        }
        Ok(Scope {
            include,
            exclude,
            manifest,
        })
    }
}

impl TryFrom<ScopeShape> for Scope {
    type Error = Error;

    fn try_from(shape: ScopeShape) -> Result<Self, Self::Error> {
        Scope::try_new(shape.include, shape.exclude, shape.manifest)
    }
}

// Route `Deserialize` through `ScopeShape` so the XOR invariant is
// enforced on parse. Hand-rolled (rather than `#[serde(try_from =
// "ScopeShape")]`) so the existing `#[derive(Serialize)]` on
// `Scope` keeps emitting the field-level `skip_serializing_if`
// attributes directly — we only want the redirection on the
// deserialize side.
impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let shape = ScopeShape::deserialize(deserializer)?;
        Scope::try_from(shape).map_err(serde::de::Error::custom)
    }
}

/// Patch applied by [`Plan::amend`] to an existing entry. Every field is
/// `Option<T>`; `None` means "leave unchanged", `Some(v)` means "replace
/// with v". `status` and `status_reason` are deliberately absent —
/// status transitions are made via [`Plan::transition`], never through
/// `amend`, and the reason field travels with the transition.
///
/// The absence of a `status` field is a type-system guarantee: `amend`
/// cannot mutate status. Transitions go via [`Plan::transition`].
#[derive(Debug, Default, Clone)]
pub struct PlanChangePatch {
    /// Replace `depends_on` wholesale when `Some`.
    pub depends_on: Option<Vec<String>>,
    /// Replace `affects` wholesale when `Some`.
    pub affects: Option<Vec<String>>,
    /// Replace `sources` wholesale when `Some`.
    pub sources: Option<Vec<String>>,
    /// Replace `description` when `Some(Some(..))`; clear when
    /// `Some(None)`; leave unchanged when `None`.
    pub description: Option<Option<String>>,
    /// Per-source scope edits, wholesale-replace-per-key. For each
    /// `(key, value)` pair: `Some(scope)` sets or replaces the entry
    /// at `key` in [`PlanChange::scope`]; `None` removes it. Keys
    /// absent from this map are left untouched. An empty map is a
    /// no-op and preserves any existing scope byte-for-byte — that is
    /// the round-trip guarantee exercised by the "amend untouched by
    /// status transition" test. See RFC-3a §*How `scope` travels
    /// through the pipeline*.
    pub scope: BTreeMap<String, Option<Scope>>,
}

/// Severity of a validation finding produced by [`Plan::validate`].
#[derive(Debug, Clone, PartialEq)]
pub enum PlanValidationLevel {
    /// Blocking problem — the plan is not usable as-is.
    Error,
    /// Non-blocking advisory — the plan is usable but something looks
    /// off (e.g. a source key is defined but unreferenced).
    Warning,
}

/// A single finding reported by [`Plan::validate`].
#[derive(Debug, Clone)]
pub struct PlanValidationResult {
    /// Severity bucket.
    pub level: PlanValidationLevel,
    /// Stable machine-readable code, e.g. `"plan.cycle"`.
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
    /// Name of the offending entry, when the finding is entry-local.
    pub entry: Option<String>,
}

impl Plan {
    /// Create an empty plan with the given name and optional named sources.
    ///
    /// Every entry starts with `status: pending`; this just initialises the
    /// top-level struct. The name is validated with
    /// [`crate::actions::validate_name`] so it obeys the same kebab-case
    /// rules as change names (RFC-1 §"Naming Rules").
    ///
    /// Does NOT write anything to disk. Call [`Plan::save`] afterwards.
    pub fn init(name: &str, sources: BTreeMap<String, String>) -> Result<Plan, Error> {
        crate::actions::validate_name(name)?;
        Ok(Plan {
            name: name.to_string(),
            sources,
            changes: vec![],
        })
    }

    /// Load `.specify/plan.yaml` from disk.
    ///
    /// Errors mirror [`crate::ChangeMetadata::load`]:
    ///   - missing file -> `Error::Config`
    ///   - malformed YAML -> `Error::Yaml`
    ///   - other I/O failure -> `Error::Io`
    ///
    /// Tolerant of files with or without a trailing newline —
    /// `serde_yaml::from_str` accepts both.
    pub fn load(path: &Path) -> Result<Self, Error> {
        if !path.exists() {
            return Err(Error::Config(format!("plan.yaml not found at {}", path.display())));
        }
        let content = std::fs::read_to_string(path)?;
        let plan: Plan = serde_yaml::from_str(&content)?;
        Ok(plan)
    }

    /// Serialize and write the plan to `path`, overwriting if present.
    ///
    /// Atomic: a partial file is never observed by readers. Write goes via
    /// a temp file in the same directory followed by `fs::rename`. Because
    /// POSIX `rename(2)` (and Windows `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`)
    /// are atomic at the filesystem level, any concurrent reader of `path`
    /// sees either the previous complete contents or the new complete
    /// contents — never a half-written or empty file. Placing the temp
    /// file in `path.parent()` keeps the rename on the same filesystem,
    /// which is the precondition that makes the rename atomic rather than
    /// a copy-then-unlink.
    ///
    /// Always emits a trailing newline so the on-disk form matches the
    /// convention used elsewhere in the project and so POSIX text-file
    /// tools (`wc -l`, `sed`, `grep`) behave predictably.
    ///
    /// Returns `Error::Io` on any I/O failure and `Error::Yaml` if
    /// serialization fails.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        crate::atomic::atomic_yaml_write(path, self)
    }

    /// Run all structural and semantic checks over the plan.
    ///
    /// `changes_dir` (when `Some`) points at `.specify/changes/` and
    /// enables the cross-reference checks against on-disk change
    /// metadata. `project_dir` (when `Some`) points at the project
    /// root and enables the filesystem-aware scope sweeps introduced
    /// in RFC-3a:
    ///   - `scope-path-missing` (C04): Error-level, fires for any
    ///     include/exclude/manifest path that fails to resolve.
    ///   - `scope-overlap`, `scope-orphan` (C05): Warning-level,
    ///     cross-entry file-ownership lint. Both fire only when at
    ///     least one change has a non-empty `scope`, preserving
    ///     back-compat for plans authored before RFC-3a.
    ///   - `scope-missing-on-monolith` (C25): Warning-level,
    ///     fires when `/spec:analyze` has classified a source as
    ///     monolith-scale (LOC or module-count above the Omnia
    ///     default threshold — see [`MONOLITH_LOC_THRESHOLD`] and
    ///     [`MONOLITH_MODULE_COUNT_THRESHOLD`]) and the change
    ///     carries no `scope.<key>` entry. Driven off the
    ///     structural metadata at
    ///     `.specify/plans/<plan.name>/analyze/<key>/metadata.json`
    ///     produced by the `/spec:analyze` skill (C20); absent
    ///     metadata silently skips, so small-legacy / greenfield
    ///     changes never see this warning.
    ///   - `manifest-invalid`, `manifest-empty`, `manifest-path-escape`
    ///     (C26): manifest files referenced from `scope.<key>.manifest`
    ///     must parse as v1 `{ version: 1, include: [ … ] }` with
    ///     `deny_unknown_fields`; each `include` entry must be a
    ///     source-root-relative path without `..` or absolute segments,
    ///     and must resolve to an existing file under `sources[key]`.
    ///     The `manifest` ⊕ `(include|exclude)` rule for [`Scope`] is
    ///     enforced at parse time via [`Scope::try_new`]; C26 adds
    ///     on-disk manifest shape validation only.
    ///
    /// Passing `None` for `project_dir` silently skips every
    /// filesystem-aware sweep above so library-level callers
    /// (`Plan::create`/`Plan::amend` during authoring) don't have to
    /// materialise a real tree.
    ///
    /// The `scope-path-missing` check resolves each scope
    /// `include`/`exclude` glob under `project_dir.join(sources[key])`
    /// and each `manifest` pointer under `project_dir` directly
    /// (manifests are project-relative per RFC-3a §*Manifest shape*).
    /// Source values that are URLs (`http://`, `https://`, `git@`,
    /// `git+…`, `ssh://`) or absolute paths are skipped silently —
    /// remote or developer-local absolute paths aren't meaningfully
    /// checkable from the project root.
    ///
    /// Findings are accumulated — no check short-circuits another. Order
    /// is structural checks first (duplicate names, cycles, unknown
    /// depends-on / affects / sources, multiple in-progress) followed by
    /// consistency checks against `changes_dir` when provided.
    ///
    /// Note on "well-formed status values": `PlanStatus` is an enum, so
    /// every in-memory instance is well-formed by construction. serde
    /// rejects invalid statuses at parse time. The RFC lists this check
    /// for completeness against hand-edited YAML that bypassed parsing,
    /// which is not reachable in-process — so nothing is emitted for it.
    pub fn validate(
        &self, changes_dir: Option<&Path>, project_dir: Option<&Path>,
    ) -> Vec<PlanValidationResult> {
        let mut results = Vec::new();
        results.extend(collect_duplicate_names(&self.changes));
        results.extend(detect_cycles(&self.changes));
        results.extend(check_unknown_depends_on(&self.changes));
        results.extend(check_unknown_affects(&self.changes));
        results.extend(check_unknown_sources(self));
        results.extend(check_scope_keys_in_sources(&self.changes));
        results.extend(check_single_in_progress(&self.changes));
        if let Some(dir) = changes_dir.filter(|d| d.is_dir()) {
            results.extend(check_changes_dir_consistency(self, dir));
        }
        if let Some(root) = project_dir {
            results.extend(check_scope_paths_exist(self, root));
            results.extend(check_scope_manifest_shapes(self, root));
            results.extend(check_scope_coverage(self, root));
            results.extend(check_scope_missing_on_monolith(self, root));
        }
        results
    }

    /// First entry in list order whose dependencies are all `done` and
    /// whose own status is `pending`. Returns `None` when nothing is
    /// eligible (plan finished, blocked, empty) **or when any entry is
    /// currently `in-progress`** — the driver must not pick a new
    /// change while one is active. The in-progress check runs before
    /// any dependency walk, so this function is independent of
    /// [`Plan::topological_order`] and safe to call on cyclic plans.
    ///
    /// An unknown `depends_on` target is treated as "not done", so the
    /// entry is not eligible. Orphan-reference diagnostics belong to
    /// [`Plan::validate`].
    pub fn next_eligible(&self) -> Option<&PlanChange> {
        if self.changes.iter().any(|c| c.status == PlanStatus::InProgress) {
            return None;
        }
        let status_by_name: HashMap<&str, PlanStatus> =
            self.changes.iter().map(|c| (c.name.as_str(), c.status)).collect();
        self.changes.iter().find(|c| {
            c.status == PlanStatus::Pending
                && c.depends_on
                    .iter()
                    .all(|dep| status_by_name.get(dep.as_str()).copied() == Some(PlanStatus::Done))
        })
    }

    /// Transition the named entry to `target`, recording `reason` in
    /// [`PlanChange::status_reason`] per the rules documented in
    /// `rfc-2-execution.md` §Fields.
    ///
    /// `reason` is only meaningful when `target` is one of
    /// `{Failed, Blocked, Skipped}`; passing `Some(_)` with any other
    /// target returns `Error::Config`. On a legal reason-less
    /// transition to `Pending`, `InProgress`, or `Done`,
    /// `status_reason` is cleared.
    ///
    /// Does not run `Plan::validate` — a status mutation cannot make a
    /// previously-valid plan invalid (the state machine has no
    /// structural side-effects).
    pub fn transition(
        &mut self, name: &str, target: PlanStatus, reason: Option<&str>,
    ) -> Result<(), Error> {
        let entry = self
            .changes
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| Error::Config(format!("no change named '{name}' in plan")))?;

        let new_status = entry.status.transition(target)?;

        match target {
            PlanStatus::Failed | PlanStatus::Blocked | PlanStatus::Skipped => {
                if let Some(s) = reason {
                    entry.status_reason = Some(s.to_string());
                }
            }
            PlanStatus::Pending | PlanStatus::InProgress | PlanStatus::Done => {
                if reason.is_some() {
                    return Err(Error::Config(format!(
                        "--reason is not valid when transitioning to {target:?}"
                    )));
                }
                entry.status_reason = None;
            }
        }

        entry.status = new_status;
        Ok(())
    }

    /// Append a new entry to the plan, rejecting duplicate names and
    /// invalid kebab-case names. The incoming `status` is forced to
    /// [`PlanStatus::Pending`] (and `status_reason` cleared) so that
    /// creation cannot introduce a pre-occupied lifecycle state — the
    /// single-writer-for-status invariant documented at the top of
    /// this module.
    ///
    /// After mutation, the plan is re-validated. Any `Error`-level
    /// finding (unknown `depends_on`/`affects`/`sources`, cycle
    /// introduced by the new entry, etc.) rolls back the append and
    /// returns `Error::Config` containing the first offending
    /// finding's message. Warnings are tolerated — they're a CLI
    /// concern, not a library-level hard stop.
    pub fn create(&mut self, change: PlanChange) -> Result<(), Error> {
        crate::actions::validate_name(&change.name)?;

        if self.changes.iter().any(|c| c.name == change.name) {
            return Err(Error::Config(format!(
                "plan already contains a change named '{}'",
                change.name
            )));
        }

        let mut change = change;
        change.status = PlanStatus::Pending;
        change.status_reason = None;

        // Targeted rollback: pop the freshly-appended entry if
        // validation rejects the resulting plan. Cheaper than cloning
        // the whole `changes` vector since we know the mutation is a
        // single trailing push.
        self.changes.push(change);
        // Surface RFC-3a's `scope-key-not-in-sources` through the
        // dedicated [`Error::InvalidPlanScopeKey`] variant before
        // falling back to the generic validate sweep, so the CLI
        // can map it to a stable wire kind.
        if let Some((change, key)) = first_orphan_scope_key(&self.changes) {
            self.changes.pop();
            return Err(Error::InvalidPlanScopeKey { change, key });
        }
        let errors: Vec<PlanValidationResult> = self
            .validate(None, None)
            .into_iter()
            .filter(|r| r.level == PlanValidationLevel::Error)
            .collect();
        if let Some(first) = errors.first() {
            let msg = first.message.clone();
            self.changes.pop();
            return Err(Error::Config(format!("plan validation failed after create: {msg}")));
        }

        Ok(())
    }

    /// Apply `patch` to the entry named `name`. `None` fields on the
    /// patch leave the corresponding [`PlanChange`] field unchanged;
    /// `Some(v)` replaces wholesale. `description` is three-way:
    /// `None` = leave, `Some(None)` = clear, `Some(Some(s))` =
    /// replace. `status` is intentionally not patchable — see
    /// [`PlanChangePatch`] and the module-level single-writer note.
    ///
    /// After mutation, the plan is re-validated. Any `Error`-level
    /// finding reverts the single-entry mutation (we snapshot the
    /// pre-mutation entry at the top of the function and write it
    /// back on failure) and returns `Error::Config`.
    ///
    /// `amend` does not consult `PlanChange::status` — it is legal to
    /// amend the currently-`in-progress` entry's non-status fields,
    /// per RFC-2 §"Phase Boundary → Rule 2".
    pub fn amend(&mut self, name: &str, patch: PlanChangePatch) -> Result<(), Error> {
        let idx = self
            .changes
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| Error::Config(format!("no change named '{name}' in plan")))?;

        // Snapshot for targeted rollback on validation failure.
        let snapshot = self.changes[idx].clone();

        {
            let entry = &mut self.changes[idx];
            if let Some(v) = patch.depends_on {
                entry.depends_on = v;
            }
            if let Some(v) = patch.affects {
                entry.affects = v;
            }
            if let Some(v) = patch.sources {
                entry.sources = v;
            }
            if let Some(v) = patch.description {
                entry.description = v;
            }
            // Per-key wholesale replace-or-remove on `scope`. Keys
            // absent from `patch.scope` are left alone so a keyless
            // patch (the common case: `amend --depends-on …`)
            // preserves scope byte-for-byte.
            for (key, value) in patch.scope {
                match value {
                    Some(scope) => {
                        entry.scope.insert(key, scope);
                    }
                    None => {
                        entry.scope.remove(&key);
                    }
                }
            }
        }

        if let Some((change, key)) = first_orphan_scope_key(&self.changes) {
            self.changes[idx] = snapshot;
            return Err(Error::InvalidPlanScopeKey { change, key });
        }
        let errors: Vec<PlanValidationResult> = self
            .validate(None, None)
            .into_iter()
            .filter(|r| r.level == PlanValidationLevel::Error)
            .collect();
        if let Some(first) = errors.first() {
            let msg = first.message.clone();
            self.changes[idx] = snapshot;
            return Err(Error::Config(format!("plan validation failed after amend: {msg}")));
        }

        Ok(())
    }

    /// Entries in dependency-respecting order. Errors with an
    /// `Error::Config` describing the cycle when the `depends_on` graph
    /// contains one.
    ///
    /// Tie-break rule: when two entries are simultaneously "ready"
    /// (dependencies already emitted), the one earlier in
    /// [`Plan::changes`] wins. This makes the output deterministic and
    /// a pure function of list order.
    ///
    /// Unknown `depends_on` targets are treated as satisfied for
    /// ordering purposes so orphan references cannot deadlock the sort;
    /// surfacing them is [`Plan::validate`]'s job.
    ///
    /// Implementation: we build a `DiGraph`, use `petgraph::toposort`
    /// (plus `tarjan_scc` on failure) for cycle detection and
    /// offender-naming, then walk the graph via a priority-queue Kahn
    /// where the priority is the original `NodeIndex` (which equals
    /// each entry's list position, since we insert in list order).
    /// That keeps the list-order tie-break contract while dropping
    /// the old O(n²) "sweep until fixpoint" fallback.
    pub fn topological_order(&self) -> Result<Vec<&PlanChange>, Error> {
        let mut graph: DiGraph<&str, ()> = DiGraph::new();
        let mut idx = HashMap::new();
        for entry in &self.changes {
            let node = graph.add_node(entry.name.as_str());
            idx.insert(entry.name.as_str(), node);
        }
        for entry in &self.changes {
            let to = idx[entry.name.as_str()];
            for dep in &entry.depends_on {
                if let Some(&from) = idx.get(dep.as_str()) {
                    graph.add_edge(from, to, ());
                }
            }
        }

        if toposort(&graph, None).is_err() {
            let offender = tarjan_scc(&graph)
                .into_iter()
                .find(|scc| {
                    scc.len() > 1 || (scc.len() == 1 && graph.find_edge(scc[0], scc[0]).is_some())
                })
                .map(|scc| graph[scc[0]].to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(Error::Config(format!("plan has dependency cycle involving '{offender}'")));
        }

        // Priority-queue Kahn: visit 0-indegree nodes in ascending
        // `NodeIndex` order (== list order, because we inserted the
        // nodes in list order). `Reverse` turns `BinaryHeap` (max-heap)
        // into a min-heap keyed by `NodeIndex`.
        let mut indegree: HashMap<NodeIndex, usize> = graph
            .node_indices()
            .map(|n| (n, graph.neighbors_directed(n, Direction::Incoming).count()))
            .collect();
        let mut ready: BinaryHeap<Reverse<NodeIndex>> = indegree
            .iter()
            .filter_map(|(&n, &d)| if d == 0 { Some(Reverse(n)) } else { None })
            .collect();

        let mut rank: HashMap<NodeIndex, usize> = HashMap::with_capacity(self.changes.len());
        let mut next_rank = 0usize;
        while let Some(Reverse(node)) = ready.pop() {
            rank.insert(node, next_rank);
            next_rank += 1;
            for downstream in graph.neighbors_directed(node, Direction::Outgoing) {
                let entry = indegree.get_mut(&downstream).expect("indegree init covers every node");
                *entry -= 1;
                if *entry == 0 {
                    ready.push(Reverse(downstream));
                }
            }
        }

        let mut output: Vec<&PlanChange> = self.changes.iter().collect();
        output.sort_by_key(|c| rank[&idx[c.name.as_str()]]);
        Ok(output)
    }

    /// Move `.specify/plan.yaml` — and, when present, the Layer-3
    /// authoring working directory `.specify/plans/<plan.name>/` —
    /// into the archive directory.
    ///
    /// Semantics (see `rfc-2-execution.md` §L1.G, §L3.B, and §"`specify
    /// plan archive`"):
    ///
    /// 1. Load the plan at `path`.
    /// 2. Collect every entry whose status is non-terminal for archival
    ///    purposes — anything not in `{Done, Skipped}`. If the list is
    ///    non-empty and `force == false`, return
    ///    [`Error::PlanHasOutstandingWork`] carrying those names in
    ///    plan list order. When `force == true`, proceed; the archived
    ///    file preserves the statuses verbatim.
    /// 3. Preflight the on-disk destinations (before any mutation):
    ///    - `<archive_dir>/<plan.name>-<YYYYMMDD>.yaml` must not exist.
    ///    - If `<path>/../plans/<plan.name>/` exists and is a
    ///      directory, `<archive_dir>/<plan.name>-<YYYYMMDD>/` must
    ///      not exist either.
    ///      Any collision errors out before any file or directory is
    ///      moved, so a failure here leaves the working tree untouched.
    /// 4. Create `archive_dir` if missing.
    /// 5. Execute: move `plan.yaml` via [`crate::actions::move_atomic`],
    ///    then (when present) move the working directory via the same
    ///    helper. It dispatches on `src.is_dir()` and does an atomic
    ///    `fs::rename` with a `copy + remove` fallback on `EXDEV`
    ///    (cross-device).
    /// 6. Return `(archived_plan_path, archived_plans_dir)` — the
    ///    second element is `Some` iff a working directory was
    ///    co-moved.
    ///
    /// Takes `&Path` rather than `&self` because it operates on the
    /// file on disk (it re-loads the plan) — archiving is a filesystem
    /// operation, not a mutation of an in-memory `Plan`.
    pub fn archive(
        path: &Path, archive_dir: &Path, force: bool,
    ) -> Result<(PathBuf, Option<PathBuf>), Error> {
        let plan = Plan::load(path)?;

        if !force {
            let entries: Vec<String> = plan
                .changes
                .iter()
                .filter(|c| !matches!(c.status, PlanStatus::Done | PlanStatus::Skipped))
                .map(|c| c.name.clone())
                .collect();
            if !entries.is_empty() {
                return Err(Error::PlanHasOutstandingWork { entries });
            }
        }

        let today = chrono::Utc::now().format("%Y%m%d").to_string();
        let dest_plan = archive_dir.join(format!("{}-{}.yaml", plan.name, today));

        // `path` is `.specify/plan.yaml`; `path.parent()` is
        // `.specify/`, so the working directory sits at
        // `.specify/plans/<plan.name>/` and the operator-authored
        // initiative brief at `.specify/initiative.md`.
        let specify_dir = path.parent();
        let plans_dir = specify_dir.map(|parent| parent.join("plans").join(&plan.name));
        let co_move_plans = plans_dir.as_ref().filter(|p| p.is_dir()).cloned();

        // RFC-3a §"When are `registry.yaml` and `initiative.md`
        // required?": the initiative brief is initiative-scoped so it
        // travels with the archive. `workspace.md` and `slices/` under
        // `.specify/plans/<name>/` are carried by the wholesale working-dir
        // move above. This hook moves `.specify/initiative.md` so operators
        // do not orphan the brief in `.specify/` after archival.
        let initiative_src =
            specify_dir.map(|parent| parent.join("initiative.md")).filter(|p| p.is_file());

        // Either the plans working directory OR the initiative brief
        // being present forces us to materialise the archived
        // `<name>-<date>/` directory so we have somewhere cohesive to
        // put both.
        let dest_plans_dir = (co_move_plans.is_some() || initiative_src.is_some())
            .then(|| archive_dir.join(format!("{}-{}", plan.name, today)));

        // Preflight: refuse both collisions BEFORE any mutation.
        // Running this check up-front means a collision never leaves
        // a half-archived state on disk (plan.yaml moved, working
        // directory still present or vice versa).
        if dest_plan.exists() {
            return Err(Error::Config(format!(
                "archive target '{}' already exists; either move it out of the archive dir (`git mv` is safe — the path is not load-bearing) or wait until tomorrow to re-archive",
                dest_plan.display()
            )));
        }
        if let Some(dest_dir) = &dest_plans_dir
            && dest_dir.exists()
        {
            return Err(Error::Config(format!(
                "archive target '{}' already exists; either move it out of the archive dir (`git mv` is safe — the path is not load-bearing) or wait until tomorrow to re-archive",
                dest_dir.display()
            )));
        }

        std::fs::create_dir_all(archive_dir)?;

        crate::actions::move_atomic(path, &dest_plan)?;
        if let (Some(src), Some(dst)) = (co_move_plans.as_ref(), dest_plans_dir.as_ref()) {
            crate::actions::move_atomic(src, dst)?;
        }
        if let (Some(src), Some(dst)) = (initiative_src.as_ref(), dest_plans_dir.as_ref()) {
            std::fs::create_dir_all(dst)?;
            crate::actions::move_atomic(src, &dst.join("initiative.md"))?;
        }

        Ok((dest_plan, dest_plans_dir))
    }
}

/// Emit one `duplicate-name` error per duplicate *occurrence* (every
/// occurrence after the first).
/// Locate the first entry whose `scope` contains a key not in its
/// own `sources` list, returning the dedicated
/// [`Error::InvalidPlanScopeKey`] variant so callers can surface
/// the stable `scope-key-not-in-sources` wire kind without
/// string-parsing a generic [`PlanValidationResult`]. Mirrors
/// [`check_scope_keys_in_sources`] field-for-field; the two are
/// kept in sync so `Plan::validate` and `Plan::create`/`amend`
/// agree on what "orphan scope key" means.
fn first_orphan_scope_key(changes: &[PlanChange]) -> Option<(String, String)> {
    for entry in changes {
        let sources: HashSet<&str> = entry.sources.iter().map(String::as_str).collect();
        for key in entry.scope.keys() {
            if !sources.contains(key.as_str()) {
                return Some((entry.name.clone(), key.clone()));
            }
        }
    }
    None
}

fn collect_duplicate_names(changes: &[PlanChange]) -> Vec<PlanValidationResult> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for entry in changes {
        if !seen.insert(entry.name.as_str()) {
            out.push(PlanValidationResult {
                level: PlanValidationLevel::Error,
                code: "duplicate-name",
                message: format!("duplicate plan entry name '{}'", entry.name),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

/// Build a `depends_on -> self` DAG and emit one `dependency-cycle`
/// result per cycle (including self-edges). Uses `petgraph::toposort`
/// to detect the existence of a cycle, then `tarjan_scc` to enumerate
/// every strongly-connected component larger than one node plus any
/// self-edges (which are their own SCC of size 1 with a loop).
fn detect_cycles(changes: &[PlanChange]) -> Vec<PlanValidationResult> {
    let mut graph: DiGraph<&str, ()> = DiGraph::new();
    let mut idx = HashMap::new();
    for entry in changes {
        let node = graph.add_node(entry.name.as_str());
        idx.insert(entry.name.as_str(), node);
    }
    let mut has_self_loop = false;
    for entry in changes {
        let to = idx[entry.name.as_str()];
        for dep in &entry.depends_on {
            if let Some(&from) = idx.get(dep.as_str()) {
                graph.add_edge(from, to, ());
                if from == to {
                    has_self_loop = true;
                }
            }
        }
    }

    if toposort(&graph, None).is_ok() && !has_self_loop {
        return Vec::new();
    }

    let mut out = Vec::new();
    for scc in tarjan_scc(&graph) {
        if scc.len() > 1 {
            let mut names: Vec<&str> = scc.iter().map(|&n| graph[n]).collect();
            names.sort_unstable();
            let mut path = names.clone();
            path.push(names[0]);
            out.push(PlanValidationResult {
                level: PlanValidationLevel::Error,
                code: "dependency-cycle",
                message: format!("cycle: {}", path.join(" → ")),
                entry: None,
            });
        } else if scc.len() == 1 {
            let node = scc[0];
            if graph.find_edge(node, node).is_some() {
                let name = graph[node];
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Error,
                    code: "dependency-cycle",
                    message: format!("cycle: {name} → {name}"),
                    entry: None,
                });
            }
        }
    }
    out
}

/// Emit one `unknown-depends-on` error per missing target.
fn check_unknown_depends_on(changes: &[PlanChange]) -> Vec<PlanValidationResult> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.depends_on {
            if !known.contains(target.as_str()) {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Error,
                    code: "unknown-depends-on",
                    message: format!("depends-on references unknown change '{target}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Emit one `unknown-affects` error per missing target.
fn check_unknown_affects(changes: &[PlanChange]) -> Vec<PlanValidationResult> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.affects {
            if !known.contains(target.as_str()) {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Error,
                    code: "unknown-affects",
                    message: format!("affects references unknown change '{target}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Emit one `unknown-source` error per source key not declared at the
/// plan level.
fn check_unknown_sources(plan: &Plan) -> Vec<PlanValidationResult> {
    let mut out = Vec::new();
    for entry in &plan.changes {
        for key in &entry.sources {
            if !plan.sources.contains_key(key) {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Error,
                    code: "unknown-source",
                    message: format!("sources references unknown source key '{key}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Every key in `scope` must also appear in the entry's own
/// `sources` list — the referential-integrity rule from RFC-3a
/// §*How `scope` travels through the pipeline*. The stable
/// diagnostic ID `scope-key-not-in-sources` is contract: the CLI
/// (`specify initiative {create,amend}`) lifts it into
/// [`Error::InvalidPlanScopeKey`] before writing to disk, and
/// `specify-validate` re-emits it at validation time (C04).
fn check_scope_keys_in_sources(changes: &[PlanChange]) -> Vec<PlanValidationResult> {
    let mut out = Vec::new();
    for entry in changes {
        let sources: HashSet<&str> = entry.sources.iter().map(String::as_str).collect();
        for key in entry.scope.keys() {
            if !sources.contains(key.as_str()) {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Error,
                    code: "scope-key-not-in-sources",
                    message: format!(
                        "scope key '{key}' on change '{}' is not declared in sources",
                        entry.name
                    ),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Filesystem-aware sweep for RFC-3a `scope-path-missing`.
///
/// For every change with a non-empty `scope`, iterate the
/// `(source_key, scope_entry)` pairs where `source_key` is present
/// in BOTH the change's own `sources` list AND the plan's top-level
/// `sources` map — anything else is handled upstream
/// (`scope-key-not-in-sources`, `unknown-source`) and skipping it
/// here prevents double-diagnosis on an orphan key.
///
/// For each eligible entry:
///   - When `plan.sources[key]` is a URL or an absolute path, skip
///     the entry silently (see [`is_remote`]). Remote checkouts and
///     developer-local absolute paths aren't meaningfully resolvable
///     from the project root.
///   - Otherwise resolve `source_root = project_dir.join(that_value)`.
///   - For each `include`/`exclude` glob, extract the glob root via
///     [`glob_root`] and emit `scope-path-missing` if
///     `source_root.join(root)` does not exist. An empty root (e.g.
///     `**/*.rs`) degenerates to "the source dir itself" — no
///     finding, because the existence of the source dir is not our
///     concern here (unknown sources surface elsewhere).
///   - For the `manifest` pointer (if present), resolve under
///     `project_dir` directly — per RFC-3a §*Manifest shape* the
///     `.specify/plans/<initiative>/slices/<change>.yaml` path is
///     project-relative, not source-relative.
fn check_scope_paths_exist(plan: &Plan, project_dir: &Path) -> Vec<PlanValidationResult> {
    let mut out = Vec::new();
    for entry in &plan.changes {
        if entry.scope.is_empty() {
            continue;
        }
        let change_sources: HashSet<&str> = entry.sources.iter().map(String::as_str).collect();
        for (key, scope) in &entry.scope {
            if !change_sources.contains(key.as_str()) {
                continue;
            }
            let Some(source_value) = plan.sources.get(key) else {
                continue;
            };
            if is_remote(source_value) || Path::new(source_value).is_absolute() {
                continue;
            }
            let source_root = project_dir.join(source_value);

            for glob in &scope.include {
                push_missing_glob(&mut out, entry, key, &source_root, glob, "include");
            }
            for glob in &scope.exclude {
                push_missing_glob(&mut out, entry, key, &source_root, glob, "exclude");
            }
            if let Some(manifest) = scope.manifest.as_deref() {
                let resolved = project_dir.join(manifest);
                if !resolved.exists() {
                    out.push(PlanValidationResult {
                        level: PlanValidationLevel::Error,
                        code: "scope-path-missing",
                        message: format!(
                            "manifest '{manifest}' for source '{key}' on change '{}' does not exist at '{}'",
                            entry.name,
                            resolved.display(),
                        ),
                        entry: Some(entry.name.clone()),
                    });
                }
            }
        }
    }
    out
}

/// RFC-3a C26: YAML shape + path hygiene for slice manifests under
/// `scope.<key>.manifest` (project-relative). Skips entries already
/// diagnosed by [`check_scope_paths_exist`] as a missing manifest
/// file (`scope-path-missing`). Overlap/orphan resolution uses
/// [`load_manifest_includes`], which treats parse/version failures as
/// an empty include list; this sweep surfaces those problems when a
/// real `project_dir` is available.
fn check_scope_manifest_shapes(plan: &Plan, project_dir: &Path) -> Vec<PlanValidationResult> {
    let mut out = Vec::new();
    for entry in &plan.changes {
        if entry.scope.is_empty() {
            continue;
        }
        let change_sources: HashSet<&str> = entry.sources.iter().map(String::as_str).collect();
        for (key, scope) in &entry.scope {
            if !change_sources.contains(key.as_str()) {
                continue;
            }
            let Some(manifest_rel) = scope.manifest.as_deref() else {
                continue;
            };
            let Some(source_value) = plan.sources.get(key) else {
                continue;
            };
            if is_remote(source_value) || Path::new(source_value).is_absolute() {
                continue;
            }
            let manifest_abs = project_dir.join(manifest_rel);
            if !manifest_abs.is_file() {
                continue;
            }
            let source_root = project_dir.join(source_value);
            if !source_root.is_dir() {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&manifest_abs) else {
                continue;
            };
            let parsed: ScopeManifestV1 = match serde_yaml::from_str(&content) {
                Ok(p) => p,
                Err(e) => {
                    out.push(PlanValidationResult {
                        level: PlanValidationLevel::Error,
                        code: "manifest-invalid",
                        message: format!(
                            "manifest '{manifest_rel}' on change '{}' could not be parsed as v1 scope manifest: {e}",
                            entry.name,
                        ),
                        entry: Some(entry.name.clone()),
                    });
                    continue;
                }
            };
            if parsed.version != 1 {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Error,
                    code: "manifest-invalid",
                    message: format!(
                        "manifest '{manifest_rel}' on change '{}' has unsupported version {} (expected 1)",
                        entry.name, parsed.version,
                    ),
                    entry: Some(entry.name.clone()),
                });
                continue;
            }
            if parsed.include.is_empty() {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Warning,
                    code: "manifest-empty",
                    message: format!(
                        "manifest '{manifest_rel}' on change '{}' has an empty `include` list",
                        entry.name,
                    ),
                    entry: Some(entry.name.clone()),
                });
            }
            for rel in &parsed.include {
                let rel_path = Path::new(rel);
                if rel_path.is_absolute() {
                    out.push(PlanValidationResult {
                        level: PlanValidationLevel::Error,
                        code: "manifest-path-escape",
                        message: format!(
                            "manifest '{manifest_rel}' on change '{}' include entry {rel:?} must be relative to source '{key}'",
                            entry.name,
                        ),
                        entry: Some(entry.name.clone()),
                    });
                    continue;
                }
                if rel_path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                    out.push(PlanValidationResult {
                        level: PlanValidationLevel::Error,
                        code: "manifest-path-escape",
                        message: format!(
                            "manifest '{manifest_rel}' on change '{}' include entry {rel:?} must not contain `..` path segments",
                            entry.name,
                        ),
                        entry: Some(entry.name.clone()),
                    });
                    continue;
                }
                let target = source_root.join(rel_path);
                if !target.is_file() {
                    out.push(PlanValidationResult {
                        level: PlanValidationLevel::Error,
                        code: "scope-path-missing",
                        message: format!(
                            "manifest '{manifest_rel}' include {rel:?} for source '{key}' on change '{}' does not resolve to an existing file at '{}'",
                            entry.name,
                            target.display(),
                        ),
                        entry: Some(entry.name.clone()),
                    });
                }
            }
        }
    }
    out
}

fn push_missing_glob(
    out: &mut Vec<PlanValidationResult>, entry: &PlanChange, key: &str, source_root: &Path,
    glob: &str, kind: &str,
) {
    let root = glob_root(glob);
    if root.is_empty() {
        return;
    }
    let resolved = source_root.join(root);
    if resolved.exists() {
        return;
    }
    out.push(PlanValidationResult {
        level: PlanValidationLevel::Error,
        code: "scope-path-missing",
        message: format!(
            "{kind} glob '{glob}' for source '{key}' on change '{}' does not exist at '{}'",
            entry.name,
            resolved.display(),
        ),
        entry: Some(entry.name.clone()),
    });
}

/// Prefix of `glob` up to the first glob meta-char (`*`, `?`, `[`,
/// `{`), with any trailing `/` trimmed so `src/foo/**` → `src/foo`
/// and `src/` → `src`. A glob with no meta-chars returns unchanged
/// (e.g. a literal path `src/a.ts`). A glob whose first character
/// is already a meta-char (e.g. `**/*.rs`) collapses to `""` and
/// callers are expected to treat that as "source root itself" and
/// skip the check.
fn glob_root(glob: &str) -> &str {
    glob.find(['*', '?', '[', '{']).map(|i| &glob[..i]).unwrap_or(glob).trim_end_matches('/')
}

/// Whether a `sources:` value looks like something we can't
/// validate locally. URL-ish prefixes only — absolute paths are
/// checked separately by the caller via [`Path::is_absolute`].
fn is_remote(source_value: &str) -> bool {
    source_value.starts_with("http://")
        || source_value.starts_with("https://")
        || source_value.starts_with("git@")
        || source_value.starts_with("git+")
        || source_value.starts_with("ssh://")
}

// --- RFC-3a C05: scope-overlap / scope-orphan -------------------------
//
// Overlap and orphan are a pair: both need the concrete set of files
// each change claims under each source, so they share a single sweep
// over `(source_key, change)` pairs. The helpers below — [`is_remote`],
// [`glob_root`] (C04), plus [`resolve_local_source_root`],
// [`walk_source_files`], [`resolve_claimed_files`], and
// [`load_manifest_includes`] (C05) — are deliberately kept inline with
// the other `check_*` functions rather than hoisted into a
// `scope_paths` submodule: the group is cohesive, the per-call depth
// is shallow (three nested helpers at most), and an early submodule
// pulls the `PlanValidationResult`/`PlanValidationLevel` types across
// a module boundary for marginal benefit. C25/C26 (monolith-scale
// lint and full manifest loader) can extract later if the surface
// grows further.

/// Directory names skipped entirely during [`walk_source_files`]. These
/// are universal build/VCS/dependency caches that would otherwise
/// dominate `scope-orphan` output on any real project — pragmatism
/// over purity. Expressed as a conservative allow-list rather than a
/// pattern language; a `.specifyignore` escape hatch is deliberately
/// out of scope for C05.
const ORPHAN_IGNORE_DIR_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".next",
    ".nuxt",
    ".cache",
];

/// Normalise a file path into a forward-slash-delimited string suitable
/// for use as a set key. Mirrors the `relative_key` idiom in
/// `specify-validate` so the two surfaces agree on cross-OS keying.
fn normalise_rel_path(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolve a plan `sources:` value to a concrete local directory, or
/// return `None` if we can't meaningfully walk it from the project
/// root — i.e. the value is a remote URL, an absolute path (treated
/// as developer-local and out of scope), or points at a path that
/// does not exist or is not a directory. The three skip cases are
/// indistinguishable to the caller on purpose: C04's
/// `scope-path-missing` is the dedicated "missing path" diagnostic
/// for scope entries, so C05 must not double-warn.
fn resolve_local_source_root(project_dir: &Path, source_value: &str) -> Option<PathBuf> {
    if is_remote(source_value) || Path::new(source_value).is_absolute() {
        return None;
    }
    let root = project_dir.join(source_value);
    if root.is_dir() { Some(root) } else { None }
}

/// Depth-first walk of `root` collecting every regular file's path
/// (normalised via [`normalise_rel_path`], relative to `root`) into
/// `out`. Skips any directory whose basename appears in
/// [`ORPHAN_IGNORE_DIR_NAMES`]. Errors during `read_dir`/`file_type`
/// are swallowed — validation is advisory and should not fail hard
/// on a transient I/O glitch. Iterative stack-based rather than
/// recursive to stay safe on pathologically deep trees.
fn walk_source_files(root: &Path, out: &mut BTreeSet<String>) {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if ft.is_dir() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str())
                    && ORPHAN_IGNORE_DIR_NAMES.contains(&name)
                {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file()
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.insert(normalise_rel_path(rel));
            }
        }
    }
}

/// RFC-3a C26: on-disk slice manifest referenced from
/// `scope.<key>.manifest` (project-relative path in the plan). v1 is
/// `{ version: 1, include: [ … ] }` with `deny_unknown_fields` — see
/// the `/spec:extract` skill §*Manifest shape*.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeManifestV1 {
    version: u32,
    #[serde(default)]
    include: Vec<String>,
}

/// Best-effort reader backing overlap/orphan resolution ([`resolve_claimed_files`]).
/// Returns `None` on I/O failure, YAML parse failure, or `version != 1`
/// — claimed files are then treated as empty while [`check_scope_manifest_shapes`]
/// emits dedicated diagnostics when a `project_dir` is supplied.
fn load_manifest_includes(manifest_abs: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(manifest_abs).ok()?;
    let shape: ScopeManifestV1 = serde_yaml::from_str(&content).ok()?;
    if shape.version != 1 {
        return None;
    }
    Some(shape.include)
}

/// Expand a single include/exclude glob pattern rooted at
/// `source_root` into normalised keys in `out`. Only regular files
/// that actually exist are added (directories and symlinks are
/// ignored). A malformed pattern or a non-UTF-8 joined path silently
/// contributes nothing — the caller's concern is "what does this
/// change claim on disk", not diagnostic emission.
fn expand_glob_into(source_root: &Path, pattern: &str, out: &mut BTreeSet<String>) {
    let joined = source_root.join(pattern);
    let Some(pattern_str) = joined.to_str() else {
        return;
    };
    let Ok(paths) = glob::glob(pattern_str) else {
        return;
    };
    for path in paths.flatten() {
        if path.is_file()
            && let Ok(rel) = path.strip_prefix(source_root)
        {
            out.insert(normalise_rel_path(rel));
        }
    }
}

/// Compute the set of files `change` claims under source `src_key`.
/// The algorithm, per RFC-3a §*The `scope` field*:
///
///   1. No `scope[src_key]` entry  → claim the whole source tree
///      (minus ignore-list dirs). This is the back-compat default
///      for changes that predate RFC-3a or simply haven't narrowed
///      their view.
///   2. `scope[src_key]` with a `manifest` pointer → load the
///      manifest (project-relative per RFC-3a §*Manifest shape*) and
///      take its `include:` list verbatim as literal source-root-
///      relative file paths. Missing or malformed manifests
///      contribute an empty set (C04/C26 concerns). The manifest
///      form is mutually exclusive with `include`/`exclude` (Scope
///      invariant), so no subtraction is applied here.
///   3. `scope[src_key]` with glob form →
///      - empty `include` + non-empty `exclude` ⇒ "whole tree minus
///        excludes", so start from the walked universe.
///      - empty `include` + empty `exclude` ⇒ treat like (1); the
///        Scope is structurally empty so hands over the whole tree.
///      - non-empty `include` ⇒ start from the union of expanded
///        includes.
///
///      Then subtract every expanded `exclude` glob.
fn resolve_claimed_files(
    change: &PlanChange, src_key: &str, source_root: &Path, project_dir: &Path,
) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    let Some(scope) = change.scope.get(src_key) else {
        walk_source_files(source_root, &mut set);
        return set;
    };

    if let Some(manifest_rel) = scope.manifest.as_deref() {
        let manifest_abs = project_dir.join(manifest_rel);
        if let Some(includes) = load_manifest_includes(&manifest_abs) {
            for rel in includes {
                let abs = source_root.join(&rel);
                if abs.is_file() {
                    set.insert(normalise_rel_path(Path::new(&rel)));
                }
            }
        }
        return set;
    }

    if scope.include.is_empty() {
        walk_source_files(source_root, &mut set);
    } else {
        for pat in &scope.include {
            expand_glob_into(source_root, pat, &mut set);
        }
    }

    if !scope.exclude.is_empty() {
        let mut removed: BTreeSet<String> = BTreeSet::new();
        for pat in &scope.exclude {
            expand_glob_into(source_root, pat, &mut removed);
        }
        for rel in &removed {
            set.remove(rel);
        }
    }

    set
}

/// Combined sweep emitting RFC-3a C05's `scope-overlap` and
/// `scope-orphan` warnings.
///
/// Guard: fires only when at least one change has a non-empty
/// `scope` map. Plans authored before RFC-3a therefore get zero new
/// findings, preserving back-compat.
///
/// For every source key in `plan.sources` that resolves to a local
/// directory:
///   - walk the source tree (respecting [`ORPHAN_IGNORE_DIR_NAMES`])
///     to build the orphan universe;
///   - for every change that lists the key in its own `sources`,
///     compute [`resolve_claimed_files`];
///   - emit one `scope-overlap` per file claimed by ≥2 changes,
///     naming the claimants in sorted order; `entry` is `None` since
///     overlap is cross-entry;
///   - emit one `scope-orphan` per file in the universe not claimed
///     by anyone. TODO(future): real monoliths may want grouped
///     reporting; C05 ships the literal per-file form and relies on
///     the ignore list for noise control.
///
/// Findings are sorted by (code, message) so golden fixtures and
/// filter-by-code tests are stable.
fn check_scope_coverage(plan: &Plan, project_dir: &Path) -> Vec<PlanValidationResult> {
    if plan.changes.iter().all(|c| c.scope.is_empty()) {
        return Vec::new();
    }
    let mut out: Vec<PlanValidationResult> = Vec::new();

    for (src_key, src_value) in &plan.sources {
        let Some(source_root) = resolve_local_source_root(project_dir, src_value) else {
            continue;
        };

        let mut universe: BTreeSet<String> = BTreeSet::new();
        walk_source_files(&source_root, &mut universe);

        let mut claims: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for change in &plan.changes {
            if !change.sources.iter().any(|s| s == src_key) {
                continue;
            }
            let files = resolve_claimed_files(change, src_key, &source_root, project_dir);
            for f in files {
                claims.entry(f).or_default().insert(change.name.clone());
            }
        }

        for (path, names) in &claims {
            if names.len() > 1 {
                let joined = names.iter().cloned().collect::<Vec<_>>().join(", ");
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Warning,
                    code: "scope-overlap",
                    message: format!("{src_key}:{path} is claimed by: {joined}"),
                    entry: None,
                });
            }
        }

        for path in &universe {
            if !claims.contains_key(path) {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Warning,
                    code: "scope-orphan",
                    message: format!("{src_key}:{path} claimed by no change"),
                    entry: None,
                });
            }
        }
    }

    out.sort_by(|a, b| a.code.cmp(b.code).then_with(|| a.message.cmp(&b.message)));
    out
}

// --- RFC-3a C25: scope-missing-on-monolith ---------------------------
//
// Warning-level lint that fires when `/spec:analyze` has classified
// a source as monolith-scale yet the change claims it whole. Drives
// operators toward narrowing via `specify initiative amend
// --scope-include <key>=<glob>` before `/spec:extract` overflows
// context at define time. See `rfcs/rfc-3a-monoliths.md` §*Validation*.

/// LOC ceiling above which a source is considered monolith-scale for
/// the Omnia schema. Matched on `AnalyzeMetadata::loc`.
///
/// Hardcoded for v1. RFC-3a §*Validation* earmarks a future
/// schema-owned threshold slot — the metadata file will grow a
/// discriminator and this constant will move into a per-schema
/// config lookup. C25 leaves that extraction to a separate chunk;
/// changing the number here is the whole v1 override path.
const MONOLITH_LOC_THRESHOLD: u64 = 10_000;

/// Module-count ceiling above which a source is considered
/// monolith-scale for the Omnia schema. Matched on
/// `AnalyzeMetadata::module_count`. See [`MONOLITH_LOC_THRESHOLD`]
/// for the v1-vs-future-schema-owned rationale.
const MONOLITH_MODULE_COUNT_THRESHOLD: u32 = 20;

/// Parsed subset of `.specify/plans/<plan.name>/analyze/<key>/metadata.json`
/// as documented in `plugins/spec/skills/analyze/SKILL.md`
/// §*Structural metadata*. The loader is tolerant of extra fields
/// (only the ones we read are captured here); `version: 1` is the
/// only supported shape for v1.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct AnalyzeMetadata {
    version: u32,
    loc: u64,
    module_count: u32,
}

/// Read `metadata.json` at `path`, returning `None` for any reason
/// that should silently skip the monolith-scale lint:
///   - file does not exist (small-legacy / greenfield never produce it).
///   - file cannot be read (permission error etc.).
///   - file is not valid JSON.
///   - file's `version` is not `1` (future shape the v1 lint can't
///     interpret — future chunk will widen this).
///
/// Malformed-metadata is silent for C25 on purpose — the RFC's
/// `invalid-analyze-metadata` error lives in a later chunk. The
/// file is skill-generated, not hand-authored, so the failure mode
/// is rare enough to defer without loss.
fn load_analyze_metadata(path: &Path) -> Option<AnalyzeMetadata> {
    let content = std::fs::read_to_string(path).ok()?;
    let meta: AnalyzeMetadata = serde_json::from_str(&content).ok()?;
    if meta.version != 1 {
        return None;
    }
    Some(meta)
}

/// Compose the canonical analyze-metadata path for `(plan_name, source_key)`
/// under `project_dir`. Pinned in C20 and mirrored by the
/// `/spec:analyze` skill; changing the layout means updating both
/// ends.
fn analyze_metadata_path(project_dir: &Path, plan_name: &str, source_key: &str) -> PathBuf {
    project_dir
        .join(".specify")
        .join("plans")
        .join(plan_name)
        .join("analyze")
        .join(source_key)
        .join("metadata.json")
}

/// Apply the Omnia default threshold. Exposed as a named fn rather
/// than inlined so the constants are reviewable in one place and a
/// future schema-owned threshold lookup slots in without touching
/// the caller.
fn is_monolith_scale(meta: &AnalyzeMetadata) -> bool {
    meta.loc >= MONOLITH_LOC_THRESHOLD || meta.module_count >= MONOLITH_MODULE_COUNT_THRESHOLD
}

/// Emit a `scope-missing-on-monolith` warning for every
/// `(change, source_key)` pair where
///   - the source has a monolith-scale `metadata.json`, AND
///   - the change lists that key in its `sources`, AND
///   - the change has no `scope.<key>` entry.
///
/// The remediation in the diagnostic uses C03's finalized
/// `--scope-include <key>=<glob>` syntax (the RFC example predates
/// C03 and drops the `<key>=`; match the binary's actual flag
/// shape — accuracy over byte-identity).
fn check_scope_missing_on_monolith(plan: &Plan, project_dir: &Path) -> Vec<PlanValidationResult> {
    let mut out = Vec::new();
    for change in &plan.changes {
        for src_key in &change.sources {
            if change.scope.contains_key(src_key) {
                continue;
            }
            let path = analyze_metadata_path(project_dir, &plan.name, src_key);
            let Some(meta) = load_analyze_metadata(&path) else {
                continue;
            };
            if !is_monolith_scale(&meta) {
                continue;
            }
            let loc_k = meta.loc / 1_000;
            let message = format!(
                "change `{name}` has sources[`{key}`] classified as \
                 monolith-scale by /spec:analyze ({mods} modules, {loc}k LOC) \
                 and no scope entry — define-time /spec:extract may overflow \
                 context. Run `specify initiative amend {name} --scope-include \
                 {key}=<glob>` to narrow the slice.",
                name = change.name,
                key = src_key,
                mods = meta.module_count,
                loc = loc_k,
            );
            out.push(PlanValidationResult {
                level: PlanValidationLevel::Warning,
                code: "scope-missing-on-monolith",
                message,
                entry: Some(change.name.clone()),
            });
        }
    }
    out
}

/// When more than one entry is `in-progress`, emit one result per
/// offending entry so every offender is surfaceable in the UI.
fn check_single_in_progress(changes: &[PlanChange]) -> Vec<PlanValidationResult> {
    let offenders: Vec<&PlanChange> =
        changes.iter().filter(|c| c.status == PlanStatus::InProgress).collect();
    if offenders.len() <= 1 {
        return Vec::new();
    }
    offenders
        .into_iter()
        .map(|c| PlanValidationResult {
            level: PlanValidationLevel::Error,
            code: "multiple-in-progress",
            message: "multiple in-progress entries: at most one allowed per plan".to_string(),
            entry: Some(c.name.clone()),
        })
        .collect()
}

/// Plan-to-change directory consistency:
///   - Warn on orphan subdirectories (no matching plan entry).
///   - Warn when an `in-progress` plan entry has no matching directory.
fn check_changes_dir_consistency(plan: &Plan, changes_dir: &Path) -> Vec<PlanValidationResult> {
    let mut out = Vec::new();
    let declared: HashSet<&str> = plan.changes.iter().map(|c| c.name.as_str()).collect();

    let Ok(read_dir) = std::fs::read_dir(changes_dir) else {
        return out;
    };
    let mut dir_names: Vec<String> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        dir_names.push(name.to_string());
    }
    dir_names.sort();

    for name in &dir_names {
        if !declared.contains(name.as_str()) {
            out.push(PlanValidationResult {
                level: PlanValidationLevel::Warning,
                code: "orphan-change-dir",
                message: format!("change directory '{name}' has no plan entry"),
                entry: Some(name.clone()),
            });
        }
    }

    for entry in &plan.changes {
        if entry.status == PlanStatus::InProgress {
            let candidate = changes_dir.join(&entry.name);
            if !candidate.is_dir() {
                out.push(PlanValidationResult {
                    level: PlanValidationLevel::Warning,
                    code: "missing-change-dir-for-in-progress",
                    message: format!(
                        "in-progress entry '{}' has no change directory (may briefly be absent during phase start-up)",
                        entry.name
                    ),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::tempdir;

    /// The 10 legal edges from `rfc-2-execution.md` §"Transition Rules".
    /// Kept here (not on `PlanStatus`) so the production matcher and the
    /// test oracle are independent representations of the same table.
    fn allowed_edges() -> HashSet<(PlanStatus, PlanStatus)> {
        use PlanStatus::*;
        let mut set = HashSet::new();
        set.insert((Pending, InProgress));
        set.insert((Pending, Blocked));
        set.insert((Pending, Skipped));
        set.insert((InProgress, Done));
        set.insert((InProgress, Failed));
        set.insert((InProgress, Blocked));
        set.insert((Blocked, Pending));
        set.insert((Failed, Pending));
        set.insert((Failed, Skipped));
        set.insert((Skipped, Pending));
        set
    }

    #[test]
    fn every_legal_edge_transitions_successfully() {
        for (from, to) in allowed_edges() {
            assert!(
                from.can_transition_to(&to),
                "{from:?} -> {to:?} should be allowed by can_transition_to"
            );
            let result = from
                .transition(to)
                .unwrap_or_else(|e| panic!("expected {from:?} -> {to:?} to succeed, got {e:?}"));
            assert_eq!(result, to);
        }
    }

    #[test]
    fn done_is_terminal() {
        for &t in &PlanStatus::ALL {
            assert!(!PlanStatus::Done.can_transition_to(&t), "Done must not allow -> {t:?}");
        }
    }

    #[test]
    fn illegal_edges_rejected() {
        use PlanStatus::*;
        let cases: &[(PlanStatus, PlanStatus)] = &[
            (Done, Pending),
            (Done, InProgress),
            (Done, Failed),
            (Pending, Done),
            (Pending, Failed),
            (Skipped, Failed),
            (InProgress, Pending),
            (InProgress, Skipped),
            (Blocked, Failed),
            (Pending, Pending),
            (InProgress, InProgress),
            (Done, Done),
            (Blocked, Blocked),
            (Failed, Failed),
            (Skipped, Skipped),
        ];

        for &(from, to) in cases {
            assert!(
                !from.can_transition_to(&to),
                "{from:?} -> {to:?} must be rejected by can_transition_to"
            );
            let err = from.transition(to).expect_err(&format!("{from:?} -> {to:?} should be Err"));
            match err {
                Error::PlanTransition { from: f, to: t } => {
                    assert_eq!(f, format!("{from:?}"), "from payload mismatch");
                    assert_eq!(t, format!("{to:?}"), "to payload mismatch");
                }
                other => panic!("expected Error::PlanTransition, got {other:?}"),
            }
        }
    }

    #[test]
    fn exhaustive_table_matches_allowed_set() {
        let allowed = allowed_edges();
        for &from in &PlanStatus::ALL {
            for &to in &PlanStatus::ALL {
                let expected = allowed.contains(&(from, to));
                let actual = from.can_transition_to(&to);
                assert_eq!(
                    actual, expected,
                    "({from:?}) -> ({to:?}): expected allowed={expected}, got {actual}"
                );
            }
        }
    }

    #[test]
    fn error_carries_from_and_to() {
        let err = PlanStatus::Done
            .transition(PlanStatus::Pending)
            .expect_err("Done -> Pending must error");
        match err {
            Error::PlanTransition { from, to } => {
                assert_eq!(from, "Done");
                assert_eq!(to, "Pending");
            }
            other => panic!("expected Error::PlanTransition, got {other:?}"),
        }
    }

    /// Verbatim reproduction of the `rfc-2-execution.md` §"The Plan"
    /// fixture. Intentionally predates RFC-3a and carries no `scope:`
    /// map on any entry: exercises the back-compat round-trip guarantee
    /// that pre-RFC-3a plans serialize byte-identically after being
    /// parsed into the current `PlanChange` (`scope` is
    /// `skip_serializing_if = BTreeMap::is_empty`). RFC-3a's optional
    /// `planChange.scope` field is covered by dedicated round-trip
    /// tests further down in this module.
    const RFC_EXAMPLE_YAML: &str = r#"name: platform-v2
sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git
changes:
  - name: user-registration
    sources: [monolith]
    status: done
  - name: email-verification
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress
  - name: registration-duplicate-email-crash
    affects: [user-registration]
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending
  - name: notification-preferences
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending
  - name: extract-shared-validation
    affects: [user-registration, email-verification]
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
    depends-on: [email-verification]
    status: pending
  - name: product-catalog
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending
  - name: shopping-cart
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending
  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
  - name: checkout-ui
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
"#;

    #[test]
    fn plan_roundtrips_rfc_example() {
        let original: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let rendered = serde_yaml::to_string(&original).expect("serialize plan");
        let reparsed: Plan = serde_yaml::from_str(&rendered).expect("reparse rendered plan");
        assert_eq!(original, reparsed, "plan should survive a serialize/parse round-trip");

        assert_eq!(original.name, "platform-v2");
        assert_eq!(original.sources.len(), 4);
        assert_eq!(original.changes.len(), 9);
        assert_eq!(original.changes[0].status, PlanStatus::Done);
        assert_eq!(original.changes[1].status, PlanStatus::InProgress);
        assert_eq!(original.changes[7].status, PlanStatus::Failed);
        assert!(original.changes[7].status_reason.is_some());
    }

    #[test]
    fn kebab_case_serialization() {
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "entry-one".to_string(),
                status: PlanStatus::InProgress,
                depends_on: vec!["entry-zero".to_string()],
                affects: vec![],
                sources: vec![],
                scope: BTreeMap::new(),
                description: None,
                status_reason: Some("awaiting upstream fix".to_string()),
            }],
        };
        let yaml = serde_yaml::to_string(&plan).expect("serialize plan");
        assert!(yaml.contains("depends-on:"), "expected kebab-case depends-on in:\n{yaml}");
        assert!(
            yaml.contains("status: in-progress"),
            "expected kebab-case enum value in-progress in:\n{yaml}"
        );
        assert!(yaml.contains("status-reason:"), "expected kebab-case status-reason in:\n{yaml}");
        assert!(!yaml.contains("depends_on"), "snake_case depends_on leaked into output:\n{yaml}");
        assert!(
            !yaml.contains("status_reason"),
            "snake_case status_reason leaked into output:\n{yaml}"
        );
    }

    #[test]
    fn missing_optional_fields_deserialize_with_defaults() {
        let yaml = "name: foo\nchanges: []\n";
        let plan: Plan = serde_yaml::from_str(yaml).expect("parse minimal plan");
        assert_eq!(plan.name, "foo");
        assert!(plan.sources.is_empty(), "sources should default to empty map");
        assert!(plan.changes.is_empty(), "changes should be empty");
    }

    #[test]
    fn status_reason_roundtrips_on_failed_entry() {
        let yaml = r#"name: demo
changes:
  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
"#;
        let plan: Plan = serde_yaml::from_str(yaml).expect("parse");
        let entry = &plan.changes[0];
        assert_eq!(entry.status, PlanStatus::Failed);
        let reason = entry.status_reason.as_deref().expect("status_reason populated");
        assert!(
            reason.contains("Type mismatch"),
            "status_reason should preserve folded text, got: {reason:?}"
        );

        let rendered = serde_yaml::to_string(&plan).expect("serialize");
        let reparsed: Plan = serde_yaml::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed);
        assert_eq!(
            reparsed.changes[0].status_reason, entry.status_reason,
            "status_reason should be byte-identical after round-trip"
        );
    }

    #[test]
    fn save_then_load_roundtrips_rfc_example() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let original: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        original.save(&path).expect("save ok");
        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, original, "full plan should round-trip through save -> load");
    }

    #[test]
    fn save_creates_new_file_and_emits_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let plan = Plan {
            name: "init".to_string(),
            sources: BTreeMap::new(),
            changes: vec![],
        };
        plan.save(&path).expect("save ok");

        let bytes = std::fs::read(&path).expect("read ok");
        assert!(!bytes.is_empty(), "saved file should not be empty");
        assert_eq!(*bytes.last().unwrap(), b'\n', "saved file should end with a newline");

        let content = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            content.contains("name: init"),
            "file should contain `name: init`, got:\n{content}"
        );
    }

    #[test]
    fn save_overwrites_existing_file_atomically() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "garbage that should be overwritten").expect("write garbage");

        let plan = Plan {
            name: "fresh".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "only-entry".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec![],
                scope: BTreeMap::new(),
                description: None,
                status_reason: None,
            }],
        };
        plan.save(&path).expect("save ok");

        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, plan, "loaded plan should equal saved plan");

        let raw = std::fs::read_to_string(&path).expect("read ok");
        assert!(
            !raw.contains("garbage"),
            "pre-existing garbage content should be gone, got:\n{raw}"
        );
    }

    #[test]
    fn load_missing_file_returns_config_error() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.yaml");
        let err = Plan::load(&path).expect_err("expected error on missing file");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("plan.yaml not found"),
                    "message should mention `plan.yaml not found`, got: {msg}"
                );
                assert!(
                    msg.contains(&path.display().to_string()),
                    "message should include the missing path, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn load_tolerates_missing_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "name: foo\nchanges: []").expect("write without trailing newline");
        let plan = Plan::load(&path).expect("load ok");
        assert_eq!(plan.name, "foo");
        assert!(plan.changes.is_empty());
    }

    #[test]
    fn save_writes_kebab_case_on_disk() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "entry-one".to_string(),
                status: PlanStatus::InProgress,
                depends_on: vec!["foo".to_string()],
                affects: vec![],
                sources: vec![],
                scope: BTreeMap::new(),
                description: None,
                status_reason: None,
            }],
        };
        plan.save(&path).expect("save ok");

        let content = std::fs::read_to_string(&path).expect("read ok");
        assert!(
            content.contains("depends-on:"),
            "expected kebab-case `depends-on:`, got:\n{content}"
        );
        assert!(
            content.contains("status: in-progress"),
            "expected kebab-case enum value `in-progress`, got:\n{content}"
        );
        assert!(
            !content.contains("depends_on"),
            "snake_case `depends_on` leaked onto disk, got:\n{content}"
        );
        assert!(
            !content.contains("in_progress"),
            "snake_case `in_progress` leaked onto disk, got:\n{content}"
        );
    }

    // --- RFC-3a: PlanChange.scope round-trips and invariant ---------

    /// Build a single-change plan whose one entry carries the given
    /// `scope` map. Keeps the RFC-3a scope tests terse and focused on
    /// the serde surface of [`Scope`]/`PlanChange::scope`.
    fn plan_with_scope(change_name: &str, scope: BTreeMap<String, Scope>) -> Plan {
        Plan {
            name: "scope-demo".to_string(),
            sources: BTreeMap::from([("monolith".to_string(), "/path/to/legacy".to_string())]),
            changes: vec![PlanChange {
                name: change_name.to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec!["monolith".to_string()],
                scope,
                description: None,
                status_reason: None,
            }],
        }
    }

    #[test]
    fn scope_field_round_trips_byte_for_byte() {
        let mut scope = BTreeMap::new();
        scope.insert(
            "monolith".to_string(),
            Scope::try_new(
                vec!["src/ingest/**".to_string()],
                vec!["src/ingest/_deprecated/**".to_string()],
                None,
            )
            .expect("valid include/exclude scope"),
        );
        let plan = plan_with_scope("ingest", scope);

        let yaml = serde_yaml::to_string(&plan).expect("serialize plan with scope");
        let reparsed: Plan = serde_yaml::from_str(&yaml).expect("parse scoped plan");
        let reserialized = serde_yaml::to_string(&reparsed).expect("reserialize");
        assert_eq!(plan, reparsed, "scope round-trip must be value-stable");
        assert_eq!(
            yaml, reserialized,
            "scope round-trip must be byte-stable:\noriginal:\n{yaml}\nreserialized:\n{reserialized}"
        );
        assert!(yaml.contains("scope:"), "serialized YAML should carry scope: key:\n{yaml}");
        assert!(yaml.contains("include:"), "serialized YAML should carry include: key:\n{yaml}");
        assert!(yaml.contains("exclude:"), "serialized YAML should carry exclude: key:\n{yaml}");
        assert!(
            !yaml.contains("manifest:"),
            "unused manifest: field must not leak into the YAML:\n{yaml}"
        );
    }

    #[test]
    fn manifest_only_scope_round_trips_without_include_or_exclude() {
        let mut scope = BTreeMap::new();
        scope.insert(
            "monolith".to_string(),
            Scope::try_new(vec![], vec![], Some("slices/ingest.yaml".to_string()))
                .expect("valid manifest-only scope"),
        );
        let plan = plan_with_scope("ingest", scope);

        let yaml = serde_yaml::to_string(&plan).expect("serialize manifest-only scope");
        let reparsed: Plan = serde_yaml::from_str(&yaml).expect("reparse manifest-only scope");
        assert_eq!(plan, reparsed);
        assert!(yaml.contains("manifest: slices/ingest.yaml"), "expected manifest line:\n{yaml}");
        assert!(!yaml.contains("include:"), "include: must be omitted when empty:\n{yaml}");
        assert!(!yaml.contains("exclude:"), "exclude: must be omitted when empty:\n{yaml}");
    }

    #[test]
    fn empty_scope_map_skipped_on_serialize() {
        let plan = plan_with_scope("ingest", BTreeMap::new());
        let yaml = serde_yaml::to_string(&plan).expect("serialize scopeless plan");
        assert!(
            !yaml.contains("scope:"),
            "empty scope map must not emit a scope: key (back-compat):\n{yaml}"
        );
        let reparsed: Plan = serde_yaml::from_str(&yaml).expect("reparse");
        assert_eq!(plan, reparsed);
    }

    #[test]
    fn plan_without_scope_round_trips_unchanged() {
        let original: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let rendered = serde_yaml::to_string(&original).expect("serialize plan");
        assert!(
            !rendered.contains("scope:"),
            "RFC-2 fixture predates RFC-3a and must not acquire a scope: key after round-trip:\n{rendered}"
        );
        let reparsed: Plan = serde_yaml::from_str(&rendered).expect("reparse");
        assert_eq!(original, reparsed);
    }

    #[test]
    fn manifest_and_include_is_rejected() {
        let yaml = r#"name: p
changes:
  - name: c
    status: pending
    sources: [monolith]
    scope:
      monolith:
        manifest: slices/c.yaml
        include: ["src/**"]
"#;
        let err = serde_yaml::from_str::<Plan>(yaml)
            .expect_err("manifest + include must be rejected on parse");
        let msg = err.to_string();
        assert!(
            msg.contains("manifest") && msg.contains("include"),
            "parse error should name both rejected fields, got: {msg}"
        );
    }

    #[test]
    fn manifest_and_exclude_is_rejected() {
        let yaml = r#"name: p
changes:
  - name: c
    status: pending
    sources: [monolith]
    scope:
      monolith:
        manifest: slices/c.yaml
        exclude: ["src/legacy/**"]
"#;
        let err = serde_yaml::from_str::<Plan>(yaml)
            .expect_err("manifest + exclude must be rejected on parse");
        let msg = err.to_string();
        assert!(
            msg.contains("manifest") && msg.contains("exclude"),
            "parse error should name both rejected fields, got: {msg}"
        );
    }

    #[test]
    fn scope_try_new_enforces_invariant() {
        Scope::try_new(vec!["a/**".into()], vec!["b/**".into()], None)
            .expect("include+exclude is a valid combination");
        Scope::try_new(vec![], vec![], Some("m.yaml".into()))
            .expect("manifest-only is a valid combination");
        Scope::try_new(vec![], vec![], None).expect("empty scope is valid (no-op)");

        let err = Scope::try_new(vec!["a/**".into()], vec![], Some("m.yaml".into()))
            .expect_err("manifest + include must be rejected");
        match err {
            Error::InvalidPlanScope(msg) => {
                assert!(
                    msg.contains("manifest") && msg.contains("include"),
                    "message should name both fields, got: {msg}"
                );
            }
            other => panic!("expected Error::InvalidPlanScope, got {other:?}"),
        }

        let err = Scope::try_new(vec![], vec!["x/**".into()], Some("m.yaml".into()))
            .expect_err("manifest + exclude must be rejected");
        assert!(matches!(err, Error::InvalidPlanScope(_)), "got {err:?}");
    }

    #[test]
    fn empty_scope_entry_is_legal_and_round_trips() {
        // Schema permits `scope.<k>: {}` as a no-op. Rust must
        // agree: an empty `Scope` parses and re-serializes cleanly.
        let yaml = r#"name: p
changes:
  - name: c
    status: pending
    sources: [monolith]
    scope:
      monolith: {}
"#;
        let plan: Plan = serde_yaml::from_str(yaml).expect("empty scope entry must parse");
        let entry = plan.changes[0].scope.get("monolith").expect("monolith scope entry exists");
        assert_eq!(entry, &Scope::default());
    }

    #[test]
    fn scope_rejects_unknown_fields() {
        // Mirrors `additionalProperties: false` in plan.schema.json's
        // `scopeEntry`. Without this guard, typos like `manifests:`
        // would silently succeed and drift the two surfaces apart.
        let yaml = r#"name: p
changes:
  - name: c
    status: pending
    sources: [monolith]
    scope:
      monolith:
        manifests: slices/c.yaml
"#;
        let err =
            serde_yaml::from_str::<Plan>(yaml).expect_err("unknown scope field must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("manifests") || msg.contains("unknown field"),
            "error should mention the unknown field, got: {msg}"
        );
    }

    // --- RFC-3a C03: PlanChangePatch.scope semantics ------------------

    /// Typing guarantee: `PlanChangePatch` must not grow a `status`
    /// (or `status_reason`) field. Status transitions route through
    /// [`Plan::transition`] — see the module-level single-writer
    /// note. Checking the field set via [`std::mem::size_of`] is
    /// coarse but cheap; the real invariant is the struct
    /// definition in this file.
    #[test]
    fn plan_change_patch_has_no_status_fields() {
        let patch = PlanChangePatch::default();
        // If someone adds `status: Option<PlanStatus>` to
        // `PlanChangePatch`, this line stops compiling. That is the
        // guarantee.
        let PlanChangePatch {
            depends_on: _,
            affects: _,
            sources: _,
            description: _,
            scope: _,
        } = patch;
    }

    #[test]
    fn amend_without_scope_patch_preserves_existing_scope() {
        let mut scope = BTreeMap::new();
        scope.insert(
            "monolith".to_string(),
            Scope::try_new(vec!["src/a/**".into()], vec![], None).expect("valid scope"),
        );
        let mut plan = plan_with_scope("ingest", scope.clone());
        plan.sources.insert("monolith".into(), "/tmp/ws".into());
        plan.changes[0].sources = vec!["monolith".into()];

        plan.amend(
            "ingest",
            PlanChangePatch {
                depends_on: Some(vec![]),
                ..Default::default()
            },
        )
        .expect("keyless scope patch must succeed");

        assert_eq!(
            plan.changes[0].scope, scope,
            "a patch with empty scope map must leave existing scope untouched"
        );
    }

    #[test]
    fn amend_scope_replace_per_key_is_wholesale() {
        let mut scope = BTreeMap::new();
        scope.insert(
            "monolith".to_string(),
            Scope::try_new(vec!["src/a/**".into(), "src/b/**".into()], vec![], None)
                .expect("initial scope"),
        );
        let mut plan = plan_with_scope("ingest", scope);
        plan.sources.insert("monolith".into(), "/tmp/ws".into());
        plan.changes[0].sources = vec!["monolith".into()];

        let mut patch_scope = BTreeMap::new();
        patch_scope.insert(
            "monolith".to_string(),
            Some(Scope::try_new(vec!["src/new/**".into()], vec![], None).expect("new scope")),
        );
        plan.amend(
            "ingest",
            PlanChangePatch {
                scope: patch_scope,
                ..Default::default()
            },
        )
        .expect("scope replace must succeed");

        assert_eq!(plan.changes[0].scope["monolith"].include, vec!["src/new/**".to_string()]);
    }

    #[test]
    fn amend_scope_rm_drops_the_entry() {
        let mut scope = BTreeMap::new();
        scope.insert(
            "monolith".to_string(),
            Scope::try_new(vec!["src/a/**".into()], vec![], None).expect("initial scope"),
        );
        let mut plan = plan_with_scope("ingest", scope);
        plan.sources.insert("monolith".into(), "/tmp/ws".into());
        plan.changes[0].sources = vec!["monolith".into()];

        let mut patch_scope = BTreeMap::new();
        patch_scope.insert("monolith".to_string(), None);
        plan.amend(
            "ingest",
            PlanChangePatch {
                scope: patch_scope,
                ..Default::default()
            },
        )
        .expect("scope-rm must succeed");

        assert!(plan.changes[0].scope.is_empty(), "scope map should be empty after removal");
    }

    #[test]
    fn scope_key_not_in_sources_surfaces_dedicated_variant() {
        let mut plan = Plan {
            name: "p".into(),
            sources: {
                let mut m = BTreeMap::new();
                m.insert("monolith".into(), "/tmp/ws".into());
                m
            },
            changes: vec![],
        };
        let mut scope = BTreeMap::new();
        scope.insert(
            "orders".to_string(),
            Scope::try_new(vec!["src/**".into()], vec![], None).expect("valid scope"),
        );
        let new_change = PlanChange {
            name: "ingest".into(),
            status: PlanStatus::Pending,
            depends_on: vec![],
            affects: vec![],
            sources: vec!["monolith".into()],
            scope,
            description: None,
            status_reason: None,
        };

        let err = plan.create(new_change).expect_err("orphan scope key must be rejected");
        match err {
            Error::InvalidPlanScopeKey { change, key } => {
                assert_eq!(change, "ingest");
                assert_eq!(key, "orders");
            }
            other => panic!("expected Error::InvalidPlanScopeKey, got {other:?}"),
        }
        assert!(plan.changes.is_empty(), "rejected create must roll back");
    }

    #[test]
    fn validate_reports_scope_key_not_in_sources() {
        let mut scope = BTreeMap::new();
        scope.insert(
            "orders".to_string(),
            Scope::try_new(vec!["src/**".into()], vec![], None).expect("valid scope"),
        );
        let mut plan = plan_with_scope("ingest", scope);
        plan.sources.insert("monolith".into(), "/tmp/ws".into());
        plan.changes[0].sources = vec!["monolith".into()];

        let results = plan.validate(None, None);
        let finding = results
            .iter()
            .find(|r| r.code == "scope-key-not-in-sources")
            .expect("expected scope-key-not-in-sources finding");
        assert_eq!(finding.level, PlanValidationLevel::Error);
        assert_eq!(finding.entry.as_deref(), Some("ingest"));
    }

    // --- RFC-3a C04: scope-path-missing --------------------------------

    /// Build a plan whose `monolith` source points to `source_rel`
    /// (a project-relative path) and whose sole change `ingest` has
    /// the given scope under key `monolith`. Keeps the path-existence
    /// tests terse and focused on the scope entry shape.
    fn plan_with_source_and_scope(source_rel: &str, scope: Scope) -> Plan {
        let mut scope_map = BTreeMap::new();
        scope_map.insert("monolith".to_string(), scope);
        Plan {
            name: "scope-paths".to_string(),
            sources: BTreeMap::from([("monolith".to_string(), source_rel.to_string())]),
            changes: vec![PlanChange {
                name: "ingest".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec!["monolith".to_string()],
                scope: scope_map,
                description: None,
                status_reason: None,
            }],
        }
    }

    #[test]
    fn validate_reports_scope_path_missing_for_include_glob() {
        let tmp = tempdir().expect("tempdir");
        // `legacy/` exists but `legacy/src/ingest/` does not —
        // the include root `src/ingest` must fail to resolve.
        std::fs::create_dir_all(tmp.path().join("legacy")).expect("mkdir legacy");
        let plan = plan_with_source_and_scope(
            "legacy",
            Scope::try_new(vec!["src/ingest/**".into()], vec![], None).expect("scope"),
        );

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-path-missing").collect();
        assert_eq!(hits.len(), 1, "expected one scope-path-missing, got {results:#?}");
        assert_eq!(hits[0].level, PlanValidationLevel::Error);
        assert_eq!(hits[0].entry.as_deref(), Some("ingest"));
        assert!(
            hits[0].message.contains("src/ingest/**"),
            "message must reference the offending glob: {}",
            hits[0].message
        );
        assert!(
            hits[0].message.contains("monolith"),
            "message must reference the source key: {}",
            hits[0].message
        );
    }

    #[test]
    fn validate_reports_scope_path_missing_for_exclude_glob() {
        let tmp = tempdir().expect("tempdir");
        // Make the include root real so only the exclude root is missing —
        // that isolates the failure mode we care about.
        std::fs::create_dir_all(tmp.path().join("legacy/src/ingest")).expect("mkdir");
        let plan = plan_with_source_and_scope(
            "legacy",
            Scope::try_new(
                vec!["src/ingest/**".into()],
                vec!["src/ingest/_deprecated/**".into()],
                None,
            )
            .expect("scope"),
        );

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-path-missing").collect();
        assert_eq!(hits.len(), 1, "expected one scope-path-missing, got {results:#?}");
        assert!(
            hits[0].message.contains("src/ingest/_deprecated/**"),
            "must name the offending exclude glob: {}",
            hits[0].message
        );
        assert!(
            hits[0].message.contains("exclude"),
            "must identify kind=exclude: {}",
            hits[0].message
        );
    }

    #[test]
    fn validate_reports_scope_path_missing_for_manifest() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy")).expect("mkdir legacy");
        let plan = plan_with_source_and_scope(
            "legacy",
            Scope::try_new(vec![], vec![], Some(".specify/plans/demo/slices/ingest.yaml".into()))
                .expect("scope"),
        );

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-path-missing").collect();
        assert_eq!(hits.len(), 1, "expected one scope-path-missing, got {results:#?}");
        assert!(
            hits[0].message.contains(".specify/plans/demo/slices/ingest.yaml"),
            "must name the manifest path: {}",
            hits[0].message
        );
        assert!(
            hits[0].message.contains("manifest"),
            "must identify kind=manifest: {}",
            hits[0].message
        );
    }

    #[test]
    fn validate_accepts_scope_paths_when_present() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy/src/ingest")).expect("mkdir ingest");
        std::fs::create_dir_all(tmp.path().join("legacy/src/ingest/_deprecated"))
            .expect("mkdir deprecated");
        let manifest_path = tmp.path().join(".specify/plans/demo/slices/ingest.yaml");
        std::fs::create_dir_all(manifest_path.parent().unwrap()).expect("mkdir manifest parent");
        std::fs::write(tmp.path().join("legacy/src/ingest/keepme.rs"), b"// ok\n")
            .expect("write keepme.rs");
        std::fs::write(&manifest_path, b"version: 1\ninclude:\n  - src/ingest/keepme.rs\n")
            .expect("write manifest");

        // Include + exclude path plus a manifest that all resolve.
        // We split into two scope entries across two changes — one
        // change carrying include/exclude, one carrying the manifest —
        // because `manifest` is mutually exclusive with
        // `include`/`exclude` for the same source key (the
        // `Scope::try_new` invariant).
        let mut changes = vec![
            PlanChange {
                name: "globs".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec!["monolith".to_string()],
                scope: BTreeMap::from([(
                    "monolith".to_string(),
                    Scope::try_new(
                        vec!["src/ingest/**".into()],
                        vec!["src/ingest/_deprecated/**".into()],
                        None,
                    )
                    .expect("scope globs"),
                )]),
                description: None,
                status_reason: None,
            },
            PlanChange {
                name: "manifest".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec!["monolith".to_string()],
                scope: BTreeMap::from([(
                    "monolith".to_string(),
                    Scope::try_new(
                        vec![],
                        vec![],
                        Some(".specify/plans/demo/slices/ingest.yaml".into()),
                    )
                    .expect("scope manifest"),
                )]),
                description: None,
                status_reason: None,
            },
        ];
        changes.sort_by(|a, b| a.name.cmp(&b.name));
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::from([("monolith".to_string(), "legacy".to_string())]),
            changes,
        };

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-path-missing"),
            "all scope paths resolve — no scope-path-missing expected, got: {results:#?}"
        );
    }

    #[test]
    fn validate_skips_path_existence_when_project_dir_none() {
        // Plan references a nonexistent include path; with
        // project_dir=None the check must not fire.
        let plan = plan_with_source_and_scope(
            "legacy",
            Scope::try_new(vec!["src/nope/**".into()], vec![], None).expect("scope"),
        );
        let results = plan.validate(None, None);
        assert!(
            !results.iter().any(|r| r.code == "scope-path-missing"),
            "project_dir=None must silently skip path-existence: {results:#?}"
        );
    }

    #[test]
    fn validate_skips_path_existence_for_remote_source() {
        let tmp = tempdir().expect("tempdir");
        // No filesystem setup — the remote URL must short-circuit
        // before any existence check runs.
        let plan = plan_with_source_and_scope(
            "https://github.com/org/monolith.git",
            Scope::try_new(vec!["src/ingest/**".into()], vec![], None).expect("scope"),
        );

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-path-missing"),
            "URL sources must be skipped silently: {results:#?}"
        );
    }

    #[test]
    fn validate_skips_path_existence_for_absolute_source() {
        let tmp = tempdir().expect("tempdir");
        // Absolute paths are developer-local and not meaningfully
        // validatable from the project root; must skip silently.
        let plan = plan_with_source_and_scope(
            "/does/not/exist/anywhere",
            Scope::try_new(vec!["src/ingest/**".into()], vec![], None).expect("scope"),
        );

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-path-missing"),
            "absolute-path sources must be skipped silently: {results:#?}"
        );
    }

    #[test]
    fn validate_does_not_emit_path_missing_for_orphan_scope_key() {
        // Change's scope names a key (`orders`) that is not in the
        // change's own `sources`. That is a `scope-key-not-in-sources`
        // error; `scope-path-missing` must NOT also fire — no
        // double-diagnose on the same offender.
        let tmp = tempdir().expect("tempdir");
        let mut plan = plan_with_source_and_scope(
            "legacy",
            Scope::try_new(vec!["src/ingest/**".into()], vec![], None).expect("scope"),
        );
        // Rename the scope key to an orphan and re-register it on the
        // top-level sources so `unknown-source` is NOT the confounding
        // finding — we specifically want to exercise the
        // "orphan-on-change" branch.
        let scope = plan.changes[0].scope.remove("monolith").expect("moved");
        plan.changes[0].scope.insert("orders".to_string(), scope);
        plan.sources.insert("orders".to_string(), "other-legacy".to_string());

        let results = plan.validate(None, Some(tmp.path()));
        let has_key_missing = results
            .iter()
            .any(|r| r.code == "scope-key-not-in-sources" && r.entry.as_deref() == Some("ingest"));
        assert!(has_key_missing, "expected scope-key-not-in-sources to still fire: {results:#?}");
        assert!(
            !results.iter().any(|r| r.code == "scope-path-missing"),
            "scope-path-missing must not double-diagnose the orphan key: {results:#?}"
        );
    }

    // --- RFC-3a C05: scope-overlap / scope-orphan ---------------------

    /// Build an empty plan entry with the given name and a single
    /// scope-source pair. Trims the ceremony of the full `PlanChange`
    /// literal so each C05 test body fits on screen.
    fn scoped_change(name: &str, source: &str, scope: Option<Scope>) -> PlanChange {
        let mut scope_map = BTreeMap::new();
        if let Some(s) = scope {
            scope_map.insert(source.to_string(), s);
        }
        PlanChange {
            name: name.to_string(),
            status: PlanStatus::Pending,
            depends_on: vec![],
            affects: vec![],
            sources: vec![source.to_string()],
            scope: scope_map,
            description: None,
            status_reason: None,
        }
    }

    /// Plan with a single `monolith -> legacy` source mapping and the
    /// given changes. Keeps the C05 test bodies focused on scope
    /// content rather than plan scaffolding.
    fn plan_with_monolith(changes: Vec<PlanChange>) -> Plan {
        Plan {
            name: "c05".to_string(),
            sources: BTreeMap::from([("monolith".to_string(), "legacy".to_string())]),
            changes,
        }
    }

    /// Touch `project/<rel>` ensuring parents exist. Fails the test on
    /// any I/O error — fixtures are tiny and local, so a failure here
    /// is a setup bug, not a flake to tolerate.
    fn write_file(project: &Path, rel: &str) {
        let p = project.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parents");
        }
        std::fs::write(&p, b"// fixture\n").expect("write fixture file");
    }

    #[test]
    fn validate_emits_no_scope_warnings_on_plan_without_scope() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/src/b.ts");
        let plan = plan_with_monolith(vec![
            scoped_change("alpha", "monolith", None),
            scoped_change("beta", "monolith", None),
        ]);

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-overlap" || r.code == "scope-orphan"),
            "plan with no scope anywhere must not emit overlap/orphan: {results:#?}"
        );
    }

    #[test]
    fn validate_reports_scope_overlap_between_two_changes() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        let plan = plan_with_monolith(vec![
            scoped_change(
                "alpha",
                "monolith",
                Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
            ),
            scoped_change(
                "beta",
                "monolith",
                Some(Scope::try_new(vec!["src/*.ts".into()], vec![], None).expect("scope")),
            ),
        ]);

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-overlap").collect();
        assert_eq!(hits.len(), 1, "one overlap finding expected, got: {results:#?}");
        assert_eq!(hits[0].level, PlanValidationLevel::Warning);
        assert_eq!(hits[0].entry, None, "overlap is cross-entry: entry must be None");
        let msg = &hits[0].message;
        assert!(msg.contains("monolith:src/a.ts"), "must name source and path: {msg}");
        assert!(msg.contains("alpha") && msg.contains("beta"), "must name both changes: {msg}");
    }

    #[test]
    fn validate_reports_scope_overlap_across_n_changes() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/x.ts");
        // Intentionally insert in non-alphabetical order so the sort
        // in the emitted message is a meaningful assertion.
        let plan = plan_with_monolith(vec![
            scoped_change(
                "gamma",
                "monolith",
                Some(Scope::try_new(vec!["src/x.ts".into()], vec![], None).expect("scope")),
            ),
            scoped_change(
                "alpha",
                "monolith",
                Some(Scope::try_new(vec!["src/x.ts".into()], vec![], None).expect("scope")),
            ),
            scoped_change(
                "beta",
                "monolith",
                Some(Scope::try_new(vec!["src/x.ts".into()], vec![], None).expect("scope")),
            ),
        ]);

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-overlap").collect();
        assert_eq!(hits.len(), 1, "three-way overlap is one finding: {results:#?}");
        let msg = &hits[0].message;
        let claim_tail = msg.split_once(": ").map(|(_, tail)| tail).expect("has ': <names>'");
        assert_eq!(
            claim_tail, "alpha, beta, gamma",
            "N-way overlap must list names in sorted order for deterministic output: {msg}"
        );
    }

    #[test]
    fn validate_no_overlap_for_distinct_globs() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/src/b.ts");
        let plan = plan_with_monolith(vec![
            scoped_change(
                "alpha",
                "monolith",
                Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
            ),
            scoped_change(
                "beta",
                "monolith",
                Some(Scope::try_new(vec!["src/b.ts".into()], vec![], None).expect("scope")),
            ),
        ]);

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-overlap"),
            "non-overlapping globs must not trip scope-overlap: {results:#?}"
        );
    }

    #[test]
    fn validate_overlap_handles_exclude_correctly() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/src/b.ts");
        let plan = plan_with_monolith(vec![
            scoped_change(
                "alpha",
                "monolith",
                Some(
                    Scope::try_new(vec!["src/**".into()], vec!["src/b.ts".into()], None)
                        .expect("scope"),
                ),
            ),
            scoped_change(
                "beta",
                "monolith",
                Some(Scope::try_new(vec!["src/b.ts".into()], vec![], None).expect("scope")),
            ),
        ]);

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-overlap"),
            "exclude must subtract from include before overlap is computed: {results:#?}"
        );
    }

    #[test]
    fn validate_overlap_handles_whole_source_claim() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/src/b.ts");
        // `alpha` has no scope entry for `monolith` (scope map is
        // non-empty on the plan because `beta` has scope) → claims
        // the whole tree.
        let alpha = PlanChange {
            name: "alpha".to_string(),
            status: PlanStatus::Pending,
            depends_on: vec![],
            affects: vec![],
            sources: vec!["monolith".to_string()],
            scope: BTreeMap::new(),
            description: None,
            status_reason: None,
        };
        let beta = scoped_change(
            "beta",
            "monolith",
            Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
        );
        let plan = plan_with_monolith(vec![alpha, beta]);

        let results = plan.validate(None, Some(tmp.path()));
        let overlap: Vec<_> = results.iter().filter(|r| r.code == "scope-overlap").collect();
        // Exactly `src/a.ts` overlaps: `alpha` claims whole tree
        // (both files), `beta` claims only `src/a.ts`; `src/b.ts` is
        // only `alpha`'s.
        assert_eq!(overlap.len(), 1, "one overlap on src/a.ts expected: {results:#?}");
        let msg = &overlap[0].message;
        assert!(msg.contains("monolith:src/a.ts"), "overlap must name src/a.ts: {msg}");
        assert!(msg.contains("alpha") && msg.contains("beta"), "must name both changes: {msg}");
    }

    #[test]
    fn validate_reports_scope_orphan_for_uncovered_file() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/src/b.ts");
        let plan = plan_with_monolith(vec![scoped_change(
            "alpha",
            "monolith",
            Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
        )]);

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-orphan").collect();
        assert_eq!(hits.len(), 1, "one orphan expected: {results:#?}");
        assert_eq!(hits[0].level, PlanValidationLevel::Warning);
        assert_eq!(hits[0].entry, None, "orphan is plan-wide: entry must be None");
        assert!(
            hits[0].message.contains("monolith:src/b.ts"),
            "must name orphan path with source prefix: {}",
            hits[0].message
        );
    }

    #[test]
    fn validate_orphan_ignores_git_dir() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/.git/HEAD");
        write_file(tmp.path(), "legacy/.git/config");
        let plan = plan_with_monolith(vec![scoped_change(
            "alpha",
            "monolith",
            Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
        )]);

        let results = plan.validate(None, Some(tmp.path()));
        let orphans: Vec<_> = results.iter().filter(|r| r.code == "scope-orphan").collect();
        assert!(
            orphans.is_empty(),
            ".git/ entries must never be walked for orphan reporting: {orphans:#?}"
        );
    }

    #[test]
    fn validate_orphan_ignores_node_modules() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/node_modules/lodash/index.js");
        let plan = plan_with_monolith(vec![scoped_change(
            "alpha",
            "monolith",
            Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
        )]);

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-orphan"),
            "node_modules/ entries must be skipped by the orphan walker: {results:#?}"
        );
    }

    #[test]
    fn validate_orphan_silent_when_no_change_has_scope() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/a.ts");
        write_file(tmp.path(), "legacy/src/b.ts");
        // No change carries scope — the guard clause must skip the
        // whole sweep, or the entire source tree would look orphan.
        let plan = plan_with_monolith(vec![
            scoped_change("alpha", "monolith", None),
            scoped_change("beta", "monolith", None),
        ]);

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-orphan"),
            "no scope anywhere → no orphan findings (back-compat): {results:#?}"
        );
    }

    #[test]
    fn validate_warnings_silent_when_project_dir_none() {
        // Plan has scope but no project_dir → the opt-in contract
        // means neither overlap nor orphan fires.
        let plan = plan_with_monolith(vec![
            scoped_change(
                "alpha",
                "monolith",
                Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
            ),
            scoped_change(
                "beta",
                "monolith",
                Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
            ),
        ]);

        let results = plan.validate(None, None);
        assert!(
            !results.iter().any(|r| r.code == "scope-overlap" || r.code == "scope-orphan"),
            "project_dir=None must silently skip overlap/orphan: {results:#?}"
        );
    }

    #[test]
    fn validate_warnings_silent_when_source_is_remote() {
        let tmp = tempdir().expect("tempdir");
        let plan = Plan {
            name: "c05".into(),
            sources: BTreeMap::from([(
                "monolith".to_string(),
                "https://github.com/org/monolith.git".to_string(),
            )]),
            changes: vec![scoped_change(
                "alpha",
                "monolith",
                Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
            )],
        };

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-overlap" || r.code == "scope-orphan"),
            "remote source must be skipped silently (no universe to walk): {results:#?}"
        );
    }

    #[test]
    fn validate_warnings_silent_when_source_path_missing() {
        let tmp = tempdir().expect("tempdir");
        // `absent/` doesn't exist — C04 already emits
        // `scope-path-missing`; C05 must not double-warn with a
        // phantom orphan sweep.
        let plan = Plan {
            name: "c05".into(),
            sources: BTreeMap::from([("monolith".to_string(), "absent".to_string())]),
            changes: vec![scoped_change(
                "alpha",
                "monolith",
                Some(Scope::try_new(vec!["src/a.ts".into()], vec![], None).expect("scope")),
            )],
        };

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-overlap" || r.code == "scope-orphan"),
            "missing source path must be skipped — C04 already owns that diagnostic: {results:#?}"
        );
    }

    #[test]
    fn validate_manifest_scope_included_in_overlap_check() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/x.ts");
        // v1 manifest (`version: 1`, `include:`) — overlap uses the
        // same shape as `Plan::validate` (C26); parse failures yield an
        // empty claim set here while validation emits `manifest-invalid`.
        let manifest_rel = ".specify/plans/demo/slices/alpha.yaml";
        let manifest_abs = tmp.path().join(manifest_rel);
        std::fs::create_dir_all(manifest_abs.parent().unwrap()).expect("mkdir manifest parent");
        std::fs::write(&manifest_abs, b"version: 1\ninclude:\n  - src/x.ts\n").expect("manifest");

        let alpha = scoped_change(
            "alpha",
            "monolith",
            Some(Scope::try_new(vec![], vec![], Some(manifest_rel.to_string())).expect("scope")),
        );
        let beta = scoped_change(
            "beta",
            "monolith",
            Some(Scope::try_new(vec!["src/x.ts".into()], vec![], None).expect("scope")),
        );
        let plan = plan_with_monolith(vec![alpha, beta]);

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-overlap").collect();
        assert_eq!(
            hits.len(),
            1,
            "manifest-carried claims must participate in overlap: {results:#?}"
        );
        assert!(hits[0].message.contains("monolith:src/x.ts"));
        assert!(hits[0].message.contains("alpha") && hits[0].message.contains("beta"));
    }

    // --- RFC-3a C26: manifest shape validation ------------------------

    fn plan_manifest_only_change(name: &str, manifest_rel: &str) -> Plan {
        Plan {
            name: "c26".into(),
            sources: BTreeMap::from([("monolith".to_string(), "legacy".to_string())]),
            changes: vec![scoped_change(
                name,
                "monolith",
                Some(
                    Scope::try_new(vec![], vec![], Some(manifest_rel.to_string())).expect("scope"),
                ),
            )],
        }
    }

    #[test]
    fn validate_reports_manifest_invalid_on_bad_yaml() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy")).expect("mkdir legacy");
        let rel = ".specify/plans/demo/slices/bad.yaml";
        let abs = tmp.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir parent");
        std::fs::write(&abs, b"version: 1\ninclude: not_a_sequence\n").expect("write broken yaml");

        let plan = plan_manifest_only_change("alpha", rel);
        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "manifest-invalid").collect();
        assert_eq!(hits.len(), 1, "expected manifest-invalid: {results:#?}");
    }

    #[test]
    fn validate_reports_manifest_invalid_on_unknown_field() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy")).expect("mkdir legacy");
        let rel = ".specify/plans/demo/slices/extra.yaml";
        let abs = tmp.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir parent");
        std::fs::write(&abs, b"version: 1\ninclude: []\nextra: true\n").expect("write manifest");

        let plan = plan_manifest_only_change("alpha", rel);
        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "manifest-invalid").collect();
        assert_eq!(hits.len(), 1, "expected manifest-invalid: {results:#?}");
    }

    #[test]
    fn validate_reports_manifest_invalid_on_wrong_version() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy")).expect("mkdir legacy");
        let rel = ".specify/plans/demo/slices/v2.yaml";
        let abs = tmp.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir parent");
        std::fs::write(&abs, b"version: 2\ninclude: []\n").expect("write manifest");

        let plan = plan_manifest_only_change("alpha", rel);
        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "manifest-invalid").collect();
        assert_eq!(hits.len(), 1, "expected manifest-invalid: {results:#?}");
    }

    #[test]
    fn validate_reports_manifest_empty_include() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy")).expect("mkdir legacy");
        let rel = ".specify/plans/demo/slices/empty.yaml";
        let abs = tmp.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir parent");
        std::fs::write(&abs, b"version: 1\ninclude: []\n").expect("write manifest");

        let plan = plan_manifest_only_change("alpha", rel);
        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "manifest-empty").collect();
        assert_eq!(hits.len(), 1, "expected manifest-empty: {results:#?}");
    }

    #[test]
    fn validate_reports_manifest_path_escape_on_dotdot() {
        let tmp = tempdir().expect("tempdir");
        write_file(tmp.path(), "legacy/src/x.ts");
        let rel = ".specify/plans/demo/slices/escape.yaml";
        let abs = tmp.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir parent");
        std::fs::write(&abs, b"version: 1\ninclude:\n  - ../outside\n").expect("write manifest");

        let plan = plan_manifest_only_change("alpha", rel);
        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "manifest-path-escape").collect();
        assert_eq!(hits.len(), 1, "expected manifest-path-escape: {results:#?}");
    }

    #[test]
    fn validate_reports_manifest_path_escape_on_absolute_include() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy")).expect("mkdir legacy");
        let rel = ".specify/plans/demo/slices/abs.yaml";
        let abs = tmp.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir parent");
        std::fs::write(&abs, b"version: 1\ninclude:\n  - /etc/passwd\n").expect("write manifest");

        let plan = plan_manifest_only_change("alpha", rel);
        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "manifest-path-escape").collect();
        assert_eq!(hits.len(), 1, "expected manifest-path-escape: {results:#?}");
    }

    #[test]
    fn validate_reports_scope_path_missing_for_manifest_include_target() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("legacy/src")).expect("mkdir legacy");
        let rel = ".specify/plans/demo/slices/missing-target.yaml";
        let abs = tmp.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("mkdir parent");
        std::fs::write(&abs, b"version: 1\ninclude:\n  - src/nope.ts\n").expect("write manifest");

        let plan = plan_manifest_only_change("alpha", rel);
        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "scope-path-missing").collect();
        assert_eq!(hits.len(), 1, "expected scope-path-missing for bad include: {results:#?}");
        assert!(hits[0].message.contains("src/nope.ts"), "{}", hits[0].message);
    }

    // --- RFC-3a C25: scope-missing-on-monolith ------------------------

    /// Seed `<project>/.specify/plans/<plan>/analyze/<key>/metadata.json`
    /// with a raw JSON body. Tests compose the body themselves so
    /// they can exercise malformed / wrong-version paths.
    fn seed_analyze_metadata(project: &Path, plan: &str, key: &str, body: &str) {
        let path = project
            .join(".specify")
            .join("plans")
            .join(plan)
            .join("analyze")
            .join(key)
            .join("metadata.json");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir analyze");
        std::fs::write(&path, body).expect("write metadata.json");
    }

    /// Render the v1 metadata shape as JSON — single source of truth
    /// for the fixture used by most C25 tests. `source_key` is the
    /// directory segment and the JSON field; they agree by construction
    /// here (the RFC requires it; checking disagreement is out of scope
    /// for the lint).
    fn metadata_v1(source_key: &str, language: &str, loc: u64, module_count: u32) -> String {
        format!(
            r#"{{
  "version": 1,
  "source_key": "{source_key}",
  "language": "{language}",
  "loc": {loc},
  "module_count": {module_count},
  "top_level_modules": []
}}"#
        )
    }

    /// A single-change plan named `demo` drawing from one source key
    /// `monolith` with the given scope map. Keeps the monolith lint
    /// tests focused on metadata + scope shape rather than plan
    /// boilerplate.
    fn monolith_plan(change_name: &str, scope: BTreeMap<String, Scope>) -> Plan {
        Plan {
            name: "demo".to_string(),
            sources: BTreeMap::from([("monolith".to_string(), "legacy".to_string())]),
            changes: vec![PlanChange {
                name: change_name.to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec!["monolith".to_string()],
                scope,
                description: None,
                status_reason: None,
            }],
        }
    }

    #[test]
    fn validate_scope_missing_on_monolith_fires_when_loc_above_threshold() {
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 10_000, 5),
        );
        let plan = monolith_plan("ingest", BTreeMap::new());

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "scope-missing-on-monolith").collect();
        assert_eq!(hits.len(), 1, "LOC == threshold must trip the lint: {results:#?}");
        assert_eq!(hits[0].level, PlanValidationLevel::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("ingest"));
        let msg = &hits[0].message;
        assert!(msg.contains("5 modules"), "must cite module_count verbatim: {msg}");
        assert!(msg.contains("10k LOC"), "must cite LOC rounded to thousands: {msg}");
    }

    #[test]
    fn validate_scope_missing_on_monolith_fires_when_module_count_above_threshold() {
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 500, 20),
        );
        let plan = monolith_plan("ingest", BTreeMap::new());

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "scope-missing-on-monolith").collect();
        assert_eq!(
            hits.len(),
            1,
            "module_count == threshold with sub-threshold LOC must still trip: {results:#?}"
        );
        assert!(
            hits[0].message.contains("20 modules"),
            "must cite module_count: {}",
            hits[0].message
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_below_thresholds() {
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 500, 5),
        );
        let plan = monolith_plan("ingest", BTreeMap::new());

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "sub-threshold metadata must not fire: {results:#?}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_change_has_scope_entry() {
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 87_000, 42),
        );
        let mut scope = BTreeMap::new();
        scope.insert(
            "monolith".to_string(),
            Scope::try_new(vec!["src/ingest/**".into()], vec![], None).expect("scope"),
        );
        let plan = monolith_plan("ingest", scope);

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "change with scope entry must not be warned: {results:#?}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_scope_entry_is_empty_object() {
        // RFC-3a / C01 / C02: empty scope entries (`{}`) are legal and
        // semantically equivalent to "whole source". The lint MUST NOT
        // second-guess them — the operator explicitly opted in.
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 87_000, 42),
        );
        let mut scope = BTreeMap::new();
        scope.insert(
            "monolith".to_string(),
            Scope::try_new(vec![], vec![], None).expect("empty scope"),
        );
        let plan = monolith_plan("ingest", scope);

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "empty scope entry still counts as opt-in: {results:#?}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_metadata_absent() {
        let tmp = tempdir().expect("tempdir");
        let plan = monolith_plan("ingest", BTreeMap::new());

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "absent metadata.json must silently skip (greenfield/small-legacy): {results:#?}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_project_dir_none() {
        let plan = monolith_plan("ingest", BTreeMap::new());
        let results = plan.validate(None, None);
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "project_dir=None must silently skip the lint: {results:#?}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_metadata_version_mismatch() {
        let tmp = tempdir().expect("tempdir");
        let body = r#"{
  "version": 2,
  "source_key": "monolith",
  "language": "typescript",
  "loc": 87000,
  "module_count": 42,
  "top_level_modules": []
}"#;
        seed_analyze_metadata(tmp.path(), "demo", "monolith", body);
        let plan = monolith_plan("ingest", BTreeMap::new());

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "unsupported metadata version must silently skip: {results:#?}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_metadata_malformed() {
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(tmp.path(), "demo", "monolith", "{ not valid json");
        let plan = monolith_plan("ingest", BTreeMap::new());

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "malformed JSON must silently skip for v1 (invalid-analyze-metadata is a \
             separate chunk): {results:#?}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_multi_source_only_fires_on_monolith() {
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 87_000, 42),
        );
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "shared-lib",
            &metadata_v1("shared-lib", "typescript", 200, 3),
        );
        let plan = Plan {
            name: "demo".into(),
            sources: BTreeMap::from([
                ("monolith".to_string(), "legacy".to_string()),
                ("shared-lib".to_string(), "vendor/shared".to_string()),
            ]),
            changes: vec![PlanChange {
                name: "ingest".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec!["monolith".to_string(), "shared-lib".to_string()],
                scope: BTreeMap::new(),
                description: None,
                status_reason: None,
            }],
        };

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "scope-missing-on-monolith").collect();
        assert_eq!(
            hits.len(),
            1,
            "only the monolith-scale source must trip, not shared-lib: {results:#?}"
        );
        assert!(
            hits[0].message.contains("`monolith`"),
            "finding must name the offending source key: {}",
            hits[0].message
        );
        assert!(
            !hits[0].message.contains("`shared-lib`"),
            "must not mention the small sibling source: {}",
            hits[0].message
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_diagnostic_matches_rfc_shape() {
        // RFC-3a §*Validation* example:
        //   "change `ingest-pipeline` has sources[`monolith`] classified
        //    as monolith-scale by /spec:analyze (42 modules, 87k LOC)
        //    and no scope entry — define-time /spec:extract may overflow
        //    context. Run `specify initiative amend ingest-pipeline
        //    --scope-include 'src/ingest/**'` to narrow the slice."
        // The remediation flag uses `<key>=<glob>` per C03's finalized
        // syntax (RFC example predates C03); the rest is byte-matched.
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 87_000, 42),
        );
        let plan = monolith_plan("ingest-pipeline", BTreeMap::new());

        let results = plan.validate(None, Some(tmp.path()));
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "scope-missing-on-monolith").collect();
        assert_eq!(hits.len(), 1, "exactly one finding for the RFC-shaped fixture: {results:#?}");
        let msg = &hits[0].message;
        assert!(msg.contains("change `ingest-pipeline`"), "must name the change: {msg}");
        assert!(msg.contains("sources[`monolith`]"), "must name the source key: {msg}");
        assert!(msg.contains("42 modules"), "must cite module count: {msg}");
        assert!(msg.contains("87k LOC"), "must cite LOC rounded to thousands: {msg}");
        assert!(msg.contains("/spec:analyze"), "must attribute the classifier: {msg}");
        assert!(msg.contains("/spec:extract may overflow"), "must state the consequence: {msg}");
        assert!(
            msg.contains("specify initiative amend ingest-pipeline --scope-include monolith="),
            "remediation must use C03's `--scope-include <key>=<glob>` syntax: {msg}"
        );
    }

    #[test]
    fn validate_scope_missing_on_monolith_silent_when_change_does_not_list_source() {
        // The change's `sources` must name the key for the lint to
        // fire — a plan-level source with monolith-scale metadata
        // that no change draws from is inert.
        let tmp = tempdir().expect("tempdir");
        seed_analyze_metadata(
            tmp.path(),
            "demo",
            "monolith",
            &metadata_v1("monolith", "typescript", 87_000, 42),
        );
        let plan = Plan {
            name: "demo".into(),
            sources: BTreeMap::from([("monolith".to_string(), "legacy".to_string())]),
            changes: vec![PlanChange {
                name: "ingest".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec![],
                scope: BTreeMap::new(),
                description: None,
                status_reason: None,
            }],
        };

        let results = plan.validate(None, Some(tmp.path()));
        assert!(
            !results.iter().any(|r| r.code == "scope-missing-on-monolith"),
            "change not drawing from the source must not trip the lint: {results:#?}"
        );
    }

    #[test]
    fn is_monolith_scale_threshold_boundaries() {
        let below = AnalyzeMetadata {
            version: 1,
            loc: 9_999,
            module_count: 19,
        };
        let loc_eq = AnalyzeMetadata {
            version: 1,
            loc: 10_000,
            module_count: 0,
        };
        let modules_eq = AnalyzeMetadata {
            version: 1,
            loc: 0,
            module_count: 20,
        };
        let both_above = AnalyzeMetadata {
            version: 1,
            loc: 87_000,
            module_count: 42,
        };
        assert!(!is_monolith_scale(&below), "9_999 LOC / 19 modules must not trip");
        assert!(is_monolith_scale(&loc_eq), "LOC at threshold must trip (inclusive)");
        assert!(is_monolith_scale(&modules_eq), "module_count at threshold must trip (inclusive)");
        assert!(is_monolith_scale(&both_above), "well above must trip");
    }

    #[test]
    fn glob_root_extraction() {
        assert_eq!(glob_root("src/foo/**"), "src/foo");
        assert_eq!(glob_root("src/*"), "src");
        assert_eq!(glob_root("src/"), "src");
        assert_eq!(glob_root("src/a.ts"), "src/a.ts");
        assert_eq!(glob_root("**/*.rs"), "");
        assert_eq!(glob_root("*.rs"), "");
        assert_eq!(glob_root("src/foo/bar.rs"), "src/foo/bar.rs");
        assert_eq!(glob_root("src/foo?"), "src/foo");
        assert_eq!(glob_root("src/[abc]/*"), "src");
        assert_eq!(glob_root("src/{a,b}/**"), "src");
        assert_eq!(glob_root(""), "");
    }

    #[test]
    fn is_remote_recognises_url_prefixes() {
        assert!(is_remote("http://example.com/repo.git"));
        assert!(is_remote("https://github.com/org/repo.git"));
        assert!(is_remote("git@github.com:org/repo.git"));
        assert!(is_remote("git+https://example.com/repo.git"));
        assert!(is_remote("ssh://user@host/repo.git"));
        assert!(!is_remote("legacy"));
        assert!(!is_remote("/abs/path"));
        assert!(!is_remote("./relative"));
    }

    fn plan_with_changes(changes: Vec<PlanChange>) -> Plan {
        Plan {
            name: "test".into(),
            sources: BTreeMap::new(),
            changes,
        }
    }

    fn change(name: &str, status: PlanStatus) -> PlanChange {
        PlanChange {
            name: name.into(),
            status,
            depends_on: vec![],
            affects: vec![],
            sources: vec![],
            scope: BTreeMap::new(),
            description: None,
            status_reason: None,
        }
    }

    #[test]
    fn clean_plan_returns_no_results() {
        let plan: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let results = plan.validate(None, None);
        assert!(
            results.is_empty(),
            "expected a clean RFC fixture to validate with no findings, got: {results:#?}"
        );
    }

    #[test]
    fn duplicate_name_reports_error() {
        let plan = plan_with_changes(vec![
            change("foo", PlanStatus::Done),
            change("foo", PlanStatus::Pending),
        ]);
        let results = plan.validate(None, None);
        let dupes: Vec<_> = results.iter().filter(|r| r.code == "duplicate-name").collect();
        assert_eq!(dupes.len(), 1, "expected one duplicate-name result, got {results:#?}");
        assert_eq!(dupes[0].level, PlanValidationLevel::Error);
        assert_eq!(dupes[0].entry.as_deref(), Some("foo"));
    }

    #[test]
    fn cycle_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.depends_on = vec!["c".into()];
        let mut b = change("b", PlanStatus::Pending);
        b.depends_on = vec!["a".into()];
        let mut c = change("c", PlanStatus::Pending);
        c.depends_on = vec!["b".into()];
        let plan = plan_with_changes(vec![a, b, c]);
        let results = plan.validate(None, None);
        let cycles: Vec<_> = results.iter().filter(|r| r.code == "dependency-cycle").collect();
        assert!(!cycles.is_empty(), "expected at least one dependency-cycle, got {results:#?}");
        let msg = &cycles[0].message;
        assert!(msg.contains('a'), "cycle message should name a: {msg}");
        assert!(msg.contains('b'), "cycle message should name b: {msg}");
        assert!(msg.contains('c'), "cycle message should name c: {msg}");
    }

    #[test]
    fn self_cycle_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.depends_on = vec!["a".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None, None);
        assert!(
            results.iter().any(|r| r.code == "dependency-cycle"),
            "expected a dependency-cycle result for self-edge, got: {results:#?}"
        );
    }

    #[test]
    fn unknown_depends_on_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.depends_on = vec!["bogus".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None, None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-depends-on").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-depends-on, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("bogus"));
    }

    #[test]
    fn unknown_affects_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.affects = vec!["ghost".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None, None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-affects").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-affects, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("ghost"));
    }

    #[test]
    fn unknown_source_reports_error() {
        let mut a = change("a", PlanStatus::Pending);
        a.sources = vec!["monolith".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None, None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-source").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-source, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("monolith"));
    }

    #[test]
    fn multiple_in_progress_reports_error_once_per_offender() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::InProgress),
            change("b", PlanStatus::InProgress),
        ]);
        let results = plan.validate(None, None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "multiple-in-progress").collect();
        assert_eq!(hits.len(), 2, "expected one result per offender, got {results:#?}");
        let names: HashSet<&str> = hits.iter().filter_map(|r| r.entry.as_deref()).collect();
        assert!(names.contains("a") && names.contains("b"), "names = {names:?}");
    }

    #[test]
    fn single_in_progress_is_fine() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::InProgress),
            change("b", PlanStatus::Pending),
        ]);
        let results = plan.validate(None, None);
        assert!(
            !results.iter().any(|r| r.code == "multiple-in-progress"),
            "single in-progress entry should not trip multiple-in-progress: {results:#?}"
        );
    }

    #[test]
    fn orphan_change_dir_is_warning() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("stale-change")).expect("mkdir");
        let plan = plan_with_changes(vec![change("other", PlanStatus::Pending)]);
        let results = plan.validate(Some(tmp.path()), None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "orphan-change-dir").collect();
        assert_eq!(hits.len(), 1, "expected one orphan-change-dir, got {results:#?}");
        assert_eq!(hits[0].level, PlanValidationLevel::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("stale-change"));
    }

    #[test]
    fn missing_dir_for_in_progress_is_warning() {
        let tmp = tempdir().expect("tempdir");
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(Some(tmp.path()), None);
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "missing-change-dir-for-in-progress").collect();
        assert_eq!(hits.len(), 1, "expected one missing-dir warning, got {results:#?}");
        assert_eq!(hits[0].level, PlanValidationLevel::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("alpha"));
    }

    #[test]
    fn present_dir_for_in_progress_is_silent() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("alpha")).expect("mkdir alpha");
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(Some(tmp.path()), None);
        assert!(
            !results.iter().any(|r| r.code.ends_with("-change-dir")
                || r.code == "orphan-change-dir"
                || r.code == "missing-change-dir-for-in-progress"),
            "no directory warnings expected, got: {results:#?}"
        );
    }

    #[test]
    fn changes_dir_none_skips_consistency_checks() {
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(None, None);
        assert!(
            !results
                .iter()
                .any(|r| r.code == "orphan-change-dir"
                    || r.code == "missing-change-dir-for-in-progress"),
            "passing None for changes_dir must skip directory consistency checks: {results:#?}"
        );
    }

    #[test]
    fn accumulates_all_findings_no_short_circuit() {
        // One plan, three distinct violations:
        //   - duplicate name `foo`
        //   - unknown depends-on target
        //   - unknown source key
        let mut a = change("foo", PlanStatus::Pending);
        a.depends_on = vec!["missing".into()];
        a.sources = vec!["ghost-source".into()];
        let b = change("foo", PlanStatus::Pending);
        let plan = plan_with_changes(vec![a, b]);
        let results = plan.validate(None, None);

        let codes: HashSet<&'static str> = results.iter().map(|r| r.code).collect();
        for expected in ["duplicate-name", "unknown-depends-on", "unknown-source"] {
            assert!(
                codes.contains(expected),
                "expected code {expected} in {codes:?} — validate must not short-circuit"
            );
        }
    }

    /// Convenience: build a `PlanChange` with an explicit `depends_on`.
    fn change_with_deps(name: &str, status: PlanStatus, deps: &[&str]) -> PlanChange {
        PlanChange {
            name: name.into(),
            status,
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            affects: vec![],
            sources: vec![],
            scope: BTreeMap::new(),
            description: None,
            status_reason: None,
        }
    }

    #[test]
    fn next_eligible_picks_first_pending_with_done_deps_in_list_order() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::Done),
            change("b", PlanStatus::Done),
            change_with_deps("c", PlanStatus::Pending, &["b"]),
        ]);
        let eligible = plan.next_eligible().expect("c should be eligible");
        assert_eq!(eligible.name, "c");
    }

    #[test]
    fn next_eligible_skips_pending_with_unmet_deps() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::Pending),
            change_with_deps("b", PlanStatus::Pending, &["a"]),
        ]);
        let eligible = plan.next_eligible().expect("a should be eligible");
        assert_eq!(eligible.name, "a", "b's dep 'a' is not done, so a (no deps) wins");
    }

    #[test]
    fn next_eligible_returns_none_when_in_progress_exists() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::InProgress),
            change("b", PlanStatus::Pending),
        ]);
        assert!(
            plan.next_eligible().is_none(),
            "an in-progress entry must block any new selection"
        );
    }

    #[test]
    fn next_eligible_returns_none_when_nothing_pending() {
        let plan = plan_with_changes(vec![
            change("a", PlanStatus::Done),
            change("b", PlanStatus::Skipped),
            change("c", PlanStatus::Failed),
        ]);
        assert!(plan.next_eligible().is_none());
    }

    #[test]
    fn next_eligible_list_order_tiebreak() {
        let plan = plan_with_changes(vec![
            change("alpha", PlanStatus::Pending),
            change("beta", PlanStatus::Pending),
        ]);
        let eligible = plan.next_eligible().expect("alpha should be first");
        assert_eq!(eligible.name, "alpha", "list-order tie-break must pick the first entry");
    }

    /// Drive `next_eligible` forward across the RFC-2 example plan,
    /// marking each returned entry `done`, and assert the exact
    /// traversal sequence. Ordering is a function of the plan's
    /// `depends-on` graph plus the list-order tie-break rule:
    ///
    ///   round 1: `user-registration` (no deps) — first pending
    ///   round 2: `email-verification` — its only dep just became done
    ///            and it precedes `registration-duplicate-email-crash`
    ///            (which is also eligible) in list order
    ///   round 3: `registration-duplicate-email-crash` (no deps, now
    ///            first remaining pending)
    ///   round 4: `notification-preferences` (dep user-registration done)
    ///   round 5: `extract-shared-validation` (dep email-verification done)
    ///   round 6: `product-catalog` (dep extract-shared-validation done)
    ///   round 7: `shopping-cart` (deps product-catalog + user-registration done)
    ///   round 8: `checkout-api` (dep shopping-cart done)
    ///   round 9: `checkout-ui` (dep checkout-api done)
    #[test]
    fn next_eligible_walks_rfc_example_forward() {
        let mut plan: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        for entry in &mut plan.changes {
            entry.status = PlanStatus::Pending;
            entry.status_reason = None;
        }

        let mut traversal = Vec::new();
        while let Some(next) = plan.next_eligible() {
            let name = next.name.clone();
            traversal.push(name.clone());
            let entry = plan
                .changes
                .iter_mut()
                .find(|c| c.name == name)
                .expect("returned name must exist in plan");
            entry.status = PlanStatus::Done;
        }

        let expected = [
            "user-registration",
            "email-verification",
            "registration-duplicate-email-crash",
            "notification-preferences",
            "extract-shared-validation",
            "product-catalog",
            "shopping-cart",
            "checkout-api",
            "checkout-ui",
        ];
        assert_eq!(
            traversal, expected,
            "next_eligible traversal should follow the RFC-2 §The Plan expected forward order"
        );
    }

    #[test]
    fn next_eligible_treats_in_progress_block_even_mid_cycle() {
        let plan = plan_with_changes(vec![
            change("in-flight", PlanStatus::InProgress),
            change_with_deps("a", PlanStatus::Pending, &["b"]),
            change_with_deps("b", PlanStatus::Pending, &["a"]),
        ]);
        assert!(
            plan.next_eligible().is_none(),
            "in-progress entry must block selection before any dependency walk"
        );
    }

    #[test]
    fn topological_order_rfc_example_matches_known_order() {
        let plan: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let ordered: Vec<&str> = plan
            .topological_order()
            .expect("rfc plan has no cycles")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let expected = [
            "user-registration",
            "email-verification",
            "registration-duplicate-email-crash",
            "notification-preferences",
            "extract-shared-validation",
            "product-catalog",
            "shopping-cart",
            "checkout-api",
            "checkout-ui",
        ];
        assert_eq!(
            ordered, expected,
            "topological_order should match next_eligible forward traversal"
        );
    }

    #[test]
    fn topological_order_on_cycle_returns_err() {
        let plan = plan_with_changes(vec![
            change_with_deps("a", PlanStatus::Pending, &["c"]),
            change_with_deps("b", PlanStatus::Pending, &["a"]),
            change_with_deps("c", PlanStatus::Pending, &["b"]),
        ]);
        let err = plan.topological_order().expect_err("cycle must surface as Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("cycle"), "Config message should mention 'cycle', got: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn topological_order_is_deterministic_under_tiebreak() {
        let alpha_first = plan_with_changes(vec![
            change("alpha", PlanStatus::Pending),
            change("beta", PlanStatus::Pending),
        ]);
        let order: Vec<&str> = alpha_first
            .topological_order()
            .expect("no cycle")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(order, ["alpha", "beta"]);

        let beta_first = plan_with_changes(vec![
            change("beta", PlanStatus::Pending),
            change("alpha", PlanStatus::Pending),
        ]);
        let order: Vec<&str> = beta_first
            .topological_order()
            .expect("no cycle")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            order,
            ["beta", "alpha"],
            "swapping list order must swap topo order when no deps constrain it"
        );
    }

    /// `next_eligible` must not depend on `topological_order` succeeding:
    /// even when the plan has a cycle, an in-progress entry short-circuits
    /// selection to `None` without walking the dependency graph.
    #[test]
    fn next_eligible_works_even_when_topological_order_errors() {
        let plan = plan_with_changes(vec![
            change("busy", PlanStatus::InProgress),
            change_with_deps("a", PlanStatus::Pending, &["b"]),
            change_with_deps("b", PlanStatus::Pending, &["a"]),
        ]);
        assert!(plan.next_eligible().is_none());
        assert!(plan.topological_order().is_err(), "cycle should surface from topological_order");
    }

    #[test]
    fn save_leaves_no_intermediate_state_observable_after_success() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");

        let first = Plan {
            name: "first".to_string(),
            sources: BTreeMap::new(),
            changes: vec![],
        };
        first.save(&path).expect("save first ok");

        let second = Plan {
            name: "second".to_string(),
            sources: BTreeMap::new(),
            changes: vec![PlanChange {
                name: "new-entry".to_string(),
                status: PlanStatus::Pending,
                depends_on: vec![],
                affects: vec![],
                sources: vec![],
                scope: BTreeMap::new(),
                description: None,
                status_reason: None,
            }],
        };
        second.save(&path).expect("save second ok");

        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, second, "after a successful save, only the new content is observable");
        assert_ne!(loaded, first, "the previous plan should no longer be on disk");

        let bytes = std::fs::read(&path).expect("read bytes");
        assert!(!bytes.is_empty(), "saved file should not be empty after overwrite");
        assert_eq!(*bytes.last().unwrap(), b'\n', "overwritten file should still end with newline");
    }

    // --- L1.F: Plan::create / Plan::amend / Plan::transition -----------

    #[test]
    fn create_appends_with_pending_status_and_clears_reason() {
        let mut plan = plan_with_changes(vec![]);
        let incoming = PlanChange {
            name: "foo".into(),
            status: PlanStatus::Failed,
            depends_on: vec![],
            affects: vec![],
            sources: vec![],
            scope: BTreeMap::new(),
            description: None,
            status_reason: Some("bogus".into()),
        };
        plan.create(incoming).expect("create ok");
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].name, "foo");
        assert_eq!(
            plan.changes[0].status,
            PlanStatus::Pending,
            "create must force status to Pending regardless of input"
        );
        assert_eq!(
            plan.changes[0].status_reason, None,
            "create must clear status_reason regardless of input"
        );
    }

    #[test]
    fn create_rejects_duplicate_name() {
        let mut plan = plan_with_changes(vec![change("foo", PlanStatus::Pending)]);
        let dup = change("foo", PlanStatus::Pending);
        let err = plan.create(dup).expect_err("duplicate must be rejected");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("already contains") && msg.contains("foo"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        assert_eq!(plan.changes.len(), 1, "plan must still have exactly one entry");
    }

    #[test]
    fn create_rejects_invalid_name() {
        let mut plan = plan_with_changes(vec![]);
        let bad = change("Bad-Name", PlanStatus::Pending);
        let err = plan.create(bad).expect_err("invalid name must be rejected");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("kebab-case"), "expected kebab-case in message, got: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        assert!(plan.changes.is_empty(), "plan must remain untouched after invalid name");
    }

    #[test]
    fn create_rejects_change_that_introduces_unknown_depends_on() {
        // We cannot introduce a cycle via a *new* entry alone (a new
        // entry has no backreferences), but the rollback path is
        // shared. Exercise it with an unknown-depends-on Error.
        let mut plan = plan_with_changes(vec![
            change("a", PlanStatus::Pending),
            change_with_deps("b", PlanStatus::Pending, &["a"]),
        ]);
        let c = change_with_deps("c", PlanStatus::Pending, &["does-not-exist"]);
        let err = plan.create(c).expect_err("unknown depends-on must roll back");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("plan validation failed after create"),
                    "rollback message missing, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        assert_eq!(plan.changes.len(), 2, "plan must still have only its original entries");
        let names: Vec<&str> = plan.changes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b"], "existing entries must be untouched");
    }

    #[test]
    fn create_rolls_back_on_validation_failure() {
        let mut plan = plan_with_changes(vec![change("foo", PlanStatus::Pending)]);
        let bar = change_with_deps("bar", PlanStatus::Pending, &["nonexistent"]);
        let err = plan.create(bar).expect_err("must Err");
        assert!(matches!(err, Error::Config(_)));
        assert_eq!(plan.changes.len(), 1, "plan length unchanged after rollback");
        assert_eq!(plan.changes[0].name, "foo");
        assert_eq!(plan.changes[0].status, PlanStatus::Pending);
        assert!(plan.changes[0].depends_on.is_empty());
    }

    #[test]
    fn amend_replaces_depends_on() {
        let mut plan = plan_with_changes(vec![
            change("a", PlanStatus::Pending),
            change_with_deps("b", PlanStatus::Pending, &["a"]),
        ]);
        let patch = PlanChangePatch {
            depends_on: Some(vec![]),
            ..PlanChangePatch::default()
        };
        plan.amend("b", patch).expect("amend ok");
        let b = plan.changes.iter().find(|c| c.name == "b").unwrap();
        assert!(b.depends_on.is_empty(), "depends_on should be replaced with empty vec");
    }

    #[test]
    fn amend_clear_vs_replace_description() {
        let mut plan = plan_with_changes(vec![PlanChange {
            name: "foo".into(),
            status: PlanStatus::Pending,
            depends_on: vec![],
            affects: vec![],
            sources: vec![],
            scope: BTreeMap::new(),
            description: Some("original".into()),
            status_reason: None,
        }]);

        plan.amend("foo", PlanChangePatch::default()).expect("amend none ok");
        assert_eq!(
            plan.changes[0].description.as_deref(),
            Some("original"),
            "None description must leave description unchanged"
        );

        plan.amend(
            "foo",
            PlanChangePatch {
                description: Some(None),
                ..PlanChangePatch::default()
            },
        )
        .expect("amend clear ok");
        assert_eq!(
            plan.changes[0].description, None,
            "Some(None) description must clear description"
        );

        plan.amend(
            "foo",
            PlanChangePatch {
                description: Some(Some("new".into())),
                ..PlanChangePatch::default()
            },
        )
        .expect("amend replace ok");
        assert_eq!(
            plan.changes[0].description.as_deref(),
            Some("new"),
            "Some(Some(s)) description must replace description"
        );
    }

    #[test]
    fn amend_leaves_unchanged_fields_alone() {
        // Need plan-level sources so the entry's `sources: ["a"]`
        // reference resolves.
        let plan = Plan {
            name: "test".into(),
            sources: {
                let mut m = BTreeMap::new();
                m.insert("a".to_string(), "/path/a".to_string());
                m
            },
            changes: vec![
                PlanChange {
                    name: "foo".into(),
                    status: PlanStatus::Pending,
                    depends_on: vec![],
                    affects: vec!["b".into()],
                    sources: vec!["a".into()],
                    scope: BTreeMap::new(),
                    description: Some("d".into()),
                    status_reason: None,
                },
                change("b", PlanStatus::Pending),
                change("x", PlanStatus::Pending),
            ],
        };
        let mut plan = plan;
        let patch = PlanChangePatch {
            depends_on: Some(vec!["x".into()]),
            ..PlanChangePatch::default()
        };
        plan.amend("foo", patch).expect("amend ok");
        let foo = plan.changes.iter().find(|c| c.name == "foo").unwrap();
        assert_eq!(foo.depends_on, vec!["x".to_string()]);
        assert_eq!(foo.affects, vec!["b".to_string()], "affects untouched");
        assert_eq!(foo.sources, vec!["a".to_string()], "sources untouched");
        assert_eq!(foo.description.as_deref(), Some("d"), "description untouched");
    }

    #[test]
    fn amend_rejects_missing_entry() {
        let mut plan = plan_with_changes(vec![change("foo", PlanStatus::Pending)]);
        let err = plan
            .amend("nonexistent", PlanChangePatch::default())
            .expect_err("missing entry must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("nonexistent"), "message should mention name, got: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn amend_rejects_patch_that_introduces_cycle() {
        let mut plan = plan_with_changes(vec![
            change("a", PlanStatus::Pending),
            change("b", PlanStatus::Pending),
        ]);

        plan.amend(
            "a",
            PlanChangePatch {
                depends_on: Some(vec!["b".into()]),
                ..PlanChangePatch::default()
            },
        )
        .expect("a -> [b] is acyclic; amend ok");

        let err = plan
            .amend(
                "b",
                PlanChangePatch {
                    depends_on: Some(vec!["a".into()]),
                    ..PlanChangePatch::default()
                },
            )
            .expect_err("introducing cycle must Err");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("plan validation failed after amend"),
                    "expected amend rollback message, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }

        let b = plan.changes.iter().find(|c| c.name == "b").unwrap();
        assert!(
            b.depends_on.is_empty(),
            "b.depends_on must be unchanged after failed amend, got {:?}",
            b.depends_on
        );
    }

    #[test]
    fn patch_has_no_status_field() {
        // Compile-time invariant: `PlanChangePatch` has no `status`
        // field, so `amend` literally cannot mutate lifecycle state.
        // The following line is commented out because it will not
        // compile — the field does not exist on the struct. Deleting
        // the comment and uncommenting the line is the only way to
        // violate the single-writer-for-status invariant, and it
        // fails to build, which is the entire point.
        //
        // let _ = PlanChangePatch { status: PlanStatus::Pending, ..Default::default() };
        //
        // Runtime assertion below is a smoke test that the type is
        // `Default`-constructible and that the four fields we do have
        // default to `None`.
        let patch = PlanChangePatch::default();
        assert!(patch.depends_on.is_none());
        assert!(patch.affects.is_none());
        assert!(patch.sources.is_none());
        assert!(patch.description.is_none());
    }

    #[test]
    fn transition_applies_legal_edge_and_clears_reason_on_pending_reentry() {
        let mut plan = plan_with_changes(vec![PlanChange {
            name: "a".into(),
            status: PlanStatus::Failed,
            depends_on: vec![],
            affects: vec![],
            sources: vec![],
            scope: BTreeMap::new(),
            description: None,
            status_reason: Some("crashed".into()),
        }]);
        plan.transition("a", PlanStatus::Pending, None).expect("failed -> pending ok");
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, PlanStatus::Pending);
        assert_eq!(a.status_reason, None, "re-entry to Pending must clear status_reason");
    }

    #[test]
    fn transition_writes_reason_on_blocked_failed_skipped() {
        let mut plan = plan_with_changes(vec![
            change("a", PlanStatus::Pending),
            change("b", PlanStatus::InProgress),
            change("c", PlanStatus::Failed),
        ]);

        plan.transition("a", PlanStatus::Blocked, Some("needs scope"))
            .expect("pending -> blocked ok");
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, PlanStatus::Blocked);
        assert_eq!(a.status_reason.as_deref(), Some("needs scope"));

        plan.transition("b", PlanStatus::Failed, Some("broken")).expect("in-progress -> failed ok");
        let b = plan.changes.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.status, PlanStatus::Failed);
        assert_eq!(b.status_reason.as_deref(), Some("broken"));

        plan.transition("c", PlanStatus::Skipped, Some("abandoned")).expect("failed -> skipped ok");
        let c = plan.changes.iter().find(|c| c.name == "c").unwrap();
        assert_eq!(c.status, PlanStatus::Skipped);
        assert_eq!(c.status_reason.as_deref(), Some("abandoned"));
    }

    #[test]
    fn transition_rejects_reason_on_pending_inprogress_done_target() {
        let mut plan = plan_with_changes(vec![
            change("a", PlanStatus::Pending),
            change("b", PlanStatus::InProgress),
        ]);

        let err = plan
            .transition("a", PlanStatus::InProgress, Some("why"))
            .expect_err("reason on InProgress target must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("--reason"), "message should mention --reason: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, PlanStatus::Pending, "a.status must be unchanged");

        let err = plan
            .transition("b", PlanStatus::Done, Some("why"))
            .expect_err("reason on Done target must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("--reason"), "message should mention --reason: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        let b = plan.changes.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.status, PlanStatus::InProgress, "b.status must be unchanged");
    }

    #[test]
    fn transition_rejects_illegal_edge_via_state_machine() {
        let mut plan = plan_with_changes(vec![change("a", PlanStatus::Done)]);
        let err = plan
            .transition("a", PlanStatus::Pending, None)
            .expect_err("Done -> Pending must Err from state machine");
        match err {
            Error::PlanTransition { from, to } => {
                assert_eq!(from, "Done");
                assert_eq!(to, "Pending");
            }
            other => panic!("expected Error::PlanTransition, got {other:?}"),
        }
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, PlanStatus::Done, "status must not be mutated on illegal edge");
    }

    #[test]
    fn transition_rejects_missing_entry() {
        let mut plan = plan_with_changes(vec![change("foo", PlanStatus::Pending)]);
        let err = plan
            .transition("nonexistent", PlanStatus::InProgress, None)
            .expect_err("missing entry must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("nonexistent"), "message should mention name: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    // --- L1.G: Plan::archive ---------------------------------------------

    /// Build a plan at `<dir>/plan.yaml` with the given name + entries and
    /// return the plan path.
    fn write_plan(dir: &Path, name: &str, changes: Vec<PlanChange>) -> PathBuf {
        let plan = Plan {
            name: name.to_string(),
            sources: BTreeMap::new(),
            changes,
        };
        let path = dir.join("plan.yaml");
        plan.save(&path).expect("save plan");
        path
    }

    fn today_yyyymmdd() -> String {
        chrono::Utc::now().format("%Y%m%d").to_string()
    }

    #[test]
    fn archive_happy_path_with_only_done_and_skipped() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path = write_plan(
            tmp.path(),
            "release-1",
            vec![
                change("a", PlanStatus::Done),
                change("b", PlanStatus::Skipped),
                change("c", PlanStatus::Done),
            ],
        );
        let pre_bytes = std::fs::read(&plan_path).expect("read pre-archive");

        let (dest, plans_dir) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        assert!(!plan_path.exists(), "original plan.yaml must be gone after archive");
        assert!(dest.exists(), "destination archive file must exist");
        let expected = archive_dir.join(format!("release-1-{}.yaml", today_yyyymmdd()));
        assert_eq!(dest, expected);
        assert!(plans_dir.is_none(), "no working dir means archived_plans_dir must be None");

        let post_bytes = std::fs::read(&dest).expect("read post-archive");
        assert_eq!(
            pre_bytes, post_bytes,
            "archived file must be byte-identical to the pre-archive plan"
        );
    }

    #[test]
    fn archive_creates_missing_archive_dir() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("does").join("not").join("exist").join("yet");
        assert!(!archive_dir.exists());
        let plan_path = write_plan(tmp.path(), "proj", vec![change("a", PlanStatus::Done)]);

        let (dest, _) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        assert!(archive_dir.is_dir(), "archive_dir must be created");
        assert!(dest.starts_with(&archive_dir));
        assert!(dest.exists());
    }

    #[test]
    fn archive_refuses_with_pending_entries_without_force() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path = write_plan(
            tmp.path(),
            "p",
            vec![
                change("done-one", PlanStatus::Done),
                change("still-pending", PlanStatus::Pending),
            ],
        );

        let err = Plan::archive(&plan_path, &archive_dir, false)
            .expect_err("must refuse pending entry without force");
        match err {
            Error::PlanHasOutstandingWork { entries } => {
                assert_eq!(entries, vec!["still-pending".to_string()]);
            }
            other => panic!("expected PlanHasOutstandingWork, got {other:?}"),
        }

        assert!(plan_path.exists(), "original plan.yaml must still exist");
        if archive_dir.exists() {
            let count =
                std::fs::read_dir(&archive_dir).expect("read_dir").filter_map(Result::ok).count();
            assert_eq!(count, 0, "no archived file should have been written");
        }
    }

    #[test]
    fn archive_refuses_with_blocked_failed_in_progress() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path = write_plan(
            tmp.path(),
            "p",
            vec![
                change("a", PlanStatus::Done),
                change("b", PlanStatus::InProgress),
                change("c", PlanStatus::Blocked),
                change("d", PlanStatus::Failed),
                change("e", PlanStatus::Skipped),
            ],
        );

        let err = Plan::archive(&plan_path, &archive_dir, false)
            .expect_err("must refuse non-terminal entries");
        match err {
            Error::PlanHasOutstandingWork { entries } => {
                assert_eq!(
                    entries,
                    vec!["b".to_string(), "c".to_string(), "d".to_string()],
                    "entries must include InProgress/Blocked/Failed in plan list order, \
                     excluding Done and Skipped"
                );
            }
            other => panic!("expected PlanHasOutstandingWork, got {other:?}"),
        }
        assert!(plan_path.exists(), "original plan.yaml must still exist");
    }

    #[test]
    fn archive_with_force_succeeds_even_with_outstanding_entries() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path = write_plan(
            tmp.path(),
            "p",
            vec![
                change("a", PlanStatus::Done),
                change("b", PlanStatus::InProgress),
                change("c", PlanStatus::Blocked),
                change("d", PlanStatus::Failed),
                change("e", PlanStatus::Skipped),
            ],
        );
        let pre_bytes = std::fs::read(&plan_path).expect("read pre-archive");

        let (dest, _) = Plan::archive(&plan_path, &archive_dir, true).expect("force archive ok");

        assert!(!plan_path.exists(), "original plan.yaml must be gone after forced archive");
        let post_bytes = std::fs::read(&dest).expect("read archived file");
        assert_eq!(
            pre_bytes, post_bytes,
            "forced archive must preserve every entry (including non-terminal) verbatim"
        );

        let archived: Plan = serde_yaml::from_slice(&post_bytes).expect("parse archived");
        let statuses: Vec<PlanStatus> = archived.changes.iter().map(|c| c.status).collect();
        assert_eq!(
            statuses,
            vec![
                PlanStatus::Done,
                PlanStatus::InProgress,
                PlanStatus::Blocked,
                PlanStatus::Failed,
                PlanStatus::Skipped,
            ],
            "statuses in archive must not be rewritten"
        );
    }

    #[test]
    fn archive_filename_is_kebab_plan_name_plus_yyyymmdd() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path =
            write_plan(tmp.path(), "my-initiative", vec![change("a", PlanStatus::Done)]);

        let (dest, _) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");
        let basename = dest.file_name().and_then(|s| s.to_str()).expect("basename utf8");

        // Regex: ^my-initiative-\d{8}\.yaml$ — implemented without a
        // regex crate dep by structural decomposition.
        let prefix = "my-initiative-";
        let suffix = ".yaml";
        assert!(basename.starts_with(prefix), "basename should start with prefix, got {basename}");
        assert!(basename.ends_with(suffix), "basename should end with .yaml, got {basename}");
        let middle = &basename[prefix.len()..basename.len() - suffix.len()];
        assert_eq!(middle.len(), 8, "date segment must be 8 chars, got {middle}");
        assert!(
            middle.chars().all(|ch| ch.is_ascii_digit()),
            "date segment must be all digits, got {middle}"
        );
    }

    #[test]
    fn archive_refuses_when_destination_exists() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        std::fs::create_dir_all(&archive_dir).expect("mkdir archive");

        let plan_path = write_plan(tmp.path(), "dup", vec![change("a", PlanStatus::Done)]);

        let existing = archive_dir.join(format!("dup-{}.yaml", today_yyyymmdd()));
        std::fs::write(&existing, "unrelated pre-existing archive").expect("seed existing");

        let err = Plan::archive(&plan_path, &archive_dir, false)
            .expect_err("must refuse when destination exists");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("already exists"),
                    "message should say 'already exists', got: {msg}"
                );
                assert!(msg.contains("git mv"), "message should suggest `git mv`, got: {msg}");
                assert!(
                    msg.contains("wait until tomorrow to re-archive"),
                    "message should mention the tomorrow-re-archive fallback, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }

        assert!(plan_path.exists(), "original plan.yaml must not be moved");
        let leftover = std::fs::read_to_string(&existing).expect("read existing");
        assert_eq!(
            leftover, "unrelated pre-existing archive",
            "pre-existing archive file must not be overwritten"
        );
    }

    #[test]
    fn archive_returns_destination_path() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path = write_plan(tmp.path(), "pkg", vec![change("a", PlanStatus::Done)]);

        let (dest, plans_dir) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");
        let expected = archive_dir.join(format!("pkg-{}.yaml", today_yyyymmdd()));
        assert_eq!(dest, expected);
        assert!(dest.exists(), "returned path must point at an existing file");
        assert!(plans_dir.is_none(), "no working dir co-moved");
    }

    // --- L3.A: Plan::init ------------------------------------------------

    #[test]
    fn init_returns_empty_plan_with_given_name() {
        let plan = Plan::init("platform-v2", BTreeMap::new()).expect("init ok");
        assert_eq!(plan.name, "platform-v2");
        assert!(plan.sources.is_empty(), "sources should default to empty");
        assert!(plan.changes.is_empty(), "changes should default to empty");
    }

    #[test]
    fn init_preserves_sources_map() {
        let mut sources = BTreeMap::new();
        sources.insert("monolith".to_string(), "/path/to/legacy".to_string());
        sources.insert("orders".to_string(), "git@github.com:org/orders.git".to_string());
        sources.insert("payments".to_string(), "git@github.com:org/payments.git".to_string());

        let plan = Plan::init("big", sources.clone()).expect("init ok");
        assert_eq!(plan.sources, sources, "init must preserve the sources map verbatim");
        assert_eq!(plan.sources.len(), 3);
    }

    #[test]
    fn init_rejects_invalid_name() {
        let err = Plan::init("BAD_NAME", BTreeMap::new()).expect_err("invalid name must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("kebab-case"), "expected kebab-case in message, got: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn init_accepts_kebab_case() {
        let plan = Plan::init("a-b-c", BTreeMap::new()).expect("kebab name accepted");
        assert_eq!(plan.name, "a-b-c");
    }

    #[test]
    fn init_output_passes_validation() {
        let plan = Plan::init("foo", BTreeMap::new()).expect("init ok");
        let findings = plan.validate(None, None);
        assert!(
            findings.is_empty(),
            "freshly-scaffolded plan must pass validation, got: {findings:#?}"
        );
    }

    // --- L3.B: archive co-moves .specify/plans/<name>/ --------------------

    /// Build `<dir>/.specify/plan.yaml` with the given name + entries,
    /// plus a working directory at `<dir>/.specify/plans/<name>/`
    /// populated by `files` (filename → bytes). Returns the plan path
    /// so callers can hand it to `Plan::archive` unchanged.
    fn write_plan_with_working_dir(
        dir: &Path, name: &str, changes: Vec<PlanChange>, files: &[(&str, &[u8])],
    ) -> PathBuf {
        let specify = dir.join(".specify");
        std::fs::create_dir_all(&specify).expect("mkdir .specify");
        let plan_path = write_plan(&specify, name, changes);

        let plans_dir = specify.join("plans").join(name);
        std::fs::create_dir_all(&plans_dir).expect("mkdir plans dir");
        for (filename, bytes) in files {
            let target = plans_dir.join(filename);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("mkdir nested working-dir path");
            }
            std::fs::write(&target, bytes).expect("seed working file");
        }

        plan_path
    }

    #[test]
    fn archive_with_working_dir_moves_both() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join(".specify/archive/plans");
        let plan_path = write_plan_with_working_dir(
            tmp.path(),
            "foo",
            vec![change("a", PlanStatus::Done)],
            &[("discovery.md", b"# discovery\n"), ("proposal.md", b"# proposal\n")],
        );
        let working_dir = tmp.path().join(".specify/plans/foo");
        let pre_bytes = std::fs::read(&plan_path).expect("read pre-archive");

        let (dest, dest_working) =
            Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        let today = today_yyyymmdd();
        assert!(!plan_path.exists(), "plan.yaml must be gone");
        assert!(!working_dir.exists(), ".specify/plans/foo/ must be gone");
        assert_eq!(dest, archive_dir.join(format!("foo-{today}.yaml")));
        assert_eq!(
            dest_working,
            Some(archive_dir.join(format!("foo-{today}"))),
            "co-move must surface the destination dir in the return"
        );

        let dest_dir = dest_working.expect("co-moved");
        assert!(dest_dir.is_dir(), "archived plans dir must exist");
        let discovered =
            std::fs::read(dest_dir.join("discovery.md")).expect("read archived discovery.md");
        assert_eq!(discovered, b"# discovery\n");
        let proposal =
            std::fs::read(dest_dir.join("proposal.md")).expect("read archived proposal.md");
        assert_eq!(proposal, b"# proposal\n");

        // plan.yaml must still survive the move byte-for-byte.
        let post_bytes = std::fs::read(&dest).expect("read archived plan");
        assert_eq!(pre_bytes, post_bytes);
    }

    #[test]
    fn archive_without_working_dir_still_succeeds() {
        let tmp = tempdir().expect("tempdir");
        let specify = tmp.path().join(".specify");
        std::fs::create_dir_all(&specify).expect("mkdir .specify");
        let archive_dir = specify.join("archive/plans");
        let plan_path = write_plan(&specify, "solo", vec![change("a", PlanStatus::Done)]);

        let (dest, dest_working) =
            Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        assert!(dest.exists(), "archived plan.yaml must exist");
        assert!(dest_working.is_none(), "no working dir -> None");

        let absent_dir = archive_dir.join(format!("solo-{}", today_yyyymmdd()));
        assert!(!absent_dir.exists(), "co-move dir must not be created when source absent");
    }

    /// RFC-3a C14 archive-sweep hook. `.specify/initiative.md` is
    /// initiative-scoped and must travel with the archive. When there
    /// is no plans working directory, the initiative brief alone
    /// still forces the archived `<name>-<date>/` directory into
    /// existence. RFC-3a C33 asserts `workspace.md` + `slices/` under
    /// `.specify/plans/<name>/` survive the wholesale co-move (see
    /// `archive_moves_workspace_md_and_slices_with_plan_working_dir`).
    #[test]
    fn archive_sweeps_initiative_md_alongside_plan() {
        let tmp = tempdir().expect("tempdir");
        let specify = tmp.path().join(".specify");
        std::fs::create_dir_all(&specify).expect("mkdir .specify");
        let archive_dir = specify.join("archive/plans");
        let plan_path = write_plan(&specify, "solo", vec![change("a", PlanStatus::Done)]);
        let brief_src = specify.join("initiative.md");
        let brief_bytes = b"---\nname: solo\n---\n\n# Solo\n";
        std::fs::write(&brief_src, brief_bytes).expect("seed initiative.md");

        let (dest, dest_working) =
            Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        assert!(dest.exists(), "archived plan.yaml must exist");
        let dest_dir = dest_working.expect("initiative.md must force the archive dir");
        let archived_brief = dest_dir.join("initiative.md");
        assert!(
            archived_brief.is_file(),
            "archived initiative.md missing at {}",
            archived_brief.display()
        );
        assert_eq!(
            std::fs::read(&archived_brief).expect("read archived brief"),
            brief_bytes,
            "archived bytes must equal source bytes"
        );
        assert!(!brief_src.exists(), "source initiative.md must be gone after move");
    }

    /// Same sweep, but with a plans working directory also present.
    /// Both files land in the same archived `<name>-<date>/` dir.
    #[test]
    fn archive_sweeps_initiative_md_and_working_dir_together() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join(".specify/archive/plans");
        let plan_path = write_plan_with_working_dir(
            tmp.path(),
            "both",
            vec![change("a", PlanStatus::Done)],
            &[("notes.md", b"# notes\n")],
        );
        let brief_src = tmp.path().join(".specify/initiative.md");
        std::fs::write(&brief_src, b"---\nname: both\n---\n\n# Both\n")
            .expect("seed initiative.md");

        let (_, dest_working) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        let dest_dir = dest_working.expect("co-moved");
        assert!(dest_dir.join("notes.md").is_file(), "working-dir file must co-move");
        assert!(dest_dir.join("initiative.md").is_file(), "initiative.md must co-move");
    }

    /// RFC-3a C33 — `workspace.md` and nested `slices/` under the plan
    /// working directory move with the wholesale `.specify/plans/<name>/`
    /// rename into the archive tree.
    #[test]
    fn archive_moves_workspace_md_and_slices_with_plan_working_dir() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join(".specify/archive/plans");
        let plan_path = write_plan_with_working_dir(
            tmp.path(),
            "traffic",
            vec![change("a", PlanStatus::Done)],
            &[("workspace.md", b"# workspace\n"), ("slices/x.yaml", b"id: slice-x\n")],
        );

        let (_, dest_working) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        let dest_dir = dest_working.expect("co-moved plans dir");
        let wm = dest_dir.join("workspace.md");
        let slice = dest_dir.join("slices").join("x.yaml");
        assert!(wm.is_file(), "workspace.md missing at {}", wm.display());
        assert!(slice.is_file(), "slices/x.yaml missing at {}", slice.display());
        assert_eq!(std::fs::read(&wm).expect("read"), b"# workspace\n");
        assert_eq!(std::fs::read(&slice).expect("read"), b"id: slice-x\n");
    }

    #[test]
    fn archive_refuses_when_working_dir_destination_exists() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join(".specify/archive/plans");
        let plan_path = write_plan_with_working_dir(
            tmp.path(),
            "foo",
            vec![change("a", PlanStatus::Done)],
            &[("discovery.md", b"# discovery\n")],
        );
        let working_dir = tmp.path().join(".specify/plans/foo");

        // Pre-create the co-move destination; the plan.yaml
        // destination is clear, so this exercises the working-dir
        // preflight specifically.
        let today = today_yyyymmdd();
        let clash = archive_dir.join(format!("foo-{today}"));
        std::fs::create_dir_all(&clash).expect("seed collision dir");

        let err = Plan::archive(&plan_path, &archive_dir, false)
            .expect_err("must refuse when working-dir destination exists");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("already exists"),
                    "message should mention 'already exists', got: {msg}"
                );
                assert!(
                    msg.contains(&format!("foo-{today}")),
                    "message should name the colliding path, got: {msg}"
                );
                assert!(msg.contains("git mv"), "message should suggest `git mv`, got: {msg}");
                assert!(
                    msg.contains("wait until tomorrow to re-archive"),
                    "message should mention the tomorrow-re-archive fallback, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }

        // Critical: plan.yaml must NOT be moved. The preflight runs
        // before any mutation, so a collision leaves the source tree
        // exactly as it was.
        assert!(plan_path.exists(), "plan.yaml must be untouched on preflight failure");
        assert!(working_dir.is_dir(), "working dir must be untouched on preflight failure");
        assert!(
            clash.is_dir() && std::fs::read_dir(&clash).expect("read").next().is_none(),
            "pre-existing collision dir must remain empty and untouched"
        );
        let archived_plan = archive_dir.join(format!("foo-{today}.yaml"));
        assert!(!archived_plan.exists(), "plan.yaml must not have been archived");
    }

    #[test]
    fn archive_preserves_working_dir_contents_byte_for_byte() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join(".specify/archive/plans");
        let payload: &[u8] = b"line-1\nline-2\nunicode: caf\xc3\xa9\n";
        let plan_path = write_plan_with_working_dir(
            tmp.path(),
            "bytes",
            vec![change("a", PlanStatus::Done)],
            &[("artefact.bin", payload)],
        );

        let (_, dest_working) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        let dest_dir = dest_working.expect("co-moved");
        let read_back =
            std::fs::read(dest_dir.join("artefact.bin")).expect("read archived artefact");
        assert_eq!(read_back, payload, "archived bytes must equal source bytes exactly");
    }

    #[test]
    fn archive_with_force_preserves_nonterminal_entries_and_moves_working_dir() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join(".specify/archive/plans");
        let plan_path = write_plan_with_working_dir(
            tmp.path(),
            "mixed",
            vec![
                change("done-one", PlanStatus::Done),
                change("still-pending", PlanStatus::Pending),
            ],
            &[("notes.md", b"# notes\n")],
        );
        let working_dir = tmp.path().join(".specify/plans/mixed");
        let pre_bytes = std::fs::read(&plan_path).expect("read pre-archive");

        let (dest, dest_working) =
            Plan::archive(&plan_path, &archive_dir, true).expect("force archive ok");

        assert!(!plan_path.exists(), "plan.yaml must be gone");
        assert!(!working_dir.exists(), "working dir must be gone");
        assert!(dest.exists());
        let dest_dir = dest_working.expect("force must still co-move");
        assert!(dest_dir.join("notes.md").exists(), "working file must survive the co-move");

        // Forced archive must preserve non-terminal statuses verbatim.
        let post_bytes = std::fs::read(&dest).expect("read archived plan");
        assert_eq!(pre_bytes, post_bytes, "forced archive must preserve plan bytes exactly");
    }

    /// Same-device happy path: `fs::rename` is atomic on a single
    /// filesystem. The `EXDEV` fallback (copy + remove) exercised by
    /// `actions::move_atomic` only fires across filesystems; a single
    /// `tempdir()` sits on one mount, so the cross-device path is not
    /// unit-testable here without a platform-specific loopback mount.
    /// The fallback's directory branch is covered by `actions.rs`
    /// unit tests; the file branch is the same implementation.
    #[test]
    fn archive_is_atomic_within_filesystem() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path = write_plan(tmp.path(), "atomic", vec![change("a", PlanStatus::Done)]);
        let pre_bytes = std::fs::read(&plan_path).expect("read pre-archive");

        let (dest, _) = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        assert!(!plan_path.exists());
        assert!(dest.exists());
        assert_eq!(
            std::fs::read(&dest).expect("read archived"),
            pre_bytes,
            "rename-on-same-fs must preserve byte content"
        );
    }
}
