# Decisions

Standing architectural decisions for the `specify` CLI. Read before
changing error layering, exit codes, atomic writes, or the YAML library.

## Error layering

`specify-error` is the dependency leaf of the workspace. It depends only
on `thiserror` and `serde-saphyr`; every other `specify-*` crate may
depend on it, and it depends on none of them. Variants that need to
carry data from a downstream crate (e.g. `Error::Validation`) take a
small projection type defined in `specify-error` (`ValidationSummary`)
rather than re-exporting the rich domain type, so the leaf stays
cycle-free. The cost is a lossy projection at the boundary; callers that
need full fidelity reach for the downstream crate's own type directly.

## Exit codes

The binary commits to a four-slot exit-code table. `Exit::from(&Error)`
in `src/output.rs` is the single source of truth; every dispatcher routes
its error through it. `Exit::Code(u8)` is reserved for `specify tool
run` WASI passthrough.

| Code | Name                     | When                                                                                          |
|------|--------------------------|-----------------------------------------------------------------------------------------------|
| 0    | `EXIT_SUCCESS`           | Command succeeded.                                                                            |
| 1    | `EXIT_GENERIC_FAILURE`   | Any `Error` variant not listed below (I/O, YAML, schema, merge, tool resolver/runtime, ...). |
| 2    | `EXIT_VALIDATION_FAILED` | Validation findings, `Error::Validation`, `Error::Argument`, or a tool request rejected as undeclared. Also the workflow §D3/D4/D6 kebab discriminants `slice-authority-override-orphan-source-key`, `slice-fusion-drift`, and `discovery-alias-collision`, routed through `Error::validation_failed`. |
| 3    | `EXIT_VERSION_TOO_OLD`   | `project.yaml.specify_version` is newer than `CARGO_PKG_VERSION`.                             |

The Rust `Exit` enum carries five named variants (plus `Exit::Code(u8)`
for WASI tool passthrough) which collapse onto these four wire codes
via `Exit::from(&Error)`:

| Variant                  | Code |
|--------------------------|------|
| `Exit::Success`          | `0`  |
| `Exit::GenericFailure`   | `1`  |
| `Exit::ValidationFailed` | `2`  |
| `Exit::ArgumentError`    | `2`  |
| `Exit::VersionTooOld`    | `3`  |

`Exit::ArgumentError` and `Exit::ValidationFailed` share code `2` so the
wire contract stays four-slot; the named distinction exists for
dispatcher-side clarity (`Error::Argument` flags malformed CLI input
shape; `Error::Validation` carries a `ValidationSummary` payload). The
two never need separate exit codes — anything actionable by the
operator is in the JSON envelope's `code` discriminant.

## Atomic writes

Use `yaml_write` (in `crates/slice/src/atomic.rs`) for any file a
concurrent reader may observe mid-write: `plan.yaml`, `.metadata.yaml`,
`plan.lock`, and the registry. It serialises to
`NamedTempFile::new_in(parent)` and `persist`-renames over the target so
readers either see the prior bytes or the new bytes. Plain `fs::write`
is reserved for files no other process reads concurrently with the
writer (one-shot scratch output, fixtures inside a tempdir test).

## YAML library

The workspace uses `serde-saphyr` (pinned to a `0.0.x` release) for both
deserialization and serialization. It is pure-Rust, panic-free, and
actively maintained, in contrast to `serde_yaml` (deprecated) and
`serde_yaml_ng` (community fork carrying the same debt). Saphyr omits a
`Value` DOM, so code that needs untyped YAML access deserializes into
`serde_json::Value`. Its separate deser/ser error types ride directly on
`specify_error::Error::YamlDe` and `Error::YamlSer` (both
`#[error(transparent)]` `#[from]` variants), so `?` on a raw
`serde_saphyr` result still propagates and the kebab discriminant on
the wire stays `yaml` for either side; library crates that don't care
which API tripped match on either variant.

## Diag-first error policy

