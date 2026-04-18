//! On-disk representation of `.specify/plan.yaml` and the in-memory
//! [`Plan`] state machine that wraps it.
//!
//! See `rfcs/rfc-2-plan.md` §"Library Implementation" for the canonical
//! type surface and `rfcs/rfc-2-plan.md` §"The Plan" for the reference
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use petgraph::algo::{tarjan_scc, toposort};
use petgraph::graph::DiGraph;
use serde::{Deserialize, Serialize};
use specify_error::Error;

/// Lifecycle state of a single entry in [`Plan::changes`].
///
/// The enum is `Copy + Eq + Hash` so it can appear in `HashSet`s,
/// `match` guards, and hash-keyed lookups without clones. This mirrors
/// the derives already used on `LifecycleStatus` in the parent module.
/// Transition-table methods (`can_transition_to`, `transition`) land in
/// Change L1.B and intentionally do not exist yet.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
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
    /// machine. See `rfc-2-plan.md` §"Transition Rules" for the canonical
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
}

/// Severity of a validation finding produced by [`Plan::validate`].
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationLevel {
    /// Blocking problem — the plan is not usable as-is.
    Error,
    /// Non-blocking advisory — the plan is usable but something looks
    /// off (e.g. a source key is defined but unreferenced).
    Warning,
}

/// A single finding reported by [`Plan::validate`].
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Severity bucket.
    pub level: ValidationLevel,
    /// Stable machine-readable code, e.g. `"plan.cycle"`.
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
    /// Name of the offending entry, when the finding is entry-local.
    pub entry: Option<String>,
}

impl Plan {
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
        let mut content = serde_yaml::to_string(self)?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(tmp.as_file_mut(), content.as_bytes())?;
        tmp.as_file_mut().sync_all()?;
        tmp.persist(path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    /// Run all structural and semantic checks over the plan. The optional
    /// `changes_dir` points at `.specify/changes/` and enables the
    /// cross-reference checks against on-disk change metadata.
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
    pub fn validate(&self, changes_dir: Option<&Path>) -> Vec<ValidationResult> {
        let mut results = Vec::new();
        results.extend(collect_duplicate_names(&self.changes));
        results.extend(detect_cycles(&self.changes));
        results.extend(check_unknown_depends_on(&self.changes));
        results.extend(check_unknown_affects(&self.changes));
        results.extend(check_unknown_sources(self));
        results.extend(check_single_in_progress(&self.changes));
        if let Some(dir) = changes_dir.filter(|d| d.is_dir()) {
            results.extend(check_changes_dir_consistency(self, dir));
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
    /// `rfc-2-plan.md` §Fields.
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
        let errors: Vec<ValidationResult> =
            self.validate(None).into_iter().filter(|r| r.level == ValidationLevel::Error).collect();
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
        }

        let errors: Vec<ValidationResult> =
            self.validate(None).into_iter().filter(|r| r.level == ValidationLevel::Error).collect();
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
    /// Implementation: the plan is small, so we sweep the list
    /// repeatedly and emit every entry whose dependencies are already
    /// in the output. This is O(n²) but trivially preserves list-order
    /// tie-breaking. We use `petgraph::toposort` first only to detect
    /// cycles and name an offending node.
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

        let known: HashSet<&str> = self.changes.iter().map(|c| c.name.as_str()).collect();
        let mut emitted: HashSet<&str> = HashSet::new();
        let mut output: Vec<&PlanChange> = Vec::with_capacity(self.changes.len());
        while output.len() < self.changes.len() {
            let before = output.len();
            for entry in &self.changes {
                if emitted.contains(entry.name.as_str()) {
                    continue;
                }
                let deps_ready = entry
                    .depends_on
                    .iter()
                    .all(|dep| !known.contains(dep.as_str()) || emitted.contains(dep.as_str()));
                if deps_ready {
                    output.push(entry);
                    emitted.insert(entry.name.as_str());
                }
            }
            if output.len() == before {
                return Err(Error::Config(
                    "plan has dependency cycle (no progress in Kahn sweep)".to_string(),
                ));
            }
        }
        Ok(output)
    }

    /// Move `.specify/plan.yaml` into the archive directory.
    ///
    /// Semantics (see `rfc-2-plan.md` §L1.G and §"`specify plan
    /// archive`"):
    ///
    /// 1. Load the plan at `path`.
    /// 2. Collect every entry whose status is non-terminal for archival
    ///    purposes — anything not in `{Done, Skipped}`. If the list is
    ///    non-empty and `force == false`, return
    ///    [`Error::PlanHasOutstandingWork`] carrying those names in
    ///    plan list order. When `force == true`, proceed; the archived
    ///    file preserves the statuses verbatim.
    /// 3. Create `archive_dir` if missing.
    /// 4. Destination: `<archive_dir>/<plan.name>-<YYYYMMDD>.yaml` using
    ///    today's UTC date. If it already exists, return an
    ///    `Error::Config` — archives are never overwritten.
    /// 5. Move the file via [`move_file_atomic`] (atomic `fs::rename`
    ///    with `copy + remove` fallback on `EXDEV`).
    /// 6. Return the destination path.
    ///
    /// Takes `&Path` rather than `&self` because it operates on the
    /// file on disk (it re-loads the plan) — archiving is a filesystem
    /// operation, not a mutation of an in-memory `Plan`.
    pub fn archive(path: &Path, archive_dir: &Path, force: bool) -> Result<PathBuf, Error> {
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

        std::fs::create_dir_all(archive_dir)?;

        let today = chrono::Utc::now().format("%Y%m%d");
        let dest = archive_dir.join(format!("{}-{}.yaml", plan.name, today));

        if dest.exists() {
            return Err(Error::Config(format!(
                "archive target '{}' already exists; archive from a different day or remove it first",
                dest.display()
            )));
        }

        move_file_atomic(path, &dest)?;
        Ok(dest)
    }
}

/// Move a single file from `src` to `dst`. Uses `fs::rename` (atomic
/// within a filesystem); falls back to `copy` + `remove_file` on
/// `EXDEV` (cross-device) so archives on a different mount from the
/// working tree still work. Mirrors the shape of
/// [`crate::actions::move_dir_atomic`] but for a single file — kept
/// local to `plan.rs` to keep the archive path self-contained.
fn move_file_atomic(src: &Path, dst: &Path) -> Result<(), Error> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) if err.raw_os_error() == Some(18) => {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)?;
            Ok(())
        }
        Err(err) => Err(Error::Io(err)),
    }
}

