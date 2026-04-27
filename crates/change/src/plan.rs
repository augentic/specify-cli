//! On-disk representation of `.specify/plan.yaml` and the in-memory
//! [`Plan`] state machine that wraps it.
//!
//! See `rfcs/rfc-2-execution.md` §"Library Implementation" for the canonical
//! type surface and `rfcs/rfc-2-execution.md` §"The Plan" for the reference
//! YAML fixture exercised by the round-trip tests.
//!
//! ## Single-writer invariant for `Entry::status`
//!
//! The only path that mutates an existing [`Entry::status`] is
//! [`Plan::transition`]. This is not just a convention — it's enforced
//! by the shape of the API:
//!
//!   - [`Plan::create`] appends a new entry and forces its `status` to
//!     [`Status::Pending`]; any other value the caller supplied is
//!     silently overwritten and `status_reason` is cleared.
//!   - [`Plan::amend`] takes a [`EntryPatch`] which structurally
//!     has no `status` (or `status_reason`) field — a type-system
//!     guarantee that `amend` cannot mutate lifecycle state.
//!   - [`Plan::transition`] delegates to [`Status::transition`]
//!     for edge-legality and is the only place that writes
//!     `entry.status` or `entry.status_reason`.
//!
//! Any future writer of `status` should route through `Plan::transition`
//! rather than poking the field directly, or add a new `Plan::*`
//! method that does. Bypassing this invariant undoes the state-machine
//! guarantees exercised by the L1.B transition tests.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use petgraph::Direction;
use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use specify_error::Error;
use specify_schema::Registry;

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
#[non_exhaustive]
pub enum Status {
    /// Not yet started.
    Pending,
    /// Currently being executed.
    InProgress,
    /// Completed successfully.
    Done,
    /// Blocked on an external dependency or question.
    Blocked,
    /// Execution failed.
    Failed,
    /// Intentionally skipped.
    Skipped,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::InProgress => "in-progress",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        })
    }
}

impl Status {
    /// Every variant in declaration order. Used by exhaustive transition
    /// tests here and by validation/topological code in L1.D/E that
    /// needs to enumerate states without depending on `strum`.
    pub const ALL: [Self; 6] =
        [Self::Pending, Self::InProgress, Self::Done, Self::Blocked, Self::Failed, Self::Skipped];

    /// Whether `self -> target` is a legal edge in the plan-entry state
    /// machine. See `rfc-2-execution.md` §"Transition Rules" for the canonical
    /// table; the 10 edges enumerated below are the *only* legal ones.
    /// `Done` is terminal: every edge with `Done` on the left is `false`.
    #[must_use]
    pub const fn can_transition_to(&self, target: &Self) -> bool {
        use Status::{Blocked, Done, Failed, InProgress, Pending, Skipped};
        matches!(
            (self, target),
            (Pending, InProgress | Blocked | Skipped)
                | (InProgress, Done | Failed | Blocked)
                | (Blocked | Failed | Skipped, Pending)
                | (Failed, Skipped)
        )
    }

    /// Return `target` if the edge is legal, otherwise an
    /// `Error::PlanTransition` carrying both endpoints by their `Debug`
    /// representation. Mirrors `LifecycleStatus::transition`.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn transition(&self, target: Self) -> Result<Self, Error> {
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
/// A `Plan` is an ordered, dependency-aware list of [`Entry`]s plus
/// a named map of [`Plan::sources`] (local paths or git URLs) that the
/// entries draw from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Plan {
    /// Human-readable plan name, e.g. `platform-v2`.
    pub name: String,
    /// Named source locations referenced by [`Entry::sources`].
    /// Optional in the YAML; defaults to an empty map.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Ordered list of plan entries. Order is the *intended* execution
    /// order; the authoritative dependency-respecting order comes from
    /// [`Plan::topological_order`].
    pub changes: Vec<Entry>,
}

/// One entry in [`Plan::changes`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Entry {
    /// Stable identifier (kebab-case) unique within the plan.
    pub name: String,
    /// Target registry project (RFC-3b). Required for multi-project registries.
    #[serde(default)]
    pub project: Option<String>,
    /// Schema identifier for project-less entries (e.g. `contracts@v1`).
    /// Required when `project` is `None`; optional override when `project`
    /// is `Some`. Mutually enriching with `project`: `project` identifies
    /// the target codebase; `schema` identifies the schema directly.
    #[serde(default)]
    pub schema: Option<String>,
    /// Current lifecycle state of this entry.
    pub status: Status,
    /// Names of other plan entries that must reach `done` before this
    /// entry is eligible.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Source keys (into [`Plan::sources`]) this entry draws from.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Baseline paths relevant to this change, relative to `.specify/`.
    /// Briefs use these as a focus hint when scanning baseline directories.
    /// Populated by `/spec:plan` or manually via `specify plan create --context`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
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

/// Patch applied by [`Plan::amend`] to an existing entry.
///
/// Every field is `Option<T>`; `None` means "leave unchanged", `Some(v)`
/// means "replace with v". `status` and `status_reason` are deliberately
/// absent — status transitions are made via [`Plan::transition`], never
/// through `amend`, and the reason field travels with the transition.
///
/// The absence of a `status` field is a type-system guarantee: `amend`
/// cannot mutate status. Transitions go via [`Plan::transition`].
#[derive(Debug, Default, Clone)]
pub struct EntryPatch {
    /// Replace `depends_on` wholesale when `Some`.
    pub depends_on: Option<Vec<String>>,
    /// Replace `sources` wholesale when `Some`.
    pub sources: Option<Vec<String>>,
    /// Replace `project` when `Some(Some(..))`; clear when
    /// `Some(None)`; leave unchanged when `None`.
    pub project: Option<Option<String>>,
    /// Replace `schema` when `Some(Some(..))`; clear when
    /// `Some(None)`; leave unchanged when `None`.
    pub schema: Option<Option<String>>,
    /// Replace `description` when `Some(Some(..))`; clear when
    /// `Some(None)`; leave unchanged when `None`.
    pub description: Option<Option<String>>,
    /// Replace `context` wholesale when `Some`.
    pub context: Option<Vec<String>>,
}

/// Severity of a validation finding produced by [`Plan::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    /// Blocking problem — the plan is not usable as-is.
    Error,
    /// Non-blocking advisory — the plan is usable but something looks
    /// off (e.g. a source key is defined but unreferenced).
    Warning,
}