`Error::Diag { code, detail }` is the default for new diagnostics. A
typed `Error::*` variant exists only when (a) a test or skill
destructures the variant's payload, (b) the variant routes to a
non-default `Exit` slot, or (c) three or more call sites share the
exact shape. The kebab `code` is the wire contract; the Rust variant is
for callers that pattern-match. See AGENTS.md §"Errors" for the full
rule. The one-time collapse that produced the steady state — twelve
historical variants moving to `Diag` — is recorded in
[`docs/explanation/decision-log.md` §"Diag-first error policy — historical variant collapse"](./docs/explanation/decision-log.md#diag-first-error-policy--historical-variant-collapse).

## Hint colocation

Long-form recovery hints live on `Error::hint(&self) -> Option<&'static
str>`, not on the renderer. `ErrorBody::render_text` calls it. Adding a
new hint means extending `Error::hint`, not the renderer. Hints for
collapsed `Diag` codes are looked up by the kebab `code` so a `Diag`
site without a typed variant can still surface guidance.

## Wire compatibility

The CLI's JSON output is a flat envelope: every successful body is the
typed `*Body` rendered directly with `serde_json::to_writer_pretty`,
and every failure body is `ErrorBody` (with an optional `results` list
when the variant is `Error::Validation`). Skills grep on the
`error` / `code` discriminants; tests assert on them. There is no
top-level `envelope-version` integer — re-introduce one only if a
breaking shape change ships and consumers need a version stamp to
refuse output they cannot parse.

The kebab-case `code` discriminant on `Error::*` variants is the
public contract. Renaming or removing one is a breaking change.
Adding a new `Error::*` variant with a fresh kebab-case `code` is
additive (consumers see a new discriminant in the same shape).

CLI **input** flags are a peer wire surface — skill drivers shell out
through them. The same minor/major rules apply: adding a new optional
flag is additive, removing or renaming a flag is breaking. One
non-additive input change has shipped under the version reflected
above:

- `specify init` enforces the `<adapter>` xor `--hub` invariant
  through clap. The historical post-parse
  `init-requires-adapter-or-hub` envelope is gone on the CLI
  surface; clap parse errors exit `2` with the standard "required
  arguments were not provided" / "the argument cannot be used with"
  diagnostics. The discriminant survives in the domain library
  (`crates/domain/src/init/`) as defence-in-depth for embedders that
  call `init()` directly.

## Shell completions

`specify completions <shell>` writes a clap-generated completion script
to stdout for any shell `clap_complete::Shell` covers (`bash`,
`elvish`, `fish`, `powershell`, `zsh`). The script is a pure function
of the live clap surface, so verb additions/removals are auto-tracked
without extra plumbing.

## Crate layout

Four workspace crates: `specify-error` (leaf), `specify-domain`
(every domain module), `specify-tool` (WASI host, gated), and the
`specify` binary.

The Phase 1B collapse from 13 crates and the subsequent
`specify-validate` re-extraction (folded into the `wasi-tools/contract`
carve-out by the 2026-05 architecture-inversion pass) are recorded in
[`docs/explanation/decision-log.md` §"Crate layout — Phase 1B / `specify-validate` carve-out history"](./docs/explanation/decision-log.md#crate-layout--phase-1b--specify-validate-carve-out-history).

Rule: new functionality lands in an existing module by default. New
workspace crates require a paragraph in this file justifying why an
existing module cannot host the code, and what dependency-direction
invariant the new crate enforces (i.e. which leaf-→-root edge it
preserves, and which existing crate would have grown a cycle if the
code had gone there). A new crate that does not strengthen the
dependency direction is overhead; refactor within an existing module
instead. Adapter-specific logic never lands as a workspace crate
— it lands in the adapter's WASI carve-out.

## Tool architecture

`specify-tool` owns the declared WASI tool model, cache, resolver, and
Wasmtime-backed execution host. It is deliberately independent of
`specify-adapter`: the binary resolves adapters, then hands this
crate project-scope and adapter-scope tool declarations.

- **Declaration sites.** Tools are declared at *project scope* (a
  top-level `tools:` array in `.specify/project.yaml`) and / or
  *adapter scope* (a `tools.yaml` sidecar next to `adapter.yaml`
  inside the resolved adapter directory). Both shapes share
  `schemas/tool.schema.json`. `specify tool` merges by `name`, with
  project scope winning on collision and a typed `tool-name-collision`
  warning emitted once per session. `adapter.yaml` itself is never
  modified and never gains a `tools:` field.
- **Cache layout.** The cache root resolves
  `$SPECIFY_TOOLS_CACHE` → `$XDG_CACHE_HOME/specify/tools/` →
  `$HOME/.cache/specify/tools/`. Within it, paths are
  `<scope-segment>/<tool-name>/<version>/{module.wasm,meta.yaml}` where
  `<scope-segment>` is `project--<project-name>` or
  `adapter--<adapter-slug>`. The `--` separator avoids collisions
  with hyphenated tool names. `<version>` is the literal manifest
  string; SemVer is parsed only at structural validation time.
- **Sidecar metadata.** `meta.yaml` records
  `(scope, tool-name, tool-version, source, sha256)` plus an
  informational `permissions-snapshot`. A sidecar is a cache hit when
  that tuple matches the live merged manifest; any mismatch forces a
  refetch into the same `<version>/` directory via atomic move. When
  `sha256` is present, fetched bytes are verified before installation.
  Permissions changes alone never invalidate the cache (permissions are
  evaluated per `run`).
- **Permission substitution.** Substitutions apply only inside
  `permissions.{read,write}` entries (not `source`, not module argv).
  `$PROJECT_DIR` is always available; `$ADAPTER_DIR` is available
  only to adapter-scope tools — project-scope use is rejected as
  `tool.adapter-dir-out-of-scope`. After substitution paths must be
  absolute, free of `..`, and canonicalise inside `PROJECT_DIR`
  (or `ADAPTER_DIR` for adapter-scope). `write:` entries that
  target Specify lifecycle state (`.specify/project.yaml`, slice /
  archive `.metadata.yaml`, `.specify/plan.lock`, etc.) are rejected.
- **Argument forwarding and environment.** `specify tool run <name>
  [-- <args>...]` forwards everything after `--` verbatim with
  `<name>` as `argv[0]`. The module receives exactly two environment
  variables — `PROJECT_DIR` always, `ADAPTER_DIR` only for
  adapter-scope tools — plus stdio. No host environment is
  inherited. Working directory is the canonicalised project root.
- **Exit-code mapping.** Module exit `0` → `0`; module exit `N`
  (1..=255) → `N`; runtime trap → `2` with a typed `runtime` envelope;
  resolver error → `2` with a typed `resolver` envelope; missing
  project context → `1` (`not-initialized`); unknown tool name → `2`
  (`tool-not-declared`).
- **Wasmtime configuration.** Pin `wasmtime` and `wasmtime-wasi` to a
  matching stable pair, use the synchronous WASI Preview 2 path
  (`wasmtime_wasi::add_to_linker_sync`) and
  `wasmtime::component::Component`, and disable filesystem access by
  default — preopens are added per-tool from manifest permissions only.
  Execution stays behind the concrete `WasiRunner` boundary.
- **Cache concurrency.** No file locks in v1; concurrent cold-cache
  resolutions may both stage, and the resolver's atomic rename makes
  the steady state deterministic. A per-tool flock is deferred until
  it is needed.
- **`specify tool gc` scope.** Deletes any
  `<cache-root>/<scope-segment>/<tool-name>/<version>/` whose
  `(scope, name, version, source)` tuple is not referenced by the live
  merged manifest of the current project. It does not scan other
  projects on the host.
- **Registry resolution.** Wasm-pkg config is layered, last-write-wins:
  (1) wasm-pkg global defaults, (2) the project-local
  `.specify/wasm-pkg.toml` (when present), (3) the `WKG_CONFIG`
  override, (4) an embedded `specify -> augentic.io` namespace
  fallback applied only when no earlier layer mapped the `specify`
  namespace. `specify init` (regular and hub modes) scaffolds
  `.specify/wasm-pkg.toml` with the canonical RFC-17 mapping; the
  file is checked in and operators edit it to register internal
  mirrors. Re-init never overwrites an operator-edited file. The
  scaffold is the only first-party constant the binary still ships;
  the previous hardcoded GHCR prefix is gone — `meta.yaml`'s
  `oci.reference` is now derived best-effort from the resolved
  registry's well-known wasm-pkg metadata, and stays `None` when the
  registry advertises no OCI protocol or the metadata fetch fails.
- **Time crate.** UTC-only domain; `jiff::Timestamp` replaces
  `chrono::DateTime<Utc>` across every host crate. All persisted
  stamps route through `specify_error::serde_rfc3339` so the on-disk
  wire shape stays `%Y-%m-%dT%H:%M:%SZ` byte-for-byte across both the
  domain DTOs and `Sidecar.fetched_at`. `system_time_to_utc` consolidates
  the previous three `Error::Diag` codes (`merge-mtime-pre-epoch`,
  `merge-mtime-overflow`, `merge-mtime-out-of-range`) into a single
  `merge-mtime-out-of-range` whose `detail` carries the underlying
  `jiff` error.

## RFC-25 type rename: `Target*` is the output role, `Adapter` is the shared shape

The output-role domain types are spelled `Target*`
(`Target`, `Slice.target`, the `slice-create-target-missing` /
`plan.entry-needs-project-or-target` / `init-requires-adapter-or-hub`
discriminants, plus every fixture, JSON envelope, and call site). The
shared manifest *shape* is loaded by the axis-aware module
`crates/domain/src/adapter/` (`SourceAdapter` / `TargetAdapter` /
`Axis` / `ResolvedAdapter` / `AdapterLocation`). Briefs are resolved by
path through `briefs.<op>` on the adapter manifest; they carry no YAML
frontmatter and the CLI never reads their bodies. `CacheMeta` lives in
[`crates/domain/src/init/cache.rs`](./crates/domain/src/init/cache.rs);
the slice-metadata wire uses `Operation { Shape, Build, Merge }`
(`phase: shape | build | merge`).

Per workflow §"Note to the implementing agent", touching any of these
symbols requires a cross-repo `rg` sweep against `augentic/specify-cli`
and `augentic/specify` in the same PR.

The Wave 0.2 / 0.3 / F9 collapse history that produced this layout —
including the names of the retired axis-generic types and the prior
`init-requires-target-or-workspace` discriminant — is recorded in
[`docs/explanation/decision-log.md` §"RFC-25 type rename — Wave 0 / F9 collapse history"](./docs/explanation/decision-log.md#rfc-25-type-rename--wave-0--f9-collapse-history).

## Adapter loader axis routing

`specify_domain::adapter::Adapter::resolve(axis, name, project_dir)` is
the single entry point for loading a source or target adapter manifest.
Probe order is path-agnostic and matches workflow §"Resolver and cache"
verbatim:

1. `<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/` —
   agent-populated manifest cache, fetched by the plan/slice flow.
2. `<project_dir>/adapters/{sources,targets}/<name>/` — in-repo
   manifests checked into the project's source tree.

The axis segment (`sources` for `Axis::Source`, `targets` for
`Axis::Target`) keeps source and target adapters with colliding names
disambiguated by axis. Cache placement matches the probe layout —
`cache_dir(axis, name)` returns
`<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/`.
The sibling workflow §D8 extraction cache lives in a disjoint tree under
`<project_dir>/.specify/.cache/extractions/<adapter>/`; see §"Cache
layout". Refer to workflow §"Resolver and cache" before changing the
probe order or manifest-cache layout.

## Plan lifecycle: two stored states

`plan.yaml.lifecycle` is `pending | reviewed`. No other plan-level
states ship in v1; `in-progress` and `drained` were dropped from RFC-23
in Wave 1.2 (`cli/W1.2`). Per-entry status remains a closed enum of
`pending | in-progress | done` and the writer ownership is split:
`plan add` / `plan amend` write `pending`, `plan next` is the sole
writer of `in-progress`, and `slice merge` (via `plan transition <entry>
done` invoked by the `/spec:merge` skill body) writes `done`. "Drained"
is computed at read time as "every entry is `done`", not stored.
`specify plan transition <plan-name> reviewed` is Gate 1 and is
operator-only — the CLI does not gate it (the call is ungated so
operators can run it from any shell), but the `--help` text documents
the rule and `/spec:plan` skill bodies MUST NOT call it. Refer to
workflow §"Execution model" for the full state diagram.

Per-entry status walks backwards only via the dedicated
`specify plan transition <entry> --undo` verb. The verb refuses to
skip rungs — it implements exactly `Done → InProgress` and
`InProgress → Pending` per call, so undoing a `done` entry to
`pending` MUST run twice. Each step emits one
`plan.transition.undone` journal event carrying `{ plan-name,
slice-name, from, to }` so replay traces line up with the
forward-direction cadence (`plan.transition.reviewed`,
`slice.transition.*`). Plan-level lifecycle has no undo path in v1:
once stamped, `reviewed` only un-sets by hand-editing `plan.yaml`
(out of scope for the CLI) or by dropping and re-creating the plan.
`Status::Reopened` does not exist — an "undone" `done` row walks
back to `in-progress` so the operator can re-run `/spec:build` and
re-merge without inventing a new state. If an upstream revert
demands a redo without re-running the slice, author a fresh slice
that captures the redo work; the original slice's `done` row stays
as the historical record.

Archive is a filesystem operation, not a lifecycle state. `specify
plan finalize` moves `change.md` + `plan.yaml` into
`.specify/archive/<plan-name>/`, but the plan-level lifecycle stamp
inside the archived `plan.yaml` stays at `reviewed`.
There is no `archived` enum variant on `plan.yaml.lifecycle` — the
on-disk location of the file is the archived signal, not a stored
state.

## `SliceSourceBinding`: bare shorthand plus structured form

`plan.yaml.slices[].sources` is a single in-memory struct
(`{ key: String, candidate: Option<String> }`) with a custom
`Deserialize` impl that accepts two wire shapes and a custom
`Serialize` impl that emits whichever shape produced the value:

- **Bare string shorthand** — `legacy` parses to `key = "legacy"`,
  `candidate = None`; serialises back as the bare string. The candidate
  falls back to the owning slice's name at lookup time via
  `SliceSourceBinding::candidate(slice_name)`, preserving the
  one-source-per-slice degenerate case (predominantly `intent`).
- **Structured form** — `{ key: legacy, candidate: legacy-monolith }`
  parses to `key = "legacy"`, `candidate = Some("legacy-monolith")`;
  serialises back as the same `{ key, candidate }` map. Required
  whenever the key and the candidate id differ.

Collapsing the two variants into one struct means every consumer
(`validate`, `doctor`, `fusion`, CLI handlers) goes through the same
`key()` / `candidate()` accessors instead of `match`-ing the
discriminator — the shorthand stays a pure parser concern. Construct in
tests via `SliceSourceBinding::bare(key)` or
`SliceSourceBinding::structured(key, candidate)` so the discipline
stays consistent. `plan amend --add-source <key>` and `plan create`
share the same shorthand on the wire. Refer to workflow §`Slice.sources`.

## `Divergence` enum

`plan.yaml.slices[].divergence` is the closed enum
`none | likely | accepted | rejected` (kebab-case on the wire;
`snake_case` Rust variants joined by `#[serde(rename = "…")]`). `none`
is the implicit default and is elided from serialised output.
`specify plan amend --divergence` only accepts `accepted | rejected`
from the wire — `none` is the absent default, and `likely` is reserved
for the `propose` sub-step of `/spec:plan`, which writes the value via
a direct YAML edit (per the W3.2 hand-off). Operators flipping the
field after Gate 1 review use `accepted | rejected` exclusively.
Refer to workflow §"Plan-time fusion".

## workflow §D2 — per-kind authority on Evidence

`evidence.schema.json` gains an
optional `authority-overrides` map keyed by claim kind, valued by
authority class. The document-level `authority:` field stays
required; the override applies to all claims of the named kind in
that Evidence document. Per-claim overrides remain explicitly
deferred. Synthesis consults the per-kind override first, then the
document-level `authority:`, then the workflow default ordering — a
byte-stable three-step fallback chain.

## workflow §D3 — per-slice authority on `plan.yaml`

`plan.yaml.slices[]` gains an
optional `authority-override` map keyed by claim kind, valued by
source key. Keys come from the closed claim-kind enum; values MUST
be source keys present in the slice's own `sources[]` list. Orphan
keys are rejected by `specify slice validate` with the
`slice-authority-override-orphan-source-key` kebab discriminant. The
map is scoped to one slice — plan-wide and project-wide overrides
are out of scope.

## workflow §D4 — `fusion.yaml` is audit-only

`schemas/slice/fusion.schema.json`
fixes the closed top-level shape (`version`, `slice`,
`generated-at`, `generator`, `requirements[]`). `/spec:refine`
writes the file atomically; downstream verbs read `spec.md` as the
authoritative artifact and treat `fusion.yaml` as an inspection
surface. `specify slice validate` enforces id-set parity between
`spec.md` `REQ-*` ids and `fusion.yaml.requirements[].id` and
catches contributing-claim → Evidence-claim drift, both via the
`slice-fusion-drift` discriminant.

## workflow §D8 — cache fingerprint inputs

The closed list of fingerprint
inputs (`source path canonicalised | adapter name@version | brief
sha256 | sorted declared-tool versions | candidate id`) lives on
[`crate::adapter::cache::CacheFingerprint`]. CI that pins the four inputs
common across runs can re-run any prior `/spec:execute` and expect
byte-stable cache hits; CI observing any of the five
`slice.extract.cache-miss` reasons knows exactly which input
drifted. Adapter authors opt out with `cache: opt-out` on
`adapter.yaml`; the matching journal event carries `reason:
adapter-opt-out`.

## Journal event names

`crates/domain/src/journal.rs` emits a closed taxonomy of RFC-19
events. The wire ids are dotted kebab-case; the Rust `EventKind`
variants are `snake_case` and bridge to the wire via
`#[serde(rename = "…")]`. The taxonomy added in Wave 1.4
(`cli/W1.4`) is:

| Wire id | Emitted by |
|---|---|
| `plan.transition.reviewed` | `specify plan transition <plan> reviewed` (Gate 1 stamp). |
| `plan.transition.undone` | `specify plan transition <entry> --undo` (per-entry reverse rung; one event per rung). |
| `plan.propose.divergence` | `/spec:plan` `propose` sub-step when it flips a slice to `divergence: likely`. |
| `plan.amend.divergence` | `specify plan amend --divergence accepted\|rejected` on any transition into or out of `accepted`/`rejected`. |
| `slice.transition.refined` | `specify slice transition <slice> refined`. |
| `slice.extract.completed` | The `/spec:refine` skill, after the serial `extract` loop closes. |
| `slice.synthesis.conflict` / `.divergence` / `.unknown` | `specify slice validate`, one per requirement-block tag emitted by the synthesis substep. |
| `slice.extract.cache-hit` / `.cache-miss` | The extract code path; payloads carry the fingerprint sha256 (and the closed `reason` enum on misses). workflow §D8. |
| `slice.fusion.written` | `/spec:refine`'s atomic `fusion.yaml` writer (Change 2.6). workflow §D4. |
| `slice.replay.completed` | Target adapter's `build` step when it consumes runtime captures; optional in v1. workflow §D1. |
| `plan.amend.authority-override` | `specify plan create --authority-override`, `specify plan amend --authority-override` / `--clear-authority-override` / `--clear-authority-overrides`. workflow §D3. |

Events persist as newline-delimited JSON at
`<project_dir>/.specify/journal.jsonl`. The closed `from` / `to`
enum on the divergence events is
`none | likely | accepted | rejected`. Refer to workflow §"Observability"
and the per-event row table.

## `$CAPABILITY_DIR` replaces `$ADAPTER_DIR`

The WASI tool runner's plugin-scope substitution variable is
`$CAPABILITY_DIR`. It expands to the resolved plugin's root directory
(`<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/`
or the in-repo equivalent) and is only valid in
`permissions.{read,write}` entries (and the `source:` URI of a
plugin-scope tool); project-scope
references are rejected as `tool.capability-dir-out-of-scope` /
`tool.source-capability-dir-out-of-scope`. The tool cache scope
segment that pairs with it is `plugin--<axis>--<slug>` — e.g.
`plugin--target--contracts` for the `contracts` target adapter's
tools. Project-scope tools keep `project--<project-name>`
unchanged. Refer to workflow §"Sandboxing".

## Lifecycle write-ownership

Per-entry status writes route to exactly one CLI verb. Skill bodies
never write status by hand; the CLI is the single source of truth for
each transition:

| State | Writer | Trigger |
|---|---|---|
| `pending` (per-entry) | `specify plan add` / `specify plan amend` | Operator (or `/spec:plan`) authors / edits a slice row. |
| `in-progress` (per-entry) | `specify plan next` | Sole writer; the `/spec:execute` loop calls it once per slice. |
| `done` (per-entry) | `specify plan transition <entry> done` | Called by `/spec:merge` after `specify slice merge` succeeds. |
| `pending` (plan-level) | `specify plan create` | `/spec:plan` scaffolds the plan in `pending`. |
| `reviewed` (plan-level) | `specify plan transition <plan> reviewed` | Operator-only (Gate 1). The CLI is ungated; `/spec:plan` MUST NOT call this verb — `--help` text documents the rule and the skill body is the actual gate. |

The plan-level `reviewed` row is the lightest-touch shape the RFC
allows: a wholly operator-driven stamp with no CLI-side authentication.
Skills that drift from this contract get caught at review time. Refer
to workflow §"CLI surface" and §"Writer ownership".

## Plan source bindings

The on-disk shape of `plan.yaml.sources.<key>` is the structured
`{ adapter, path?, value? }` object — the 1.x bare-string shorthand
was dropped at the Specify 2.0 cut and the `oneOf` branch that
documented it is gone from `schemas/plan/plan.schema.json`. Every
binding now carries an explicit kebab-case `adapter` and exactly one
of `path` (filesystem path or repo location) or `value` (literal
payload supplied directly to the adapter — used by `intent`). The
`oneOf [path, value]` exclusion is enforced in both the JSON Schema
and the Rust loader (`specify_domain::change::SourceBinding`).

The `specify plan create --source` flag grammar mirrors the wire
shape:

| Form | Materialises as |
|---|---|
| `--source <key>=<adapter>:<path>` | `SourceBinding { adapter, path: Some(<path>), value: None }` |
| `--source <key>=<adapter>:value:<literal>` | `SourceBinding { adapter, path: None, value: Some(<literal>) }` |

The adapter is the substring up to the first `:` after `=`; the
binding payload is everything after that first `:`. URLs that
contain `:` (e.g. `git@github.com:org/foo.git`) round-trip through
the path form unchanged. The `value:` sentinel switches the parser
to literal mode, so the literal payload may contain any character
(including `:`, `=`, and newlines) without further escaping. No
shorthand exists for "the adapter name equals the key"; every flag
invocation carries both. Refer to workflow §Source and
`crates/domain/src/change/plan/core/model.rs::SourceBinding`.

Source keys are plan-scoped; each key maps to exactly one binding
under `Plan::sources`, but slices may reference the same key with
different candidates.

## Adapter manifest requireds

`description` is required at the top level of every adapter manifest —
sources and targets alike — alongside the existing `name`, `version`,
`axis`, and `briefs`. `tools[].version` is required for every declared
tool. The accepted shape is semver only: `x.y.z` with an optional
`-prerelease` suffix, locked by the schema pattern
`^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$`. No `v` prefix, no `sha256:` digest,
no free-form strings. Tools without a release must cut one before being
declared. The reproducibility argument is the workflow §D8 cache
fingerprint: it folds `sorted declared-tool versions` into the
extraction cache key, so an absent or non-semver pin would silently
drop tool-version from the fingerprint and let two adapter revisions
share a cache slot. Enforced uniformly by `adapter.schema.json`,
`source.schema.json`, and `target.schema.json`.

## Adapter name uniqueness

Adapter names are unique across axes — a name is declared under
`adapters/sources/<name>/` xor `adapters/targets/<name>/`, never both
(and the same applies to their
`.specify/.cache/manifests/{sources,targets}/<name>/` manifest-cache
mirrors). Eagerly enforced at `specify init` time (inside
`crates/domain/src/init/cache.rs::cache_adapter`, before the target
cache directory is rewritten) and at `*Adapter::resolve` time. The
resolve-time probe is process-memoised per `(project_dir, axis, name)`
in `crates/domain/src/adapter/core.rs::check_axis_unique_for_name_memo`,
so a re-resolve of the same adapter in the same session avoids
re-walking `adapters/{sources,targets}/` and the matching cache
mirrors — operators see the diagnostic on first reach but pay no FS
stat cost on every subsequent resolve. The public
`check_axis_unique_for_name(axis, name, project_dir)` helper is the
one-sided variant `init` calls before the side it is about to
install exists on disk; it is unmemoised because each `init` is the
once-per-install boundary. Collisions surface as `Error::Validation`
with the kebab-case discriminant `adapter-name-axis-collision`; the
wire body names both the axis the loader was asked for and the
colliding sibling axis so operators can rename or delete one side
without grepping the manifest tree.

## Cache layout

`.specify/.cache/` hosts two distinct, root-disjoint caches:

- `manifests/{sources,targets}/<name>/` — adapter manifest cache. The
  agent-populated mirror of `adapters/{sources,targets}/<name>/`
  (`adapter.yaml` plus the brief markdown files it references). Per-axis
  because adapter names are unique per axis. Resolved by
  `crates/domain/src/adapter/core.rs::cache_dir`.
- `extractions/<adapter>/<fingerprint>/` — workflow §D8 per-source
  extraction result cache, with the append-only `index.jsonl` at the
  adapter root (`extractions/<adapter>/index.jsonl`). Per-adapter only —
  not per-axis — because extraction is a source-axis operation; the
  adapter name carries enough identity. Resolved by
  `crates/domain/src/adapter/cache/io.rs::CacheLayout`.

Each cache owns its own root, so the loader no longer probes for an
`adapter.yaml` inside the cache directory to disambiguate manifest vs.
extraction co-tenancy — a manifest-cache directory is always a manifest
mirror, and the extraction tree never carries `adapter.yaml` at any
level. Refer to workflow §D8 for the extraction-cache fingerprint
contract.

## Target adapter suffix policy

`plan.yaml.slices[].target` carries the `name@vN` form (e.g.
`omnia@v1`) and the integer `N` is a load-bearing wire field, not
decorative metadata:

- `schemas/plan/plan.schema.json` pins the wire shape with the regex
  `^[a-z][a-z0-9-]*@v\d+$`; bare names and non-kebab variants are
  rejected at schema-validation time.
- `crates/domain/src/change/plan/core/model.rs::TargetRef` is the
  parsed in-memory representation. Serde routes
  `Option<TargetRef>` through `TargetRef::parse`, so any value that
  reaches the validator already carries a typed `(name, version)`
  pair.
- The cross-field "at least one of `project` / `target`" rule lives
  inside the schema as a per-slice `anyOf`, so external consumers
  (Cursor IDE renderers, CI linters) get the same gate as the Rust
  loader without having to mirror the Rust-only
  `plan.entry-needs-project-or-target` finding.
- `plan-target-malformed` is the discriminant reserved for the
  CLI-flag parser (`--target <raw>`); the schema regex prevents it
  from being reachable through on-disk YAML.

## Operations typed at parse boundary

Adapter operations are typed Rust enums by the time YAML parsing
finishes; string operation names never survive past the manifest loader.

- Task A (review 1.A1) removed the decorative `operations:` array from
  every `adapter.yaml`, `schemas/adapter.schema.json`,
  `schemas/source.schema.json`, and `schemas/target.schema.json`.
  `briefs.keys()` is the canonical iterator over an adapter's declared
  operations; `Adapter::operations()` derives from it if a caller needs
  the typed iterator.
- Task E (review 1.B1) split the legacy axis-generic `Adapter` struct
  into `SourceAdapter` and `TargetAdapter` with
  `briefs: BTreeMap<SourceOperation, String>` and
  `BTreeMap<TargetOperation, String>` respectively. The closed
  `{Source,Target}Operation` enums in
  `crates/domain/src/adapter/operation.rs` are the typed `briefs.keys()`
  carried by each manifest struct; manifest brief maps are enum-keyed
  and string literals at call sites are gone.
- **Wire invariant.** The `specify source resolve` and
  `specify target resolve` JSON envelopes' `operations: [...]` arrays
  iterate in kebab-alphabetical order (e.g. `["enumerate", "extract"]`,
  `["build", "merge", "shape"]`). `BTreeMap` ordering combined with
  manual `Ord` / `PartialOrd` impls on `{Source,Target}Operation`
  (sorting by kebab string, not by Rust variant declaration order)
  preserves this contract end-to-end. Future refactors must not
  re-derive `Ord` on these enums without preserving the kebab-string
  sort — derived `Ord` follows declaration order and would silently
  break the wire.
