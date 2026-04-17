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
(`thiserror` + `serde_yaml` only) — while still giving the CLI enough
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

## Change H — Federation stub layering

**Decision.** `specify-federation::parse_federation_config` takes a generic
`Cfg: FederationConfig` parameter rather than a concrete `&ProjectConfig`.
The `FederationConfig` trait is declared (empty) in `specify-federation`;
`ProjectConfig` (defined in the root `specify` crate in Change I) will add
a zero-method `impl FederationConfig for ProjectConfig {}`. RFC-3 extends
the trait with real methods (`fn peers(&self) -> &[…];` and similar).

**Rationale.** RFC-1 §`federation.rs`
([rfcs/rfc-1-cli.md](rfcs/rfc-1-cli.md) line 898) shows
`parse_federation_config(config: &ProjectConfig)`. Implementing that
signature verbatim would require `specify-federation` to depend on the
root `specify` crate — but the root crate depends on `specify-federation`
(it re-exports the public API per
[RFC-1 plan line 202](rfcs/rfc-1-plan.md#change-h--stubs-specify-drift-specify-federation)),
producing a dependency cycle. Moving `ProjectConfig` down into a leaf
config crate was considered and rejected: Change I deliberately keeps
config + init + CLI plumbing in the root crate so the binary has a single
assembly point, and splitting it would add a fourth "plumbing" crate for
no payoff.

The trait-in-the-leaf-crate approach keeps `specify-federation` dependency-
free from the root crate while freezing the call-site signature today, so
Change I and every subsequent Change can wire through
`parse_federation_config(&config)` without a later refactor. The empty
trait costs nothing at the type level — it's a pure capability marker
until RFC-3 fills it in.

## Change I — CLI exit codes and version-floor semantics

**Decision.** The `specify` binary commits to a four-slot exit-code table
and centralises the `specify_version` floor check inside
`ProjectConfig::load`:

| Code | Name                      | When                                                                 |
|------|---------------------------|----------------------------------------------------------------------|
| 0    | `EXIT_SUCCESS`            | Command succeeded.                                                   |
| 1    | `EXIT_GENERIC_FAILURE`    | Any `Error` variant not listed below (I/O, YAML, schema, merge, …). |
| 2    | `EXIT_VALIDATION_FAILED`  | `specify validate` returned a report whose `passed` flag is `false` (Change J wires this), or `Error::Validation { .. }` bubbles up. |
| 3    | `EXIT_VERSION_TOO_OLD`    | `.specify/project.yaml.specify_version` is newer than `CARGO_PKG_VERSION` — surfaced as `Error::SpecifyVersionTooOld`. |

`main.rs::exit_code_for(&Error)` is the single source of truth for the
mapping; every subcommand dispatcher routes its error through it so the
table stays honest regardless of which crate raised the error. The
constants (`EXIT_SUCCESS`, `EXIT_GENERIC_FAILURE`,
`EXIT_VALIDATION_FAILED`, `EXIT_VERSION_TOO_OLD`) live at the top of
`src/main.rs` alongside a module-level doc comment that reproduces the
table verbatim for skill authors.

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

