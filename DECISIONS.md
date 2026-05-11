# Decisions

A running log of architectural decisions made while implementing Specify. Each
entry links back to the RFC (or change plan) that prompted it.

## Change A — `Error::Validation` payload

**Decision.** `specify-validate` owns the canonical `ValidationResult`
(`Pass` / `Fail` / `Deferred`) enum. `specify-error` exposes a small leaf-level
projection, `ValidationResultSummary` (status as `String`, plus `rule_id`,
`rule`, optional `detail`), and the `Error::Validation` variant carries a
`Vec<ValidationResultSummary>` plus a `count`.

**Rationale.** RFC-1 §`error.rs` (see
[rfcs/rfc-1-cli.md](rfcs/rfc-1-cli.md)) and the Change A scope in the
[RFC-1 implementation plan](rfcs/rfc-1-plan.md#change-a--workspace-scaffold-specify-error-ci)
offer two options: (a) forward-declare `ValidationResult` in
`specify-error`, or (b) keep the rich type in `specify-validate` and make
`Error::Validation` carry a summary. We picked (b) because it preserves the
"leaves to root" dependency contract from RFC-1 §Workspace Layout —
`specify-error` stays dependency-free from every other workspace crate
(`thiserror` + `serde-saphyr` only) — while still giving the CLI enough
structured data (status, rule id, rule description, detail) to render
sensible failure output without reaching back into `specify-validate`. The
cost is a lossy projection at the crate boundary (the enum variant collapses
into a `status` string), which is acceptable because callers who need the
full fidelity can consume `specify_validate::ValidationResult` directly.

## Change E — Task skill directive format

**Decision.** `specify-task::parse_tasks` recognises skill directives as a
trailing HTML comment of the form `<!-- skill: plugin:skill -->` (colon
separator between plugin and skill name).

**Rationale.** The RFC-1 implementation plan
([rfcs/rfc-1-plan.md](rfcs/rfc-1-plan.md#change-e--specify-task-parser--mark_complete))
describes the attached directive as `[plugin/skill]` (brackets, slash
separator). The authoritative on-disk format in
[plugins/spec/references/specify.md §"Skill Directive Tags"](plugins/spec/references/specify.md)
and the current `plugins/spec/skills/build/SKILL.md` parser both use the
HTML-comment form with a colon separator
(`<!-- skill: omnia:crate-writer -->`). The reference file is the source of
truth for what humans and the `define` skill actually write, so the parser
goes with the on-disk form. The public `SkillDirective { plugin, skill }`
field names are unchanged — those names carry semantics, not the separator
character — so downstream consumers need no adjustment when the plan's
bracket form is eventually re-aligned in the RFC.

## Change G — `ValidationResult` canonical home

**Decision.** `ValidationResult` stays in `specify-schema`; `specify-validate`
re-exports it via `pub use specify_schema::ValidationResult;`. Change G keeps
the type *where Change B put it* rather than relocating it to `specify-validate`
as the RFC-1 plan originally suggested.

**Rationale.** `specify-validate` depends on `specify-schema` for
`PipelineView` (the runner walks the pipeline to discover artifacts). Moving
`ValidationResult` into `specify-validate` would require `specify-schema` to
re-export it from the downstream crate, closing a dependency cycle
(`schema → validate → schema`). Introducing a fourth "types" crate just for
one enum would cost more than it saves: `ValidationResult` has three call
sites today (`Schema::validate_structure`, `CacheMeta::validate_structure`,
and the `specify-merge::validate_baseline` coherence check), all of which
already depend on `specify-schema`. Keeping the enum in the leaf-ish type
crate and re-exporting from `specify-validate` preserves the layering
without new wiring. The `TODO(RFC-1 Change G)` comment that anticipated the
move has been replaced with a pointer to this decision.

## Change G — Terminology inference

**Decision.** `specify-validate` infers the deliverable terminology (`"crate"`
vs `"feature"`) from the schema name: `omnia` → `"crate"`, `vectis` →
`"feature"`, anything else defaults to `"crate"`. Rules that hinge on
terminology (e.g. `proposal.crates-listed`, `cross.proposal-crates-have-specs`)
read `BriefContext.terminology` / `CrossContext.terminology` rather than
hard-coding the heading.

**Rationale.** RFC-1 plan line 661 specifies the terminology field and calls
out the two current schema flavours. `schema.yaml` does not (yet) carry a
first-class terminology field, and adding one just for validator heading
choice would couple schema authors to a concept they otherwise never touch.
Mapping on `schema.name` is cheap, matches the two in-repo schemas
(`omnia`/`vectis`) exactly, and the `"crate"` default means user-authored
schemas continue to work without a schema.yaml change. Adding a proper
`terminology` field to `schema.yaml` remains an option later (the
`infer_terminology` helper is the single choke point); we've kept the
surface area minimal until a concrete schema needs it.

## Change H — Platform stub layering

> **Superseded by RFC-9 §1B.** The `specify-platform` crate, the
> `PlatformConfig` trait, and the `parse_platform_config` entry point have
> all been retired. `Registry` (in `specify-schema`) is the single peer
> catalogue; the layering rationale below is preserved for historical
> context only.

**Decision.** `specify-platform::parse_platform_config` takes a generic
`Cfg: PlatformConfig` parameter rather than a concrete `&ProjectConfig`.
The `PlatformConfig` trait is declared (empty) in `specify-platform`;
`ProjectConfig` (defined in the root `specify` crate in Change I) will add
a zero-method `impl PlatformConfig for ProjectConfig {}`. RFC-3 extends
the trait with real methods (`fn peers(&self) -> &[…];` and similar).

**Rationale.** RFC-1 §`federation.rs`
([rfcs/rfc-1-cli.md](rfcs/rfc-1-cli.md) line 898) shows
`parse_platform_config(config: &ProjectConfig)`. Implementing that
signature verbatim would require `specify-platform` to depend on the
root `specify` crate — but the root crate depends on `specify-platform`
(it re-exports the public API per
[RFC-1 plan line 202](rfcs/rfc-1-plan.md#change-h--stubs-specify-drift-specify-platform)),
producing a dependency cycle. Moving `ProjectConfig` down into a leaf
config crate was considered and rejected: Change I deliberately keeps
config + init + CLI plumbing in the root crate so the binary has a single
assembly point, and splitting it would add a fourth "plumbing" crate for
no payoff.

The trait-in-the-leaf-crate approach keeps `specify-platform` dependency-
free from the root crate while freezing the call-site signature today, so
Change I and every subsequent Change can wire through
`parse_platform_config(&config)` without a later refactor. The empty
trait costs nothing at the type level — it's a pure capability marker
until RFC-3 fills it in.

## Change I — CLI exit codes and version-floor semantics

**Decision.** The `specify` binary commits to a four-slot exit-code table
and centralises the `specify_version` floor check inside
`ProjectConfig::load`:

| Code | Name                      | When                                                                 |
|------|---------------------------|----------------------------------------------------------------------|
| 0    | `EXIT_SUCCESS`            | Command succeeded.                                                   |
| 1    | `EXIT_GENERIC_FAILURE`    | Any `Error` variant not listed below (I/O, YAML, schema, merge, tool resolver/runtime, …). |
| 2    | `EXIT_VALIDATION_FAILED`  | A validation command returns findings, `Error::Validation { .. }` bubbles up, or a tool request is rejected as undeclared / outside its declared permissions. |
| 3    | `EXIT_VERSION_TOO_OLD`    | `.specify/project.yaml.specify_version` is newer than `CARGO_PKG_VERSION` — surfaced as `Error::CliTooOld` and JSON `specify-version-too-old`. |

`CliResult::from(&Error)` is the single source of truth for the
mapping; every subcommand dispatcher routes its error through it so the
table stays honest regardless of which crate raised the error. The
variants (`Success`, `GenericFailure`, `ValidationFailed`,
`VersionTooOld`) live in `src/output.rs` alongside a module-level doc
comment in `src/main.rs` that reproduces the table for skill authors.

**Decision.** `ProjectConfig::load` is the choke point for the
version-floor check. Every subcommand that reads `project.yaml` goes
through `load` (`specify init` bypasses the check because the file
doesn't exist yet; `specify schema resolve/check` bypass because they
don't need `project.yaml`). This means a new subcommand added in a later
Change inherits the floor check for free — forgetting to add the check
is no longer possible at the subcommand dispatch site.

**Decision.** Unparseable `specify_version` values are permissive —
`ProjectConfig::load` treats any non-`semver`-parseable pin as "not
older" and loads successfully. The alternative (hard-fail on a bad
version string) was rejected because `project.yaml` is a human-edited
YAML file and a typo in `specify_version` should not brick the project:
the user can still run `specify init --upgrade` (Change J) or fix the
field in an editor, whereas a hard failure would force them off the CLI
entirely. Deliberate downgrades of the pin remain possible — the field
is still a floor, just a lenient one.

## Change J — golden JSON generation

**Decision.** End-to-end golden files under
`tests/fixtures/e2e/goldens/` are generated by the same test binary that
consumes them, gated behind the `REGENERATE_GOLDENS` environment
variable. To refresh after an intentional stdout shape change:

```sh
REGENERATE_GOLDENS=1 cargo test --test e2e
git diff tests/fixtures/e2e/goldens/
```

Commit the diff once it matches the expected new shape. Running
`cargo test --test e2e` without the env var compares stdout against the
checked-in golden and fails on any divergence.

**Decision.** Before comparison, every string value in the parsed
stdout is rewritten with two substitutions:

1. The raw `TempDir` path (`tmp.path()`) → `<TEMPDIR>`.
2. The canonicalised `TempDir` path (resolves macOS's
   `/var/folders/...` → `/private/var/folders/...`) → `<TEMPDIR>`.

The walker (`strip_substitutions` in `tests/e2e.rs`) recurses into
arrays and objects so nested paths (e.g. `resolved_path`,
`new_content_path`) are stripped regardless of where they appear.
Dates never leak into JSON goldens — the only date-formatted field in
Change J's JSON surface is the archive directory name (`<YYYY-MM-DD>-<name>`),
which is an on-disk artifact asserted via `fs::read_dir` rather than
via stdout.

**Rationale.** Goldens are the clearest way to pin the
`schema_version: 1` contract across every subcommand: a single file per
subcommand captures the entire response shape, so skill authors can
`diff` against them when upgrading. Generating them from the test
itself (rather than an external script) keeps the regeneration workflow
a single Cargo command and avoids the drift between "what the script
writes" and "what the tests read" that a separate generator would
introduce. Tempdir substitution is mandatory because both `init` and
`schema resolve` surface absolute filesystem paths in their JSON
payloads; the placeholder keeps goldens machine-independent without
losing the structural assertion that the path *was* produced.

## Change J — Golden JSON generation workflow

**Decision.** End-to-end tests (`tests/e2e.rs`) compare stdout against
checked-in golden JSON files at
`tests/fixtures/e2e/goldens/<name>.json`. The `REGENERATE_GOLDENS`
environment variable gates rewrites — when set, the test harness
overwrites the matching golden with the actual (tempdir-stripped)
output instead of asserting equality.

Regeneration:

```bash
REGENERATE_GOLDENS=1 cargo test --test e2e
git diff tests/fixtures/e2e/goldens/
# inspect the diff, commit if intentional
```

**Tempdir stripping rule.** Each test runs inside a throw-away
`tempfile::TempDir`. Before writing or comparing a golden the harness
walks the JSON tree and replaces any string starting with the tempdir
path (raw or canonicalised — the macOS `/var` vs `/private/var` split
is normalised) with `"<TEMPDIR>"` + the trailing suffix. Goldens are
therefore machine-independent and stable across CI runners.

**Retry discipline.** If a golden comparison fails, inspect the diff
first. A real behavioural regression should surface as a mismatch;
only regenerate after understanding why the shape changed.

## Change J — Subcommand wiring boundaries

**Decision.** `src/main.rs::run_merge` reconstructs the archive path
from `(archive_dir, today, change_basename)` rather than extending
`specify_merge::merge_change`'s return type to include it. The `merge`
function's post-conditions guarantee the layout — there is exactly one
archive target per invocation and it is named
`<YYYY-MM-DD>-<change-name>/` — so re-deriving the path at the CLI
boundary avoids threading a `PathBuf` through the domain crate's
return type and preserves `MergeResult` as a pure record of the
in-memory merge output.

**Decision.** `resolve_tasks_path` lives in `src/main.rs` (not in a
new crate) and is shared by `specify task progress` and `specify task
mark`. It honours the schema's `build.tracks → tasks.generates`
chain instead of hard-coding `tasks.md`, so a schema that renames the
task list brief picks up the new path for free. The helper accepts a
pre-computed project dir as an argument so callers that already loaded
`ProjectConfig` don't re-derive it.

**Decision.** `specify status` skips changes whose `.metadata.yaml`
can't be loaded. Change directories that pre-date metadata (e.g. an
`initial-baseline` scaffold that hasn't yet been through `define`)
would otherwise poison the listing with per-change errors. A concrete
`specify status <name>` call against a missing change still surfaces
the underlying `Error::Config`, so the "silent skip" only kicks in
for the bulk listing path.

## Option-2 phase primitives — `specify change`, `specify schema pipeline`, `specify spec`

**Decision.** Move every deterministic operation currently inlined in
the phase skills (`/spec:define`, `/spec:build`, `/spec:merge`,
`/spec:drop`) into CLI subcommands backed by shared library functions.
The skills retain only agent-driven work (brief-body interpretation,
artifact generation, plugin-skill dispatch, user prompts).

New CLI surface:

- `specify change {create, list, status, transition, touched-specs, overlap, archive, drop}` — lifecycle verbs backed by
  `specify_change::actions::{create, transition, scan_touched_specs,
  overlap, archive, drop}`. `ChangeMetadata` gains `merged_at`,
  `dropped_at`, and `drop_reason` fields. `Change::archive` is the
  sole implementation of the archive move; `merge_change` calls it
  instead of duplicating the cross-device-safe rename fallback.
- `specify schema pipeline <phase> [--change <dir>]` — topo-sorted
  briefs plus per-brief completion relative to a change, powered by
  `PipelineView::topo_order(phase)` (Kahn's algorithm over the
  in-phase `needs` subgraph) and `PipelineView::completion_for(phase,
  change_dir)`. `collect_status` in `src/main.rs` was refactored to
  call `completion_for`, so `specify status`, `specify schema pipeline`,
  and future skill callers agree byte-for-byte on what "complete"
  means.
- `specify spec preview <change_dir>` / `specify spec conflict-check
  <change_dir>` — no-write counterparts to `specify merge`. `preview`
  is factored out of `merge_change` as `preview_change`; `conflict_check`
  compares each `type: modified` `touched_spec`'s baseline mtime against
  `ChangeMetadata::defined_at`.

**Rationale.** The three phase skills were reimplementing roughly the
same set of deterministic operations — kebab-case validation, YAML
read/write, schema resolution, pipeline topology, artifact existence
checks, `.metadata.yaml` status transitions, delta merge, archive
move — in prose. Moving them into the CLI gives humans, agents, and
CI one place to reason about lifecycle semantics, one transition state
machine to maintain, and one archive move implementation. Integration
tests under `tests/{change,schema,spec}.rs` (favouring assert_cmd +
golden JSON over unit tests per the Option-2 plan) pin the JSON
contract for downstream skill callers.

**New integration test files.** `tests/change.rs` (14 tests) covers
every `specify change` verb; `tests/schema.rs` (3 tests) covers
`specify schema pipeline`; `tests/spec.rs` (6 tests) covers
`specify spec preview` and `specify spec conflict-check`. Each stands
up a tempdir project via `specify init`, exercises the verb under
test, and asserts both JSON shape and filesystem side effects.

## Option-2 phase primitives — legacy Python retirement

**Decision.** The archived Python reference implementation — formerly at
`scripts/legacy/merge-specs.py` — is retired. The Rust merge engine
under `specify-merge` and the parser under `specify-spec` are the sole
implementations; no skill, CLI invocation, or test calls Python at
runtime.

**What stayed.** The parity fixtures under
`tests/fixtures/parity/` remain as a frozen regression baseline for
both `specify-merge::merge` / `validate_baseline` and
`specify-spec::{parse_baseline, parse_delta}`. Source-level code
comments that reference the Python reference have been rephrased to
describe it as "the archived Python reference" rather than linking to
the deleted file.

**What was removed.** `scripts/legacy/merge-specs.py`,
`scripts/legacy/README.md`, and `tests/fixtures/parity/regenerate.sh`
(the regeneration script that shelled out to the Python binary).
Changes to the frozen fixtures now land hand-crafted in the same
commit as the corresponding Rust edit.

**Rationale.** The Python script had been dead code since the Rust
port landed: no skill, `specify` subcommand, or automated test invoked
it. Keeping it in-tree added no testable value and, worse, implied a
dependency (`python3`) that the project no longer has. Retiring it in
this change alongside the skill rewrites closes the loop on the
Option-2 phase-primitives consolidation — the CLI is now the single
implementation of spec merge semantics.

## Change L1.C — tempfile promotion to runtime dependency

**Decision.** `tempfile` moves from `[dev-dependencies]` to
`[dependencies]` in `crates/change/Cargo.toml` (pinned to `tempfile = "3"`,
matching the root workspace). The dev-dependency entry stays so that the
existing unit tests continue to pull it in under `cfg(test)` without any
new feature flag.

**Rationale.** `Plan::save` uses `tempfile::NamedTempFile::new_in(parent)`
+ `persist(path)` to make the on-disk update atomic: the YAML is written
to a temp file in the same directory as the target, then renamed over the
target. Because `rename(2)` is atomic only within a single filesystem,
the temp file must live in the target's parent directory — which is
exactly what `NamedTempFile::new_in` guarantees. Alternatives considered:

- Hand-rolling the temp-name + `fs::rename` pattern: rejected because we
  would have to reimplement collision-safe random naming, cleanup-on-drop,
  and cross-platform persist semantics that `tempfile` already has.
- Using `fs::write` directly (the pattern `ChangeMetadata::save` uses):
  rejected because `plan.yaml` is the authoritative driver for a
  multi-change pipeline. A reader racing a writer mid-`write` would
  observe a truncated YAML and either fail to parse or, worse, parse a
  partial structure; atomicity at the filesystem level is the simplest
  way to pin that invariant. `ChangeMetadata::save` can be upgraded to
  the same pattern in a later change, but L1.C's scope is `Plan` only.

Promoting the crate costs one extra dependency in the release build of
`specify-change`. `tempfile` is already transitively in the dependency
tree (every test binary pulls it in), and it is a small, well-maintained
crate with no additional transitive cost in practice.

## RFC-2 milestone — Layer 1 / 2 / 3 delivered

Summary of the RFC-2 build (29 Changes across two repos). Layer 1
delivers the plan format and CLI primitives (`specify plan
{init, validate, next, status, create, amend, transition, archive}`)
plus the JSON Schema. Layer 2 delivers /spec:execute — phase outcome
contract on .metadata.yaml, journal.yaml append-only audit log,
.specify/plan.lock advisory driver lock, self-heal on startup,
--loop mode, and sources/affects execution wiring. Layer 3 delivers
/spec:plan — pipeline.plan briefs for Omnia and Vectis, discovery
via /spec:extract, interactive propose with accept/edit/reject, and
a working-directory under .specify/plans/<name>/ archived alongside
the plan.

Key technical decisions locked during the build (see individual
Change SHAs on the rfc-2 branch for detail):

- PlanStatus state machine uses matches! over (self, target) tuples
  mirroring LifecycleStatus.
- Plan::save uses NamedTempFile::new_in(parent) + persist for
  atomic writes; ChangeMetadata::save was migrated to the same
  pattern in L2.A.
- petgraph backs validate / topological_order; next_eligible uses
  a list-order tie-break and is independent of topo sort (works
  even on cyclic plans).
- PlanLockStamp is a PID-file-only lock (no flock) because the
  driver runs across multiple short-lived CLI invocations;
  PlanLockGuard remains for in-process long-lived use.
- Phase outcome transport: .metadata.yaml:outcome field; journal.yaml
  is a pure audit log never consumed by the driver.
- specify-change depends on specify-schema (for Phase) — one-direction
  edge, no cycle.
- Phase::Plan added ahead of Define/Build/Merge; Schema::plan_entries
  is a separate accessor so existing Schema::entries() iteration
  is unchanged.
- --dry-run on /spec:execute runs self-heal in REPORT-ONLY mode
  (resolved the L2.G/L2.H tension).

## RFC-3b milestone — multi-repo platform routing

RFC-3b extends the plan and registry for multi-repo execution
routing. Key additions and decisions:

- RegistryProject gains `description: Option<String>` with
  `#[serde(default)]`. Required when `projects.len() > 1`;
  validated by `Registry::validate_shape`.
- PlanChange gains `project: Option<String>` with
  `#[serde(default)]`. Required for multi-project registries;
  optional for single-project. `plan.schema.json` updated.
- PlanChangePatch gains `project: Option<Option<String>>` with
  the same three-way semantics as `description` (None = leave,
  Some(None) = clear, Some(Some(s)) = replace).
- `specify plan add` and `specify plan amend` gain `--project`
  flag, validated against the loaded registry at write time.
  (RFC-3b originally introduced this on `specify plan create`; the
  entry-append verb was renamed to `add` by RFC-9 §1G.)
- `specify plan next --format json` gains `project`, `description`,
  and `sources` in the response when an eligible entry is found.
  Fields are absent when `reason` is non-null.
- `Plan::validate` signature changed from unused `_project_dir` to
  `registry: Option<&Registry>`, enabling cross-validation.
- Four new validation codes: `project-not-in-registry`,
  `project-missing-multi-repo`, `description-missing-multi-repo`
  (on the registry side), and `schema-mismatch-workspace` (warning).
- `specify merge` auto-commits `.specify/specs/` and
  `.specify/archive/` in workspace clones. Detection heuristic:
  CWD under `*/.specify/workspace/*/` with `.specify/project.yaml`
  present and no `.specify/plan.yaml`. Commit failure is a warning.
- `specify workspace push` is a new verb: creates
  `specify/<initiative-name>` branch per dirty clone, pushes to
  remote (creating GitHub repo via `gh` if needed for greenfield),
  opens PR. `--dry-run` mode performs no writes.
- `specify workspace sync` gains greenfield bootstrapping: when a
  clone fails (repo not found), creates the workspace slot via
  `git init` + `specify init` using the initiating repo's
  `.specify/.cache/` for schema resolution.
- `extract_github_slug` utility function for URL → `org/repo`
  extraction, with unit tests covering six URL forms.

## YAML library — serde_yaml_ng → serde-saphyr

**Decision.** Replace `serde_yaml_ng` (0.9) with `serde-saphyr` (0.0.25)
as the workspace YAML (de)serialization library. Code that previously
used `serde_yaml_ng::Value` for untyped DOM manipulation now uses
`serde_json::Value`, which was already in the dependency graph.

**Rationale.** `serde_yaml_ng` is a community fork of the deprecated
`serde_yaml` and carries forward the same architectural debt.
`serde-saphyr` is an actively maintained, pure-Rust, panic-free
library built on `saphyr-parser` and recommended by the Rust community
as the forward-looking choice.

`serde-saphyr` deliberately omits a `Value` type (it deserializes
directly into Rust types without an intermediate tree). The four files
that used `serde_yaml_ng::Value` for dynamic YAML access
(`crates/merge/src/composition.rs`, `crates/validate/src/registry.rs`,
`tests/change.rs`, `src/commands/plan.rs`) now deserialize into
`serde_json::Value` instead. This simplified key handling in the
composition merge (JSON maps use `String` keys, removing
`Value::String(...)` constructors) and the validation rules
(`.get("key")` accepts `&str` directly on `serde_json::Map`).

`serde-saphyr` uses separate error types for deserialization
(`serde_saphyr::Error`) and serialization (`serde_saphyr::ser::Error`),
unlike `serde_yaml_ng`'s unified error. Both are wrapped behind
`specify_error::YamlError` / `specify_error::YamlSerError` so the
upstream crate name does not leak through every `specify-*` public
surface; `specify-error::Error` carries both via
`Yaml(#[from] YamlError)` and `YamlSer(#[from] YamlSerError)`, plus
explicit `From<serde_saphyr::Error>` and `From<serde_saphyr::ser::Error>`
impls that go through the wrappers so `?` keeps working on raw saphyr
results.

**Risks.** `serde-saphyr` is pre-1.0 (0.0.x) and its API may shift.
The dependency is pinned to `0.0.25`. YAML serialization output may
differ in whitespace or quoting from `serde_yaml_ng`; all existing
tests pass against the new output.

## v2 layout — platform artifacts at the repo root

**Decision.** Move the operator-facing platform artifacts —
`registry.yaml`, `plan.yaml`, `initiative.md`, `contracts/` — from
`.specify/` to the repo root. `.specify/` retains the
framework-managed state every CLI verb writes through (configuration
under `project.yaml`, `changes/`, `specs/`, `archive/`, `.cache/`,
`workspace/`, `plans/`, `plan.lock`). The boundary is "operator
artifacts at root, framework state under `.specify/`".

The CLI ships:

- `ProjectConfig::registry_path` / `plan_path` / `initiative_path`
  helpers (alongside the pre-existing `contracts_dir` which already
  pointed at the root). Every call site routes through these helpers.
- A new `specify migrate v2-layout` verb that renames each present
  legacy artifact in place. Idempotent. Refuses to clobber an
  existing destination. Refuses to run inside a workspace clone.
- A hard-cutover detector at the project-aware command boundary
  (`run_with_project`) that surfaces `Error::LegacyLayout` (stable
  code `legacy-layout`, exit 1) and tells the operator to run the
  migrate verb.

**Rationale.** `.specify/` started life as workflow scratch — cache,
archive, working changes, lifecycle metadata. The artifacts that
have accreted there since (the registry, the operator brief, the
plan, contracts) are durable, PR-reviewed, human-edited material.
Putting them under a dot-prefixed framework directory understated
their importance and forced operators to navigate framework
internals to inspect or hand-edit them. Pulling them up to the root
makes the boundary explicit: framework owns `.specify/`; operators
own everything else.

**Hard cutover, no transition window.** The CLI does not silently
read both layouts. An operator on a v1-layout repo running any
project-aware verb gets `Error::LegacyLayout` and is pointed at the
migrate command. The trade-off: a one-time stop for every operator
when they upgrade past `0.2.0`, against a smaller surface to
maintain (no dual-read code paths, no warning lifecycle).

**Plan::archive.** The function gained an explicit
`initiative_path: &Path` parameter so it no longer infers the brief
location from the plan path's parent. Callers pass
`ProjectConfig::initiative_path(project_dir)`. The archive co-moves
both the plan file (now at the repo root) and the brief (also at
the root) into `.specify/archive/plans/<name>-<YYYYMMDD>/`.

**Risks.** Hand-written tooling that hard-codes `.specify/registry.yaml`
or sibling paths breaks on upgrade. The migrate verb addresses every
in-repo case; downstream tooling consumers must adopt the root paths
in lockstep with the version bump.

## Integration tests — keep per-file binaries (no `tests/it.rs` umbrella)

**Decision.** The root crate's integration tests stay as 14 separate
`tests/<name>.rs` binaries instead of being consolidated under a single
`tests/it.rs` umbrella. Crate-level test suites (`crates/*/tests/`)
likewise keep their per-file split.

**Rationale.** The standard "umbrella `tests/it.rs`" optimisation
([matklad's *Delete Cargo Integration Tests*](https://matklad.github.io/2021/02/27/delete-cargo-im.html))
trades parallelism for fewer link steps. The win is large when each
integration binary drags in a heavy dependency graph the parent crate
does not, because the linker work is duplicated per binary. We
measured it on this repo's `code-review` branch (post-R17):

| Configuration                              | `cargo nextest run --no-run` (cargo-reported) | Wall-clock |
|--------------------------------------------|-----------------------------------------------|------------|
| Baseline (14 separate `tests/*.rs`)        | 34.06 s                                       | 40.77 s    |
| Umbrella `tests/it.rs` (+ `e2e.rs` split)  | 31.58 s                                       | 33.25 s    |
| Delta                                      | **−7.3 %**                                    | −18.4 %    |

The R18 chunk in the "Idiomatic Rust Cleanup" plan gates consolidation
on a ≥ 20 % cold-build improvement; both signals undershoot. The
dominant cost is compiling third-party deps (`clap`, `wasmtime`,
`ureq`, `assert_cmd`, …) plus the workspace crates, not linking the
per-test binaries themselves — the test binaries each link the same
`libspecify-*.rlib` set and `assert_cmd` only adds a thin shell.
Collapsing 14 → 2 link steps saves ~2.5 s of a ~32 s cold build.

**Risks of consolidating anyway.** Beyond the modest CI win, the
umbrella pattern would hurt local iteration: `cargo test --test
capability` against one file currently links one small binary, but
under the umbrella any `cargo test --test it` recompiles every other
`mod` in the umbrella. We would also lose nextest's per-binary process
isolation for tests that mutate ambient env vars (`GIT_AUTHOR_NAME`,
`REGENERATE_GOLDENS`, etc.).

**Status.** R18 measured and dropped. Future chunks that revisit
test-build time should target the dep graph (e.g. tightening feature
flags on `wasmtime`, replacing `ureq` with a stub at test time) before
re-litigating the umbrella shape.