/// A single finding reported by [`Plan::validate`].
#[derive(Debug, Clone)]
pub struct Finding {
    /// Severity bucket.
    pub level: Severity,
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
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn init(name: &str, sources: BTreeMap<String, String>) -> Result<Self, Error> {
        crate::actions::validate_name(name)?;
        Ok(Self {
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
    /// `serde_saphyr::from_str` accepts both.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(path: &Path) -> Result<Self, Error> {
        if !path.exists() {
            return Err(Error::ArtifactNotFound {
                kind: "plan.yaml",
                path: path.to_path_buf(),
            });
        }
        let content = std::fs::read_to_string(path)?;
        let plan: Self = serde_saphyr::from_str(&content)?;
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
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        crate::atomic::atomic_yaml_write(path, self)
    }

    /// Run all structural and semantic checks over the plan.
    ///
    /// `changes_dir` (when `Some`) points at `.specify/changes/` and
    /// enables the cross-reference checks against on-disk change
    /// metadata. `registry` (when `Some`) enables the RFC-3b
    /// cross-registry checks (`project-not-in-registry`,
    /// `project-missing-multi-repo`).
    ///
    /// Findings are accumulated — no check short-circuits another. Order
    /// is structural checks first (duplicate names, cycles, unknown
    /// depends-on / sources, multiple in-progress) followed by
    /// consistency checks against `changes_dir` when provided.
    ///
    /// Note on "well-formed status values": `Status` is an enum, so
    /// every in-memory instance is well-formed by construction. serde
    /// rejects invalid statuses at parse time. The RFC lists this check
    /// for completeness against hand-edited YAML that bypassed parsing,
    /// which is not reachable in-process — so nothing is emitted for it.
    #[must_use]
    pub fn validate(
        &self, changes_dir: Option<&Path>, registry: Option<&Registry>,
    ) -> Vec<Finding> {
        let mut results = Vec::new();
        results.extend(duplicate_names(&self.changes));
        results.extend(detect_cycles(&self.changes));
        results.extend(check_unknown_depends_on(&self.changes));
        results.extend(check_unknown_sources(self));
        results.extend(check_single_in_progress(&self.changes));
        results.extend(missing_project_or_schema(&self.changes));
        results.extend(check_context_paths(&self.changes));
        if let Some(reg) = registry {
            results.extend(check_project_in_registry(&self.changes, reg));
            results.extend(check_project_required_multi_repo(&self.changes, reg));
        }
        if let Some(dir) = changes_dir.filter(|d| d.is_dir()) {
            results.extend(changes_dir_consistency(self, dir));
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
    #[must_use]
    pub fn next_eligible(&self) -> Option<&Entry> {
        if self.changes.iter().any(|c| c.status == Status::InProgress) {
            return None;
        }
        let status_by_name: HashMap<&str, Status> =
            self.changes.iter().map(|c| (c.name.as_str(), c.status)).collect();
        self.changes.iter().find(|c| {
            c.status == Status::Pending
                && c.depends_on
                    .iter()
                    .all(|dep| status_by_name.get(dep.as_str()).copied() == Some(Status::Done))
        })
    }

    /// Transition the named entry to `target`, recording `reason` in
    /// [`Entry::status_reason`] per the rules documented in
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
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn transition(
        &mut self, name: &str, target: Status, reason: Option<&str>,
    ) -> Result<(), Error> {
        let entry = self
            .changes
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| Error::Config(format!("no change named '{name}' in plan")))?;

        let new_status = entry.status.transition(target)?;

        match target {
            Status::Failed | Status::Blocked | Status::Skipped => {
                if let Some(s) = reason {
                    entry.status_reason = Some(s.to_string());
                }
            }
            Status::Pending | Status::InProgress | Status::Done => {
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
    /// [`Status::Pending`] (and `status_reason` cleared) so that
    /// creation cannot introduce a pre-occupied lifecycle state — the
    /// single-writer-for-status invariant documented at the top of
    /// this module.
    ///
    /// After mutation, the plan is re-validated. Any `Error`-level
    /// finding (unknown `depends_on`/`sources`, cycle
    /// introduced by the new entry, etc.) rolls back the append and
    /// returns `Error::Config` containing the first offending
    /// finding's message. Warnings are tolerated — they're a CLI
    /// concern, not a library-level hard stop.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn create(&mut self, change: Entry) -> Result<(), Error> {
        crate::actions::validate_name(&change.name)?;

        if self.changes.iter().any(|c| c.name == change.name) {
            return Err(Error::Config(format!(
                "plan already contains a change named '{}'",
                change.name
            )));
        }

        let mut change = change;
        change.status = Status::Pending;
        change.status_reason = None;

        // Targeted rollback: pop the freshly-appended entry if
        // validation rejects the resulting plan. Cheaper than cloning
        // the whole `changes` vector since we know the mutation is a
        // single trailing push.
        self.changes.push(change);
        let errors: Vec<Finding> = self
            .validate(None, None)
            .into_iter()
            .filter(|r| r.level == Severity::Error)
            .collect();
        if let Some(first) = errors.first() {
            let msg = first.message.clone();
            self.changes.pop();
            return Err(Error::Config(format!("plan validation failed after create: {msg}")));
        }

        Ok(())
    }

    /// Apply `patch` to the entry named `name`. `None` fields on the
    /// patch leave the corresponding [`Entry`] field unchanged;
    /// `Some(v)` replaces wholesale. `description` is three-way:
    /// `None` = leave, `Some(None)` = clear, `Some(Some(s))` =
    /// replace. `status` is intentionally not patchable — see
    /// [`EntryPatch`] and the module-level single-writer note.
    ///
    /// After mutation, the plan is re-validated. Any `Error`-level
    /// finding reverts the single-entry mutation (we snapshot the
    /// pre-mutation entry at the top of the function and write it
    /// back on failure) and returns `Error::Config`.
    ///
    /// `amend` does not consult `Entry::status` — it is legal to
    /// amend the currently-`in-progress` entry's non-status fields,
    /// per RFC-2 §"Phase Boundary → Rule 2".
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn amend(&mut self, name: &str, patch: EntryPatch) -> Result<(), Error> {
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
            if let Some(v) = patch.sources {
                entry.sources = v;
            }
            if let Some(v) = patch.project {
                entry.project = v;
            }
            if let Some(v) = patch.schema {
                entry.schema = v;
            }
            if let Some(v) = patch.description {
                entry.description = v;
            }
            if let Some(v) = patch.context {
                entry.context = v;
            }
        }

        let errors: Vec<Finding> = self
            .validate(None, None)
            .into_iter()
            .filter(|r| r.level == Severity::Error)
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
    ///
    /// # Panics
    ///
    /// Panics if the internal indegree map is inconsistent (should never
    /// happen in practice since every node is inserted during init).
    ///
    /// # Errors
    ///
    /// Returns an error if the dependency graph contains a cycle.
    pub fn topological_order(&self) -> Result<Vec<&Entry>, Error> {
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
                .map_or_else(|| "<unknown>".to_string(), |scc| graph[scc[0]].to_string());
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
        let mut ready: BinaryHeap<Reverse<NodeIndex>> =
            indegree.iter().filter_map(|(&n, &d)| (d == 0).then_some(Reverse(n))).collect();

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

        let mut output: Vec<&Entry> = self.changes.iter().collect();
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
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn archive(
        path: &Path, archive_dir: &Path, force: bool,
    ) -> Result<(PathBuf, Option<PathBuf>), Error> {
        let plan = Self::load(path)?;

        if !force {
            let entries: Vec<String> = plan
                .changes
                .iter()
                .filter(|c| !matches!(c.status, Status::Done | Status::Skipped))
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

fn duplicate_names(changes: &[Entry]) -> Vec<Finding> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for entry in changes {
        if !seen.insert(entry.name.as_str()) {
            out.push(Finding {
                level: Severity::Error,
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
fn detect_cycles(changes: &[Entry]) -> Vec<Finding> {
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
            out.push(Finding {
                level: Severity::Error,
                code: "dependency-cycle",
                message: format!("cycle: {}", path.join(" → ")),
                entry: None,
            });
        } else if scc.len() == 1 {
            let node = scc[0];
            if graph.find_edge(node, node).is_some() {
                let name = graph[node];
                out.push(Finding {
                    level: Severity::Error,
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
fn check_unknown_depends_on(changes: &[Entry]) -> Vec<Finding> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.depends_on {
            if !known.contains(target.as_str()) {
                out.push(Finding {
                    level: Severity::Error,
                    code: "unknown-depends-on",
                    message: format!("depends-on references unknown change '{target}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Emit one `unknown-source` error per source key not declared at the
/// plan level.
fn check_unknown_sources(plan: &Plan) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in &plan.changes {
        for key in &entry.sources {
            if !plan.sources.contains_key(key) {
                out.push(Finding {
                    level: Severity::Error,
                    code: "unknown-source",
                    message: format!("sources references unknown source key '{key}'"),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// When more than one entry is `in-progress`, emit one result per
/// offending entry so every offender is surfaceable in the UI.
fn check_single_in_progress(changes: &[Entry]) -> Vec<Finding> {
    let offenders: Vec<&Entry> =
        changes.iter().filter(|c| c.status == Status::InProgress).collect();
    if offenders.len() <= 1 {
        return Vec::new();
    }
    offenders
        .into_iter()
        .map(|c| Finding {
            level: Severity::Error,
            code: "multiple-in-progress",
            message: "multiple in-progress entries: at most one allowed per plan".to_string(),
            entry: Some(c.name.clone()),
        })
        .collect()
}

/// RFC-3b: Every non-None `project` on a change must match a `projects[].name` in the registry.
fn check_project_in_registry(
    changes: &[Entry], registry: &Registry,
) -> Vec<Finding> {
    let project_names: HashSet<&str> = registry.projects.iter().map(|p| p.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        if let Some(project) = &entry.project
            && !project_names.contains(project.as_str())
        {
            out.push(Finding {
                level: Severity::Error,
                code: "project-not-in-registry",
                message: format!(
                    "project '{}' on change '{}' does not match any project in registry.yaml",
                    project, entry.name
                ),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

/// RFC-3b: When registry has >1 project, every change must have a `project` field.
fn check_project_required_multi_repo(
    changes: &[Entry], registry: &Registry,
) -> Vec<Finding> {
    if registry.projects.len() <= 1 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in changes {
        if entry.project.is_none() {
            out.push(Finding {
                level: Severity::Error,
                code: "project-missing-multi-repo",
                message: format!(
                    "change '{}' has no project; every change must specify a project when the registry declares more than one project",
                    entry.name
                ),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

/// RFC-8: every plan entry must have at least one of `project` or `schema`.
fn missing_project_or_schema(changes: &[Entry]) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in changes {
        if entry.project.is_none() && entry.schema.is_none() {
            out.push(Finding {
                level: Severity::Error,
                code: "plan.entry-needs-project-or-schema",
                message: format!(
                    "entry '{}' has neither 'project' nor 'schema'; at least one is required",
                    entry.name
                ),
                entry: Some(entry.name.clone()),
            });
        }
    }
    out
}

fn check_context_paths(changes: &[Entry]) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in changes {
        for path in &entry.context {
            if path.starts_with('/') || path.contains("..") {
                out.push(Finding {
                    level: Severity::Error,
                    code: "plan.context-path-invalid",
                    message: format!(
                        "entry '{}': context path '{}' must be relative to .specify/ (no '..' or absolute paths)",
                        entry.name, path
                    ),
                    entry: Some(entry.name.clone()),
                });
            }
        }
    }
    out
}

/// Plan-to-change directory consistency:
///   - Warn on orphan subdirectories (no matching plan entry).
///   - Warn when an `in-progress` plan entry has no matching directory.
fn changes_dir_consistency(plan: &Plan, changes_dir: &Path) -> Vec<Finding> {
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
            out.push(Finding {
                level: Severity::Warning,
                code: "orphan-change-dir",
                message: format!("change directory '{name}' has no plan entry"),
                entry: Some(name.clone()),
            });
        }
    }

    for entry in &plan.changes {
        if entry.status == Status::InProgress {
            let candidate = changes_dir.join(&entry.name);
            if !candidate.is_dir() {
                out.push(Finding {
                    level: Severity::Warning,
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
    use std::collections::HashSet;

    use specify_schema::RegistryProject;
    use tempfile::tempdir;

    use super::*;

    /// The 10 legal edges from `rfc-2-execution.md` §"Transition Rules".
    /// Kept here (not on `Status`) so the production matcher and the
    /// test oracle are independent representations of the same table.
    fn allowed_edges() -> HashSet<(Status, Status)> {
        use Status::*;
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
    fn legal_edges_succeed() {
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
        for &t in &Status::ALL {
            assert!(!Status::Done.can_transition_to(&t), "Done must not allow -> {t:?}");
        }
    }

    #[test]
    fn illegal_edges_rejected() {
        use Status::*;
        let cases: &[(Status, Status)] = &[
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
    fn table_matches_oracle() {
        let allowed = allowed_edges();
        for &from in &Status::ALL {
            for &to in &Status::ALL {
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
    fn error_carries_endpoints() {
        let err = Status::Done
            .transition(Status::Pending)
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
    /// fixture, updated to remove `affects` fields.
    const RFC_EXAMPLE_YAML: &str = r"name: platform-v2
sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git
changes:
  - name: user-registration
    project: platform
    sources: [monolith]
    status: done
  - name: email-verification
    project: platform
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress
  - name: registration-duplicate-email-crash
    project: platform
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending
  - name: notification-preferences
    project: platform
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending
  - name: extract-shared-validation
    project: platform
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
    depends-on: [email-verification]
    status: pending
  - name: product-catalog
    project: platform
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending
  - name: shopping-cart
    project: platform
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending
  - name: checkout-api
    project: platform
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
  - name: checkout-ui
    project: platform
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
";

    #[test]
    fn rfc_example_round_trips() {
        let original: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let rendered = serde_saphyr::to_string(&original).expect("serialize plan");
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse rendered plan");
        assert_eq!(original, reparsed, "plan should survive a serialize/parse round-trip");

        assert_eq!(original.name, "platform-v2");
        assert_eq!(original.sources.len(), 4);
        assert_eq!(original.changes.len(), 9);
        assert_eq!(original.changes[0].status, Status::Done);
        assert_eq!(original.changes[1].status, Status::InProgress);
        assert_eq!(original.changes[7].status, Status::Failed);
        assert!(original.changes[7].status_reason.is_some());
    }

    #[test]
    fn serializes_kebab_case() {
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "entry-one".to_string(),
                project: Some("default".into()),
                schema: None,
                status: Status::InProgress,
                depends_on: vec!["entry-zero".to_string()],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: Some("awaiting upstream fix".to_string()),
            }],
        };
        let yaml = serde_saphyr::to_string(&plan).expect("serialize plan");
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
    fn missing_fields_default() {
        let yaml = "name: foo\nchanges: []\n";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse minimal plan");
        assert_eq!(plan.name, "foo");
        assert!(plan.sources.is_empty(), "sources should default to empty map");
        assert!(plan.changes.is_empty(), "changes should be empty");
    }

    #[test]
    fn status_reason_round_trips() {
        let yaml = r"name: demo
changes:
  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse");
        let entry = &plan.changes[0];
        assert_eq!(entry.status, Status::Failed);
        let reason = entry.status_reason.as_deref().expect("status_reason populated");
        assert!(
            reason.contains("Type mismatch"),
            "status_reason should preserve folded text, got: {reason:?}"
        );

        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed);
        assert_eq!(
            reparsed.changes[0].status_reason, entry.status_reason,
            "status_reason should be byte-identical after round-trip"
        );
    }

    #[test]
    fn save_load_round_trips() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let original: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        original.save(&path).expect("save ok");
        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, original, "full plan should round-trip through save -> load");
    }

    #[test]
    fn save_emits_trailing_newline() {
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
    fn save_overwrites_atomically() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "garbage that should be overwritten").expect("write garbage");

        let plan = Plan {
            name: "fresh".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "only-entry".to_string(),
                project: Some("default".into()),
                schema: None,
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
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
    fn load_missing_returns_not_found() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.yaml");
        let err = Plan::load(&path).expect_err("expected error on missing file");
        match err {
            Error::ArtifactNotFound { kind, path: p } => {
                assert_eq!(kind, "plan.yaml");
                assert_eq!(p, path);
            }
            other => panic!("expected Error::ArtifactNotFound, got {other:?}"),
        }
    }

    #[test]
    fn load_no_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "name: foo\nchanges: []").expect("write without trailing newline");
        let plan = Plan::load(&path).expect("load ok");
        assert_eq!(plan.name, "foo");
        assert!(plan.changes.is_empty());
    }

    #[test]
    fn save_writes_kebab_case() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "entry-one".to_string(),
                project: Some("default".into()),
                schema: None,
                status: Status::InProgress,
                depends_on: vec!["foo".to_string()],
                sources: vec![],
                context: vec![],
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

    fn plan_with_changes(changes: Vec<Entry>) -> Plan {
        Plan {
            name: "test".into(),
            sources: BTreeMap::new(),
            changes,
        }
    }

    fn change(name: &str, status: Status) -> Entry {
        Entry {
            name: name.into(),
            project: Some("default".into()),
            schema: None,
            status,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }
    }

    #[test]
    fn clean_plan_validates() {
        let plan: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let results = plan.validate(None, None);
        assert!(
            results.is_empty(),
            "expected a clean RFC fixture to validate with no findings, got: {results:#?}"
        );
    }

    #[test]
    fn duplicate_name_error() {
        let plan = plan_with_changes(vec![
            change("foo", Status::Done),
            change("foo", Status::Pending),
        ]);
        let results = plan.validate(None, None);
        let dupes: Vec<_> = results.iter().filter(|r| r.code == "duplicate-name").collect();
        assert_eq!(dupes.len(), 1, "expected one duplicate-name result, got {results:#?}");
        assert_eq!(dupes[0].level, Severity::Error);
        assert_eq!(dupes[0].entry.as_deref(), Some("foo"));
    }

    #[test]
    fn cycle_error() {
        let mut a = change("a", Status::Pending);
        a.depends_on = vec!["c".into()];
        let mut b = change("b", Status::Pending);
        b.depends_on = vec!["a".into()];
        let mut c = change("c", Status::Pending);
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
    fn self_cycle_error() {
        let mut a = change("a", Status::Pending);
        a.depends_on = vec!["a".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None, None);
        assert!(
            results.iter().any(|r| r.code == "dependency-cycle"),
            "expected a dependency-cycle result for self-edge, got: {results:#?}"
        );
    }

    #[test]
    fn unknown_depends_on_error() {
        let mut a = change("a", Status::Pending);
        a.depends_on = vec!["bogus".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None, None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-depends-on").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-depends-on, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("bogus"));
    }

    #[test]
    fn unknown_source_error() {
        let mut a = change("a", Status::Pending);
        a.sources = vec!["monolith".into()];
        let plan = plan_with_changes(vec![a]);
        let results = plan.validate(None, None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-source").collect();
        assert_eq!(hits.len(), 1, "expected one unknown-source, got {results:#?}");
        assert_eq!(hits[0].entry.as_deref(), Some("a"));
        assert!(hits[0].message.contains("monolith"));
    }

    #[test]
    fn multiple_in_progress_error() {
        let plan = plan_with_changes(vec![
            change("a", Status::InProgress),
            change("b", Status::InProgress),
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
            change("a", Status::InProgress),
            change("b", Status::Pending),
        ]);
        let results = plan.validate(None, None);
        assert!(
            !results.iter().any(|r| r.code == "multiple-in-progress"),
            "single in-progress entry should not trip multiple-in-progress: {results:#?}"
        );
    }

    #[test]
    fn orphan_dir_warning() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("stale-change")).expect("mkdir");
        let plan = plan_with_changes(vec![change("other", Status::Pending)]);
        let results = plan.validate(Some(tmp.path()), None);
        let hits: Vec<_> = results.iter().filter(|r| r.code == "orphan-change-dir").collect();
        assert_eq!(hits.len(), 1, "expected one orphan-change-dir, got {results:#?}");
        assert_eq!(hits[0].level, Severity::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("stale-change"));
    }

    #[test]
    fn missing_dir_for_in_progress_warning() {
        let tmp = tempdir().expect("tempdir");
        let plan = plan_with_changes(vec![change("alpha", Status::InProgress)]);
        let results = plan.validate(Some(tmp.path()), None);
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "missing-change-dir-for-in-progress").collect();
        assert_eq!(hits.len(), 1, "expected one missing-dir warning, got {results:#?}");
        assert_eq!(hits[0].level, Severity::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("alpha"));
    }

    #[test]
    fn present_dir_for_in_progress_silent() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("alpha")).expect("mkdir alpha");
        let plan = plan_with_changes(vec![change("alpha", Status::InProgress)]);
        let results = plan.validate(Some(tmp.path()), None);
        assert!(
            !results.iter().any(|r| r.code.ends_with("-change-dir")
                || r.code == "orphan-change-dir"
                || r.code == "missing-change-dir-for-in-progress"),
            "no directory warnings expected, got: {results:#?}"
        );
    }

    #[test]
    fn no_changes_dir_skips_consistency() {
        let plan = plan_with_changes(vec![change("alpha", Status::InProgress)]);
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
    fn no_short_circuit() {
        // One plan, three distinct violations:
        //   - duplicate name `foo`
        //   - unknown depends-on target
        //   - unknown source key
        let mut a = change("foo", Status::Pending);
        a.depends_on = vec!["missing".into()];
        a.sources = vec!["ghost-source".into()];
        let b = change("foo", Status::Pending);
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

    /// Convenience: build a `Entry` with an explicit `depends_on`.
    fn change_with_deps(name: &str, status: Status, deps: &[&str]) -> Entry {
        Entry {
            name: name.into(),
            project: Some("default".into()),
            schema: None,
            status,
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }
    }

    #[test]
    fn next_eligible_picks_first_ready() {
        let plan = plan_with_changes(vec![
            change("a", Status::Done),
            change("b", Status::Done),
            change_with_deps("c", Status::Pending, &["b"]),
        ]);
        let eligible = plan.next_eligible().expect("c should be eligible");
        assert_eq!(eligible.name, "c");
    }

    #[test]
    fn next_eligible_skips_unmet_deps() {
        let plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let eligible = plan.next_eligible().expect("a should be eligible");
        assert_eq!(eligible.name, "a", "b's dep 'a' is not done, so a (no deps) wins");
    }

    #[test]
    fn next_eligible_blocked_by_in_progress() {
        let plan = plan_with_changes(vec![
            change("a", Status::InProgress),
            change("b", Status::Pending),
        ]);
        assert!(
            plan.next_eligible().is_none(),
            "an in-progress entry must block any new selection"
        );
    }

    #[test]
    fn next_eligible_none_when_finished() {
        let plan = plan_with_changes(vec![
            change("a", Status::Done),
            change("b", Status::Skipped),
            change("c", Status::Failed),
        ]);
        assert!(plan.next_eligible().is_none());
    }

    #[test]
    fn next_eligible_tiebreak() {
        let plan = plan_with_changes(vec![
            change("alpha", Status::Pending),
            change("beta", Status::Pending),
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
    fn next_eligible_rfc_forward() {
        let mut plan: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        for entry in &mut plan.changes {
            entry.status = Status::Pending;
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
            entry.status = Status::Done;
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
    fn next_eligible_blocks_mid_cycle() {
        let plan = plan_with_changes(vec![
            change("in-flight", Status::InProgress),
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        assert!(
            plan.next_eligible().is_none(),
            "in-progress entry must block selection before any dependency walk"
        );
    }

    #[test]
    fn topo_order_rfc_example() {
        let plan: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
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
    fn topo_order_cycle_errors() {
        let plan = plan_with_changes(vec![
            change_with_deps("a", Status::Pending, &["c"]),
            change_with_deps("b", Status::Pending, &["a"]),
            change_with_deps("c", Status::Pending, &["b"]),
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
    fn topo_order_deterministic_tiebreak() {
        let alpha_first = plan_with_changes(vec![
            change("alpha", Status::Pending),
            change("beta", Status::Pending),
        ]);
        let order: Vec<&str> = alpha_first
            .topological_order()
            .expect("no cycle")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(order, ["alpha", "beta"]);

        let beta_first = plan_with_changes(vec![
            change("beta", Status::Pending),
            change("alpha", Status::Pending),
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
    fn next_eligible_with_cycle() {
        let plan = plan_with_changes(vec![
            change("busy", Status::InProgress),
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        assert!(plan.next_eligible().is_none());
        assert!(plan.topological_order().is_err(), "cycle should surface from topological_order");
    }

    #[test]
    fn save_no_intermediate_state() {
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
            changes: vec![Entry {
                name: "new-entry".to_string(),
                project: Some("default".into()),
                schema: None,
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
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
    fn create_forces_pending_clears_reason() {
        let mut plan = plan_with_changes(vec![]);
        let incoming = Entry {
            name: "foo".into(),
            project: Some("default".into()),
            schema: None,
            status: Status::Failed,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: Some("bogus".into()),
        };
        plan.create(incoming).expect("create ok");
        assert_eq!(plan.changes.len(), 1);
        assert_eq!(plan.changes[0].name, "foo");
        assert_eq!(
            plan.changes[0].status,
            Status::Pending,
            "create must force status to Pending regardless of input"
        );
        assert_eq!(
            plan.changes[0].status_reason, None,
            "create must clear status_reason regardless of input"
        );
    }

    #[test]
    fn create_rejects_duplicate() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let dup = change("foo", Status::Pending);
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
    fn create_rejects_bad_name() {
        let mut plan = plan_with_changes(vec![]);
        let bad = change("Bad-Name", Status::Pending);
        let err = plan.create(bad).expect_err("invalid name must be rejected");
        match err {
            Error::InvalidName(msg) => {
                assert!(msg.contains("kebab-case"), "expected kebab-case in message, got: {msg}");
            }
            other => panic!("expected Error::InvalidName, got {other:?}"),
        }
        assert!(plan.changes.is_empty(), "plan must remain untouched after invalid name");
    }

    #[test]
    fn create_rejects_unknown_depends_on() {
        // We cannot introduce a cycle via a *new* entry alone (a new
        // entry has no backreferences), but the rollback path is
        // shared. Exercise it with an unknown-depends-on Error.
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let c = change_with_deps("c", Status::Pending, &["does-not-exist"]);
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
    fn create_rolls_back_on_failure() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let bar = change_with_deps("bar", Status::Pending, &["nonexistent"]);
        let err = plan.create(bar).expect_err("must Err");
        assert!(matches!(err, Error::Config(_)));
        assert_eq!(plan.changes.len(), 1, "plan length unchanged after rollback");
        assert_eq!(plan.changes[0].name, "foo");
        assert_eq!(plan.changes[0].status, Status::Pending);
        assert!(plan.changes[0].depends_on.is_empty());
    }

    #[test]
    fn amend_deps() {
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let patch = EntryPatch {
            depends_on: Some(vec![]),
            ..EntryPatch::default()
        };
        plan.amend("b", patch).expect("amend ok");
        let b = plan.changes.iter().find(|c| c.name == "b").unwrap();
        assert!(b.depends_on.is_empty(), "depends_on should be replaced with empty vec");
    }

    #[test]
    fn amend_description_three_way() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "foo".into(),
            project: Some("default".into()),
            schema: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: Some("original".into()),
            status_reason: None,
        }]);

        plan.amend("foo", EntryPatch::default()).expect("amend none ok");
        assert_eq!(
            plan.changes[0].description.as_deref(),
            Some("original"),
            "None description must leave description unchanged"
        );

        plan.amend(
            "foo",
            EntryPatch {
                description: Some(None),
                ..EntryPatch::default()
            },
        )
        .expect("amend clear ok");
        assert_eq!(
            plan.changes[0].description, None,
            "Some(None) description must clear description"
        );

        plan.amend(
            "foo",
            EntryPatch {
                description: Some(Some("new".into())),
                ..EntryPatch::default()
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
    fn amend_leaves_unchanged() {
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
                Entry {
                    name: "foo".into(),
                    project: Some("default".into()),
                    schema: None,
                    status: Status::Pending,
                    depends_on: vec![],
                    sources: vec!["a".into()],
                    context: vec![],
                    description: Some("d".into()),
                    status_reason: None,
                },
                change("b", Status::Pending),
                change("x", Status::Pending),
            ],
        };
        let mut plan = plan;
        let patch = EntryPatch {
            depends_on: Some(vec!["x".into()]),
            ..EntryPatch::default()
        };
        plan.amend("foo", patch).expect("amend ok");
        let foo = plan.changes.iter().find(|c| c.name == "foo").unwrap();
        assert_eq!(foo.depends_on, vec!["x".to_string()]);
        assert_eq!(foo.sources, vec!["a".to_string()], "sources untouched");
        assert_eq!(foo.description.as_deref(), Some("d"), "description untouched");
    }

    #[test]
    fn amend_missing_entry() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let err = plan
            .amend("nonexistent", EntryPatch::default())
            .expect_err("missing entry must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("nonexistent"), "message should mention name, got: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn amend_rejects_cycle() {
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change("b", Status::Pending),
        ]);

        plan.amend(
            "a",
            EntryPatch {
                depends_on: Some(vec!["b".into()]),
                ..EntryPatch::default()
            },
        )
        .expect("a -> [b] is acyclic; amend ok");

        let err = plan
            .amend(
                "b",
                EntryPatch {
                    depends_on: Some(vec!["a".into()]),
                    ..EntryPatch::default()
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
    fn patch_omits_status() {
        // Compile-time invariant: `EntryPatch` has no `status`
        // field, so `amend` literally cannot mutate lifecycle state.
        // The following line is commented out because it will not
        // compile — the field does not exist on the struct. Deleting
        // the comment and uncommenting the line is the only way to
        // violate the single-writer-for-status invariant, and it
        // fails to build, which is the entire point.
        //
        // let _ = EntryPatch { status: Status::Pending, ..Default::default() };
        //
        // Runtime assertion below is a smoke test that the type is
        // `Default`-constructible and that the three fields we do have
        // default to `None`.
        let patch = EntryPatch::default();
        assert!(patch.depends_on.is_none());
        assert!(patch.sources.is_none());
        assert!(patch.project.is_none());
        assert!(patch.schema.is_none());
        assert!(patch.description.is_none());
    }

    #[test]
    fn transition_clears_reason_on_reentry() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "a".into(),
            project: Some("default".into()),
            schema: None,
            status: Status::Failed,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: Some("crashed".into()),
        }]);
        plan.transition("a", Status::Pending, None).expect("failed -> pending ok");
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Pending);
        assert_eq!(a.status_reason, None, "re-entry to Pending must clear status_reason");
    }

    #[test]
    fn transition_writes_reason() {
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change("b", Status::InProgress),
            change("c", Status::Failed),
        ]);

        plan.transition("a", Status::Blocked, Some("needs scope"))
            .expect("pending -> blocked ok");
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Blocked);
        assert_eq!(a.status_reason.as_deref(), Some("needs scope"));

        plan.transition("b", Status::Failed, Some("broken")).expect("in-progress -> failed ok");
        let b = plan.changes.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.status, Status::Failed);
        assert_eq!(b.status_reason.as_deref(), Some("broken"));

        plan.transition("c", Status::Skipped, Some("abandoned")).expect("failed -> skipped ok");
        let c = plan.changes.iter().find(|c| c.name == "c").unwrap();
        assert_eq!(c.status, Status::Skipped);
        assert_eq!(c.status_reason.as_deref(), Some("abandoned"));
    }

    #[test]
    fn transition_rejects_reason_on_clean_target() {
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change("b", Status::InProgress),
        ]);

        let err = plan
            .transition("a", Status::InProgress, Some("why"))
            .expect_err("reason on InProgress target must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("--reason"), "message should mention --reason: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Pending, "a.status must be unchanged");

        let err = plan
            .transition("b", Status::Done, Some("why"))
            .expect_err("reason on Done target must Err");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("--reason"), "message should mention --reason: {msg}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        let b = plan.changes.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.status, Status::InProgress, "b.status must be unchanged");
    }

    #[test]
    fn transition_rejects_illegal_edge_via_state_machine() {
        let mut plan = plan_with_changes(vec![change("a", Status::Done)]);
        let err = plan
            .transition("a", Status::Pending, None)
            .expect_err("Done -> Pending must Err from state machine");
        match err {
            Error::PlanTransition { from, to } => {
                assert_eq!(from, "Done");
                assert_eq!(to, "Pending");
            }
            other => panic!("expected Error::PlanTransition, got {other:?}"),
        }
        let a = plan.changes.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Done, "status must not be mutated on illegal edge");
    }

    #[test]
    fn transition_rejects_missing_entry() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let err = plan
            .transition("nonexistent", Status::InProgress, None)
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
    fn write_plan(dir: &Path, name: &str, changes: Vec<Entry>) -> PathBuf {
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
                change("a", Status::Done),
                change("b", Status::Skipped),
                change("c", Status::Done),
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
        let plan_path = write_plan(tmp.path(), "proj", vec![change("a", Status::Done)]);

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
                change("done-one", Status::Done),
                change("still-pending", Status::Pending),
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
                change("a", Status::Done),
                change("b", Status::InProgress),
                change("c", Status::Blocked),
                change("d", Status::Failed),
                change("e", Status::Skipped),
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
                change("a", Status::Done),
                change("b", Status::InProgress),
                change("c", Status::Blocked),
                change("d", Status::Failed),
                change("e", Status::Skipped),
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

        let archived: Plan = serde_saphyr::from_slice(&post_bytes).expect("parse archived");
        let statuses: Vec<Status> = archived.changes.iter().map(|c| c.status).collect();
        assert_eq!(
            statuses,
            vec![
                Status::Done,
                Status::InProgress,
                Status::Blocked,
                Status::Failed,
                Status::Skipped,
            ],
            "statuses in archive must not be rewritten"
        );
    }

    #[test]
    fn archive_filename_is_kebab_plan_name_plus_yyyymmdd() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path =
            write_plan(tmp.path(), "my-initiative", vec![change("a", Status::Done)]);

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

        let plan_path = write_plan(tmp.path(), "dup", vec![change("a", Status::Done)]);

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
        let plan_path = write_plan(tmp.path(), "pkg", vec![change("a", Status::Done)]);

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
            Error::InvalidName(msg) => {
                assert!(msg.contains("kebab-case"), "expected kebab-case in message, got: {msg}");
            }
            other => panic!("expected Error::InvalidName, got {other:?}"),
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
        dir: &Path, name: &str, changes: Vec<Entry>, files: &[(&str, &[u8])],
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
            vec![change("a", Status::Done)],
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
        let plan_path = write_plan(&specify, "solo", vec![change("a", Status::Done)]);

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
        let plan_path = write_plan(&specify, "solo", vec![change("a", Status::Done)]);
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
            vec![change("a", Status::Done)],
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
            vec![change("a", Status::Done)],
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
            vec![change("a", Status::Done)],
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
            vec![change("a", Status::Done)],
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
                change("done-one", Status::Done),
                change("still-pending", Status::Pending),
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
        let plan_path = write_plan(tmp.path(), "atomic", vec![change("a", Status::Done)]);
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

    // ---------- RFC-3b: Entry.project ----------

    #[test]
    fn plan_change_project_round_trips_with_value() {
        let yaml = "\
name: foo
project: traffic
status: pending
";
        let parsed: Entry = serde_saphyr::from_str(yaml).expect("parses with project");
        assert_eq!(parsed.project.as_deref(), Some("traffic"));
        let round_tripped = serde_saphyr::to_string(&parsed).expect("serialize");
        let re_parsed: Entry = serde_saphyr::from_str(&round_tripped).expect("re-parse");
        assert_eq!(re_parsed.project, parsed.project);
    }

    #[test]
    fn plan_change_project_defaults_to_none() {
        let yaml = "\
name: foo
status: pending
";
        let parsed: Entry = serde_saphyr::from_str(yaml).expect("parses without project");
        assert_eq!(parsed.project, None);
    }

    #[test]
    fn amend_project_three_way_semantics() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "foo".into(),
            project: Some("alpha".into()),
            schema: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }]);

        // None leaves project unchanged.
        plan.amend("foo", EntryPatch::default()).expect("amend none ok");
        assert_eq!(
            plan.changes[0].project.as_deref(),
            Some("alpha"),
            "None must leave project unchanged"
        );

        // Some(Some(s)) replaces project.
        plan.amend(
            "foo",
            EntryPatch {
                project: Some(Some("beta".into())),
                ..EntryPatch::default()
            },
        )
        .expect("amend replace ok");
        assert_eq!(
            plan.changes[0].project.as_deref(),
            Some("beta"),
            "Some(Some(s)) must replace project"
        );

        // Some(None) clears project — set schema so the entry still has
        // at least one of project/schema after the clear.
        plan.amend(
            "foo",
            EntryPatch {
                project: Some(None),
                schema: Some(Some("contracts@v1".into())),
                ..EntryPatch::default()
            },
        )
        .expect("amend clear ok");
        assert_eq!(plan.changes[0].project, None, "Some(None) must clear project");
    }

    #[test]
    fn project_not_in_registry_error() {
        let plan = Plan {
            name: "test".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "a".to_string(),
                project: Some("nonexistent".to_string()),
                schema: None,
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        let registry = Registry {
            version: 1,
            projects: vec![RegistryProject {
                name: "real-project".to_string(),
                url: ".".to_string(),
                schema: "omnia@v1".to_string(),
                description: None,
                contracts: None,
            }],
        };
        let results = plan.validate(None, Some(&registry));
        assert!(results.iter().any(|r| r.code == "project-not-in-registry"));
    }

    #[test]
    fn project_missing_multi_repo_error() {
        let plan = Plan {
            name: "test".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "a".to_string(),
                project: None,
                schema: Some("contracts@v1".into()),
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        let registry = Registry {
            version: 1,
            projects: vec![
                RegistryProject {
                    name: "alpha".to_string(),
                    url: ".".to_string(),
                    schema: "omnia@v1".to_string(),
                    description: Some("Alpha project".to_string()),
                    contracts: None,
                },
                RegistryProject {
                    name: "beta".to_string(),
                    url: "git@github.com:org/beta.git".to_string(),
                    schema: "omnia@v1".to_string(),
                    description: Some("Beta project".to_string()),
                    contracts: None,
                },
            ],
        };
        let results = plan.validate(None, Some(&registry));
        assert!(results.iter().any(|r| r.code == "project-missing-multi-repo"));
    }

    #[test]
    fn project_valid_in_single_repo_no_error() {
        let plan = Plan {
            name: "test".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "a".to_string(),
                project: None,
                schema: Some("contracts@v1".into()),
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        let registry = Registry {
            version: 1,
            projects: vec![RegistryProject {
                name: "solo".to_string(),
                url: ".".to_string(),
                schema: "omnia@v1".to_string(),
                description: None,
                contracts: None,
            }],
        };
        let results = plan.validate(None, Some(&registry));
        assert!(!results.iter().any(|r| r.code == "project-missing-multi-repo"));
        assert!(!results.iter().any(|r| r.code == "project-not-in-registry"));
    }

    #[test]
    fn project_matches_registry_no_error() {
        let plan = Plan {
            name: "test".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "a".to_string(),
                project: Some("alpha".to_string()),
                schema: None,
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        let registry = Registry {
            version: 1,
            projects: vec![
                RegistryProject {
                    name: "alpha".to_string(),
                    url: ".".to_string(),
                    schema: "omnia@v1".to_string(),
                    description: Some("Alpha".to_string()),
                    contracts: None,
                },
                RegistryProject {
                    name: "beta".to_string(),
                    url: "git@github.com:org/beta.git".to_string(),
                    schema: "omnia@v1".to_string(),
                    description: Some("Beta".to_string()),
                    contracts: None,
                },
            ],
        };
        let results = plan.validate(None, Some(&registry));
        assert!(!results.iter().any(|r| r.level == Severity::Error));
    }

    // --- RFC-8: schema field -------------------------------------------------

    #[test]
    fn schema_field_roundtrips_yaml() {
        let yaml = r"name: test
changes:
  - name: define-contracts
    schema: contracts@v1
    status: pending
  - name: impl-auth
    project: auth-service
    schema: omnia@v1
    status: pending
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(plan.changes[0].schema.as_deref(), Some("contracts@v1"));
        assert_eq!(plan.changes[0].project, None);
        assert_eq!(plan.changes[1].schema.as_deref(), Some("omnia@v1"));
        assert_eq!(plan.changes[1].project.as_deref(), Some("auth-service"));

        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed, "plan must survive a YAML round-trip");
    }

    #[test]
    fn validation_error_when_neither_project_nor_schema() {
        let plan = Plan {
            name: "test".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "orphan".to_string(),
                project: None,
                schema: None,
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        let results = plan.validate(None, None);
        assert!(
            results.iter().any(|r| r.code == "plan.entry-needs-project-or-schema"
                && r.level == Severity::Error),
            "expected entry-needs-project-or-schema error, got: {results:#?}"
        );
    }

    #[test]
    fn validation_passes_with_schema_only() {
        let plan = Plan {
            name: "test".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "contracts".to_string(),
                project: None,
                schema: Some("contracts@v1".into()),
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        let results = plan.validate(None, None);
        assert!(
            !results.iter().any(|r| r.code == "plan.entry-needs-project-or-schema"),
            "schema-only entry must not trigger project-or-schema error"
        );
    }

    #[test]
    fn validation_passes_with_both_project_and_schema() {
        let plan = Plan {
            name: "test".to_string(),
            sources: BTreeMap::new(),
            changes: vec![Entry {
                name: "impl".to_string(),
                project: Some("auth-service".into()),
                schema: Some("omnia@v1".into()),
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        let results = plan.validate(None, None);
        assert!(
            !results.iter().any(|r| r.code == "plan.entry-needs-project-or-schema"),
            "entry with both project and schema must pass"
        );
    }

    #[test]
    fn create_rejects_entry_without_project_or_schema() {
        let mut plan = plan_with_changes(vec![]);
        let entry = Entry {
            name: "bad".into(),
            project: None,
            schema: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        };
        let err = plan.create(entry).expect_err("must reject entry without project or schema");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("project") && msg.contains("schema"),
                    "error should mention project and schema: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        assert!(plan.changes.is_empty(), "plan must remain empty after rejected create");
    }

    #[test]
    fn amend_schema_three_way_semantics() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "foo".into(),
            project: Some("default".into()),
            schema: Some("omnia@v1".into()),
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }]);

        // None leaves schema unchanged.
        plan.amend("foo", EntryPatch::default()).expect("amend none ok");
        assert_eq!(
            plan.changes[0].schema.as_deref(),
            Some("omnia@v1"),
            "None must leave schema unchanged"
        );

        // Some(Some(s)) replaces schema.
        plan.amend(
            "foo",
            EntryPatch {
                schema: Some(Some("contracts@v1".into())),
                ..EntryPatch::default()
            },
        )
        .expect("amend replace ok");
        assert_eq!(
            plan.changes[0].schema.as_deref(),
            Some("contracts@v1"),
            "Some(Some(s)) must replace schema"
        );

        // Some(None) clears schema (project is still set, so validation passes).
        plan.amend(
            "foo",
            EntryPatch {
                schema: Some(None),
                ..EntryPatch::default()
            },
        )
        .expect("amend clear ok");
        assert_eq!(plan.changes[0].schema, None, "Some(None) must clear schema");
    }

    #[test]
    fn context_round_trip_through_yaml() {
        let yaml = r"
name: ctx-test
changes:
  - name: with-ctx
    project: default
    status: pending
    context:
      - contracts/http/user-api.yaml
      - specs/user-registration/spec.md
  - name: without-ctx
    project: default
    status: pending
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse yaml");
        assert_eq!(
            plan.changes[0].context,
            vec!["contracts/http/user-api.yaml", "specs/user-registration/spec.md"],
        );
        assert!(plan.changes[1].context.is_empty(), "missing context defaults to empty");

        let serialized = serde_saphyr::to_string(&plan).expect("serialize");
        assert!(
            serialized.contains("contracts/http/user-api.yaml"),
            "populated context must appear in serialized output"
        );
        assert!(
            !serialized.contains("without-ctx")
                || !serialized.split("without-ctx").nth(1).unwrap_or("").contains("context"),
            "empty context must be omitted from serialized output"
        );
    }

    #[test]
    fn validate_rejects_context_path_with_dotdot() {
        let mut entry = change("foo", Status::Pending);
        entry.context = vec!["../etc/passwd".into()];
        let plan = plan_with_changes(vec![entry]);
        let errors: Vec<_> = plan
            .validate(None, None)
            .into_iter()
            .filter(|r| r.code == "plan.context-path-invalid")
            .collect();
        assert_eq!(errors.len(), 1, "expected exactly one context-path-invalid error");
        assert!(errors[0].message.contains(".."), "message should mention '..'");
    }

    #[test]
    fn validate_rejects_absolute_context_path() {
        let mut entry = change("foo", Status::Pending);
        entry.context = vec!["/absolute/path".into()];
        let plan = plan_with_changes(vec![entry]);
        let errors: Vec<_> = plan
            .validate(None, None)
            .into_iter()
            .filter(|r| r.code == "plan.context-path-invalid")
            .collect();
        assert_eq!(errors.len(), 1, "expected exactly one context-path-invalid error");
        assert!(errors[0].message.contains("/absolute/path"));
    }

    #[test]
    fn validate_accepts_valid_context_paths() {
        let mut entry = change("foo", Status::Pending);
        entry.context =
            vec!["contracts/http/user-api.yaml".into(), "specs/user-registration/spec.md".into()];
        let plan = plan_with_changes(vec![entry]);
        assert!(
            !plan.validate(None, None).into_iter().any(|r| r.code == "plan.context-path-invalid"),
            "valid relative paths must not produce errors"
        );
    }

    #[test]
    fn amend_replaces_context() {
        let mut entry = change("foo", Status::Pending);
        entry.context = vec!["old/path.yaml".into()];
        let mut plan = plan_with_changes(vec![entry]);

        plan.amend(
            "foo",
            EntryPatch {
                context: Some(vec!["new/path.yaml".into(), "another.md".into()]),
                ..EntryPatch::default()
            },
        )
        .expect("amend ok");
        assert_eq!(
            plan.changes[0].context,
            vec!["new/path.yaml", "another.md"],
            "amend must replace context wholesale"
        );
    }

    #[test]
    fn amend_none_context_leaves_unchanged() {
        let mut entry = change("foo", Status::Pending);
        entry.context = vec!["keep/this.yaml".into()];
        let mut plan = plan_with_changes(vec![entry]);

        plan.amend("foo", EntryPatch::default()).expect("amend ok");
        assert_eq!(
            plan.changes[0].context,
            vec!["keep/this.yaml"],
            "None context must leave field unchanged"
        );
    }

    #[test]
    fn create_stores_context() {
        let mut plan = plan_with_changes(vec![]);
        let entry = Entry {
            name: "with-ctx".into(),
            project: Some("default".into()),
            schema: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec!["contracts/http/foo.yaml".into()],
            description: None,
            status_reason: None,
        };
        plan.create(entry).expect("create ok");
        assert_eq!(
            plan.changes[0].context,
            vec!["contracts/http/foo.yaml"],
            "create must preserve context"
        );
    }

    #[test]
    fn create_rejects_entry_with_invalid_context_path() {
        let mut plan = plan_with_changes(vec![]);
        let entry = Entry {
            name: "bad-ctx".into(),
            project: Some("default".into()),
            schema: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec!["../escape".into()],
            description: None,
            status_reason: None,
        };
        let err = plan.create(entry).expect_err("invalid context path must be rejected");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("context-path-invalid") || msg.contains(".."),
                    "error should mention context path issue, got: {msg}"
                );
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
        assert!(plan.changes.is_empty(), "rollback must remove the entry");
    }

    #[test]
    fn plan_status_display_matches_serde_wire_format() {
        assert_eq!(Status::Pending.to_string(), "pending");
        assert_eq!(Status::InProgress.to_string(), "in-progress");
        assert_eq!(Status::Done.to_string(), "done");
        assert_eq!(Status::Blocked.to_string(), "blocked");
        assert_eq!(Status::Failed.to_string(), "failed");
        assert_eq!(Status::Skipped.to_string(), "skipped");
    }
}
