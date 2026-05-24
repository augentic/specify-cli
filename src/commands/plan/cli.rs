//! Clap derive surface for `specify plan *` and the nested
//! `plan lock *` verbs. The umbrella `cli.rs` re-exports both action
//! enums.

use clap::{ArgAction, Subcommand};

use crate::cli::{AliasAssign, AuthorityOverrideKindAssign, SliceSourceArg, SourceArg};

/// Plan-authoring verbs (`specify plan *`).
#[derive(Subcommand)]
pub enum PlanAction {
    /// Scaffold an empty `plan.yaml` at the repo root. Refuses to
    /// overwrite an existing plan.
    Create {
        /// Kebab-case change name
        name: String,
        /// Named source binding, repeatable. Wire grammar:
        /// `--source <key>=<adapter>:<path>` for path-bound bindings,
        /// or `--source <key>=<adapter>:value:<literal>` for
        /// value-bound bindings (used by `intent`). Recorded in the
        /// plan's `sources:` map as the structured
        /// `{ adapter, path?, value? }` shape per workflow §Source.
        #[arg(long = "source")]
        sources: Vec<SourceArg>,
        /// Pre-stage `slices[].divergence: likely` on the named slice
        /// (repeatable; workflow §D5). Each occurrence fires one
        /// `plan.propose.divergence` journal event. Refuses with
        /// `plan-divergence-likely-unknown-slice` when the slice is
        /// not present in the plan; the CLI is the single writer of
        /// this field — do not edit `plan.yaml` directly.
        #[arg(long = "divergence-likely", value_name = "SLICE", action = ArgAction::Append)]
        divergence_likely: Vec<String>,
        /// Stamp `lifecycle: reviewed` atomically with create
        /// (workflow §D7). Typing this flag *is* the operator's
        /// Gate-1 consent — the CLI runs the same validation it
        /// runs on the post-create path, refuses the create on
        /// failure regardless of the flag, and on success writes a
        /// single atomic `plan.yaml` carrying `lifecycle: reviewed`
        /// plus the matching `plan.transition.reviewed` journal
        /// event. Valid on any plan shape (empty scaffold,
        /// single-slice, multi-slice).
        #[arg(long = "auto-review", action = ArgAction::SetTrue)]
        auto_review: bool,
        /// Pre-seed a per-slice `authority-override` entry on a
        /// named slice (workflow §D3). Each occurrence takes two
        /// positional values: the slice name and a
        /// `<claim-kind>=<source-key>` assignment. Repeatable; later
        /// occurrences override earlier ones on the same
        /// `(slice, kind)` tuple. The slice MUST already exist in
        /// the plan being created (unknown names short-circuit with
        /// `plan-authority-override-unknown-slice`); the source key
        /// is validated at `specify slice validate` time via the
        /// orphan-key check. One
        /// `plan.amend.authority-override` journal event fires per
        /// resolved entry in the same batched append as
        /// `--auto-review` / `--divergence-likely`.
        #[arg(
            long = "authority-override",
            value_names = ["SLICE", "KIND=KEY"],
            num_args = 2,
            action = ArgAction::Append,
        )]
        authority_override: Vec<String>,
    },
    /// Validate plan.yaml (structure + plan/change consistency).
    ///
    /// Includes the four health diagnostics — `cycle-in-depends-on`,
    /// `orphan-source-key`, `stale-workspace-clone`, and
    /// `unreachable-entry` — alongside the base shape rules.
    Validate,
    /// Return the active in-progress entry, or transition the next eligible
    /// `Pending` entry to `InProgress` and return it. `plan next` is the
    /// only writer of per-entry `in-progress` (workflow §CLI surface).
    Next,
    /// Add a new plan entry (status: pending)
    Add {
        /// Kebab-case change name
        name: String,
        /// Ordering dependencies (repeatable). Every value is a change name in the plan.
        /// Pass `--depends-on` (with no value) to clear the field; omit the flag to
        /// leave it unchanged.
        #[arg(long = "depends-on", action = ArgAction::Append)]
        depends_on: Vec<String>,
        /// Per-slice source binding (repeatable). Wire form is
        /// `<key>=<candidate-id>`; bare `<key>` is accepted as
        /// shorthand for `{ key: <key>, candidate: <slice.name> }`
        /// per workflow §`Slice.sources`.
        #[arg(long = "sources", action = ArgAction::Append)]
        sources: Vec<SliceSourceArg>,
        /// Free-text scoping hint for the define step
        #[arg(long)]
        description: Option<String>,
        /// Target registry project name
        #[arg(long)]
        project: Option<String>,
        /// Plan-entry target-adapter identifier for project-less entries (e.g. `contracts@v1`)
        #[arg(long)]
        target: Option<String>,
        /// Baseline paths relevant to this change, relative to `.specify/` (repeatable)
        #[arg(long)]
        context: Vec<String>,
        /// Set a per-slice `authority-override` entry on the slice
        /// being added (workflow §D3). Wire form is
        /// `<claim-kind>=<source-key>`; both sides are kebab-case
        /// and the kind is checked against the closed [`ClaimKind`]
        /// enum at parse time. Repeatable; later occurrences win on
        /// the same `(kind)` key. Orphan source keys are caught by
        /// `specify slice validate`. One
        /// `plan.amend.authority-override` event fires per resolved
        /// entry.
        #[arg(long = "authority-override", action = ArgAction::Append)]
        authority_override: Vec<AuthorityOverrideKindAssign>,
    },
    /// Edit non-status fields on an existing plan entry.
    ///
    /// Three orthogonal flag families operate on `sources`:
    ///
    /// - `--sources <binding>` (with `num_args = 0..`) replaces the
    ///   slice's `sources` array wholesale.
    /// - `--add-source <binding>` (repeatable) adds a single binding.
    /// - `--remove-source <key>` (repeatable) removes a binding by
    ///   key; fails with `plan-binding-not-found` when no binding
    ///   matches.
    ///
    /// `--add-source` and `--remove-source` apply after `--sources`,
    /// so wholesale replacement plus targeted edits can be combined
    /// in a single invocation when needed.
    Amend {
        /// Kebab-case change name
        name: String,
        /// Replace depends-on. Pass `--depends-on` (with no value) to clear the
        /// field; omit the flag to leave it unchanged. Repeat or comma-separate
        /// to supply multiple values.
        #[arg(long = "depends-on", num_args = 0.., value_delimiter = ',')]
        depends_on: Option<Vec<String>>,
        /// Replace per-slice source bindings wholesale. Each value
        /// is `<key>=<candidate-id>` (or bare `<key>` shorthand).
        /// Pass `--sources` (no value) to clear; omit to leave
        /// unchanged.
        #[arg(long = "sources", num_args = 0.., value_delimiter = ',')]
        sources: Option<Vec<SliceSourceArg>>,
        /// Add a single per-slice source binding (repeatable). Each
        /// value is `<key>=<candidate-id>` or the bare `<key>`
        /// shorthand per workflow §`Slice.sources`.
        #[arg(long = "add-source", action = ArgAction::Append)]
        add_source: Vec<SliceSourceArg>,
        /// Remove a per-slice source binding by key (repeatable).
        /// Fails with `plan-binding-not-found` when no such binding
        /// exists on the slice.
        #[arg(long = "remove-source", action = ArgAction::Append)]
        remove_source: Vec<String>,
        /// Set the slice's `divergence` field (workflow §Plan-time
        /// fusion; workflow §D5). Accepts `likely`, `accepted`, or
        /// `rejected` — the CLI is the single writer of this field
        /// across every value of the closed enum, so use
        /// `specify plan amend <plan> <slice> --divergence likely`
        /// (or `--divergence accepted|rejected`) instead of editing
        /// `plan.yaml` by hand. `none` (absent) is the implicit
        /// default; omit this flag to leave the field unchanged.
        #[arg(long = "divergence")]
        divergence: Option<String>,
        /// Replace description. Pass `--description ""` to clear; omit the flag
        /// to leave it unchanged.
        #[arg(long)]
        description: Option<String>,
        /// Replace project. Pass `--project ""` to clear; omit the flag to leave it unchanged.
        #[arg(long)]
        project: Option<String>,
        /// Replace the plan-entry target-adapter identifier. Pass `--target ""` to clear;
        /// omit the flag to leave it unchanged.
        #[arg(long)]
        target: Option<String>,
        /// Replace context paths. Pass `--context` (with no value) to clear; omit the
        /// flag to leave it unchanged.
        #[arg(long, num_args = 0.., value_delimiter = ',')]
        context: Option<Vec<String>>,
        /// Set a per-slice `authority-override` entry (workflow §D3).
        /// Two positional values per occurrence: the slice name and
        /// a `<claim-kind>=<source-key>` assignment. Repeatable;
        /// later occurrences override earlier ones on the same
        /// `(slice, kind)` tuple. If the same `(slice, kind)` also
        /// appears in `--clear-authority-override`, the clear
        /// wins (clears apply after sets). Validated against the
        /// closed [`ClaimKind`] enum at parse time; orphan source
        /// keys are caught by `specify slice validate`.
        #[arg(
            long = "authority-override",
            value_names = ["SLICE", "KIND=KEY"],
            num_args = 2,
            action = ArgAction::Append,
        )]
        authority_override: Vec<String>,
        /// Remove a single `(slice, kind)` entry from the
        /// per-slice `authority-override` map (workflow §D3). Two
        /// positional values per occurrence: the slice name and
        /// the claim kind (closed enum, kebab-case). Repeatable;
        /// no-op when the entry was already absent. Applied after
        /// `--authority-override` sets so a same-invocation set +
        /// clear pair resolves to the cleared state.
        #[arg(
            long = "clear-authority-override",
            value_names = ["SLICE", "KIND"],
            num_args = 2,
            action = ArgAction::Append,
        )]
        clear_authority_override: Vec<String>,
        /// Wipe the entire per-slice `authority-override` map on
        /// the named slice (workflow §D3). Repeatable for multiple
        /// slices. Applied last, after `--authority-override` sets
        /// and `--clear-authority-override` clears. One
        /// `plan.amend.authority-override` event with `action: clear`
        /// fires per kind that was actually present in the map
        /// before the wipe (no events when the map was already
        /// empty).
        #[arg(
            long = "clear-authority-overrides",
            value_name = "SLICE",
            num_args = 1,
            action = ArgAction::Append,
        )]
        clear_authority_overrides: Vec<String>,
        /// Append an alias to a candidate in `<project_dir>/discovery.md`
        /// (workflow §D6). Wire form is `<candidate-id>=<alias>`; both
        /// sides are kebab-case. Repeatable. Mutates `discovery.md`
        /// (NOT `plan.yaml`); the whole amend is refused at exit 2
        /// (`discovery-alias-collision`) when the new alias would
        /// collide with any other candidate's `id` or `aliases[]` in
        /// the same `discovery.md`. Operator additions through this
        /// flag survive re-enumeration so long as the source adapter
        /// keeps emitting the bearing candidate's `id` (workflow §D6).
        #[arg(long = "add-alias", action = ArgAction::Append)]
        add_alias: Vec<AliasAssign>,
        /// Remove an alias from a candidate in
        /// `<project_dir>/discovery.md` (workflow §D6). Wire form is
        /// `<candidate-id>=<alias>`; idempotent (no-op when the
        /// alias is already absent). Repeatable. The whole amend
        /// fails at exit 2 (`discovery-candidate-unknown`) when no
        /// candidate has the named id.
        #[arg(long = "remove-alias", action = ArgAction::Append)]
        remove_alias: Vec<AliasAssign>,
    },
    /// Apply a validated status transition.
    ///
    /// Two transition shapes share this verb (workflow §CLI surface):
    ///
    /// - **Plan-level Gate 1 stamp** — `<name>` is the plan name and
    ///   `<target>` is `reviewed`. Operator-only — `/spec:plan` MUST
    ///   NOT call this verb; skill bodies stop at `pending` and print
    ///   the literal `specify plan transition <name> reviewed`
    ///   command in their closing hint for the operator to run.
    /// - **Per-entry close** — `<name>` is a plan-entry name and
    ///   `<target>` is `done`. The `/spec:merge` skill is the
    ///   canonical caller.
    ///
    /// Per-entry `pending` is written only by `plan add` / `plan amend`;
    /// per-entry `in-progress` is written only by `plan next`. v1 has
    /// no per-entry `blocked`, `failed`, or `skipped` state — build
    /// failures and merge conflicts leave the active entry `in-progress`.
    Transition {
        /// Plan name (for plan-level `reviewed`) or kebab-case entry
        /// name (for per-entry `done`).
        name: String,
        /// Transition target — `reviewed` (plan-level) or `done`
        /// (per-entry).
        target: String,
    },
    /// Archive the current plan to `.specify/archive/plans/<name>-<YYYYMMDD>.yaml`
    Archive {
        /// Archive even when the plan has pending or in-progress entries.
        /// Without --force, these non-terminal statuses block the archive.
        #[arg(long)]
        force: bool,
    },
}