/// Emit one `duplicate-name` error per duplicate *occurrence* (every
/// occurrence after the first).
fn collect_duplicate_names(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for entry in changes {
        if !seen.insert(entry.name.as_str()) {
            out.push(ValidationResult {
                level: ValidationLevel::Error,
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
fn detect_cycles(changes: &[PlanChange]) -> Vec<ValidationResult> {
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
            out.push(ValidationResult {
                level: ValidationLevel::Error,
                code: "dependency-cycle",
                message: format!("cycle: {}", path.join(" → ")),
                entry: None,
            });
        } else if scc.len() == 1 {
            let node = scc[0];
            if graph.find_edge(node, node).is_some() {
                let name = graph[node];
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
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
fn check_unknown_depends_on(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.depends_on {
            if !known.contains(target.as_str()) {
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
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
fn check_unknown_affects(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let known: HashSet<&str> = changes.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for entry in changes {
        for target in &entry.affects {
            if !known.contains(target.as_str()) {
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
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
fn check_unknown_sources(plan: &Plan) -> Vec<ValidationResult> {
    let mut out = Vec::new();
    for entry in &plan.changes {
        for key in &entry.sources {
            if !plan.sources.contains_key(key) {
                out.push(ValidationResult {
                    level: ValidationLevel::Error,
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
fn check_single_in_progress(changes: &[PlanChange]) -> Vec<ValidationResult> {
    let offenders: Vec<&PlanChange> =
        changes.iter().filter(|c| c.status == PlanStatus::InProgress).collect();
    if offenders.len() <= 1 {
        return Vec::new();
    }
    offenders
        .into_iter()
        .map(|c| ValidationResult {
            level: ValidationLevel::Error,
            code: "multiple-in-progress",
            message: "multiple in-progress entries: at most one allowed per plan".to_string(),
            entry: Some(c.name.clone()),
        })
        .collect()
}

/// Plan-to-change directory consistency:
///   - Warn on orphan subdirectories (no matching plan entry).
///   - Warn when an `in-progress` plan entry has no matching directory.
fn check_changes_dir_consistency(plan: &Plan, changes_dir: &Path) -> Vec<ValidationResult> {
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
            out.push(ValidationResult {
                level: ValidationLevel::Warning,
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
                out.push(ValidationResult {
                    level: ValidationLevel::Warning,
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

    /// The 10 legal edges from `rfc-2-plan.md` §"Transition Rules".
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

    /// Verbatim reproduction of the `rfc-2-plan.md` §"The Plan" fixture.
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
            description: None,
            status_reason: None,
        }
    }

    #[test]
    fn clean_plan_returns_no_results() {
        let plan: Plan = serde_yaml::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let results = plan.validate(None);
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
        let results = plan.validate(None);
        let dupes: Vec<_> = results.iter().filter(|r| r.code == "duplicate-name").collect();
        assert_eq!(dupes.len(), 1, "expected one duplicate-name result, got {results:#?}");
        assert_eq!(dupes[0].level, ValidationLevel::Error);
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
        let results = plan.validate(None);
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
        let results = plan.validate(None);
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
        let results = plan.validate(None);
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
        let results = plan.validate(None);
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
        let results = plan.validate(None);
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
        let results = plan.validate(None);
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
        let results = plan.validate(None);
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
        let results = plan.validate(Some(tmp.path()));
        let hits: Vec<_> = results.iter().filter(|r| r.code == "orphan-change-dir").collect();
        assert_eq!(hits.len(), 1, "expected one orphan-change-dir, got {results:#?}");
        assert_eq!(hits[0].level, ValidationLevel::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("stale-change"));
    }

    #[test]
    fn missing_dir_for_in_progress_is_warning() {
        let tmp = tempdir().expect("tempdir");
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(Some(tmp.path()));
        let hits: Vec<_> =
            results.iter().filter(|r| r.code == "missing-change-dir-for-in-progress").collect();
        assert_eq!(hits.len(), 1, "expected one missing-dir warning, got {results:#?}");
        assert_eq!(hits[0].level, ValidationLevel::Warning);
        assert_eq!(hits[0].entry.as_deref(), Some("alpha"));
    }

    #[test]
    fn present_dir_for_in_progress_is_silent() {
        let tmp = tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join("alpha")).expect("mkdir alpha");
        let plan = plan_with_changes(vec![change("alpha", PlanStatus::InProgress)]);
        let results = plan.validate(Some(tmp.path()));
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
        let results = plan.validate(None);
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
        let results = plan.validate(None);

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

        let dest = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        assert!(!plan_path.exists(), "original plan.yaml must be gone after archive");
        assert!(dest.exists(), "destination archive file must exist");
        let expected = archive_dir.join(format!("release-1-{}.yaml", today_yyyymmdd()));
        assert_eq!(dest, expected);

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

        let dest = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

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

        let dest = Plan::archive(&plan_path, &archive_dir, true).expect("force archive ok");

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

        let dest = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");
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

        let dest = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");
        let expected = archive_dir.join(format!("pkg-{}.yaml", today_yyyymmdd()));
        assert_eq!(dest, expected);
        assert!(dest.exists(), "returned path must point at an existing file");
    }

    /// Same-device happy path: `fs::rename` is atomic on a single
    /// filesystem. The `EXDEV` fallback (copy + remove) exercised by
    /// `move_file_atomic` only fires across filesystems; a single
    /// `tempdir()` sits on one mount, so the cross-device path is not
    /// unit-testable here without a platform-specific loopback mount.
    /// The fallback is covered by the `move_dir_atomic` contract in
    /// `actions.rs`, which `move_file_atomic` mirrors exactly for
    /// single files.
    #[test]
    fn archive_is_atomic_within_filesystem() {
        let tmp = tempdir().expect("tempdir");
        let archive_dir = tmp.path().join("archive");
        let plan_path = write_plan(tmp.path(), "atomic", vec![change("a", PlanStatus::Done)]);
        let pre_bytes = std::fs::read(&plan_path).expect("read pre-archive");

        let dest = Plan::archive(&plan_path, &archive_dir, false).expect("archive ok");

        assert!(!plan_path.exists());
        assert!(dest.exists());
        assert_eq!(
            std::fs::read(&dest).expect("read archived"),
            pre_bytes,
            "rename-on-same-fs must preserve byte content"
        );
    }
}
