# Decisions

Standing architectural decisions for the `specify` CLI. Read before
changing error layering, exit codes, atomic writes, or the YAML library.

## Error layering

`specify-error` is the dependency leaf of the workspace. It depends only
on `thiserror` and `serde-saphyr`; every other `specify-*` crate may
depend on it, and it depends on none of them. Variants that need to
carry data from a downstream crate (e.g. `Error::Validation`) take a
small projection type defined in `specify-error` (`ValidationResultSummary`)
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
| 2    | `EXIT_VALIDATION_FAILED` | Validation findings, `Error::Validation`, or a tool request rejected as undeclared.           |
| 3    | `EXIT_VERSION_TOO_OLD`   | `project.yaml.specify_version` is newer than `CARGO_PKG_VERSION`.                             |

## Atomic writes

Use `yaml_write` (in `crates/slice/src/atomic.rs`) for any file a
concurrent reader may observe mid-write: `plan.yaml`, `.metadata.yaml`,
`journal.yaml`, `plan.lock`, and the registry. It serialises to
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
rule. Twelve historical one-site variants
(`RegistryMissing`, `PlanNotFound`, `PlanStructural`,
`CompatibilityCheckFailed`, `ContextDriftDetected`,
`ContextWouldUpdate`, `ContextNoLock`, `ContextMissing`,
`ContextUnfenced`, `ContextDrift`, `InitNeedsAdapter`,
`WorkspacePushFailed`) collapsed to `Diag` under this policy with their
kebab discriminants preserved.

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
flag is additive, removing or renaming a flag is breaking. Two
non-additive input changes have shipped under the version reflected
above:

- `slice outcome set <slice> <phase> registry-amendment-required`
  takes a single `--proposal '<json>'` instead of seven
  `--proposed-*` flags. Skills build the proposal as a JSON object
  (`{"proposed-name": ..., "proposed-url": ..., "proposed-adapter":
  ..., "proposed-description": ..., "rationale": ...}`) and pass it
  verbatim. The on-disk `outcome.outcome.registry-amendment-required.*`
  shape and the `outcome.proposal` JSON returned by `slice outcome
  show` are unchanged.
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

History: until Phase 1B of the 2026-05 cleanup the workspace had 13
crates; the fragmentation cost more than it earned (wide build graph,
redundant `Cargo.toml` files, indirect re-export hops, repeated
duplicate-version exemptions). Module boundaries inside
`specify-domain` preserve the original separation; `pub` cross-module
surfaces match the prior cross-crate `pub use` exports.

`specify-validate` was a Phase 1B re-extraction that owned the
baseline-contract validation primitives (`ContractFinding`,
`validate_baseline`) and was shared between `specify-domain` (for
compatibility classification) and the `wasi-tools/contract` carve-out.
The 2026-05 architecture-inversion pass collapsed it into the
carve-out: a adapter's validation logic belongs inside its WASI
tool, not as a `specify-*` workspace crate the host can link
against. The host's `compatibility::classify_project` no longer
short-circuits on contract baseline failures — operators run
`specify tool run contract -- "$PWD/contracts"` as a pre-flight when
they need that gate, identical to every other adapter. The
carve-out is now self-contained; `wasi-tools/Cargo.toml` no longer
has a path bridge into the host workspace.

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

## RFC-25 type rename: `Target*` is the output role, `Plugin` is the shared shape

Wave 0.2 (`cli/W0.2`) renamed `Adapter*` → `Target*` for the output-role
domain types (`Target`, the `Slice.target` field, the
`init-requires-target-or-workspace` / `slice-create-target-missing` /
`plan.entry-needs-project-or-target` error discriminants, plus every
fixture, JSON envelope, and call site). Wave 0.3 (`cli/W0.3`) moved the
shared manifest *shape* into the new `crates/domain/src/plugin/`
loader so source and target adapters share one loader keyed by an
explicit `axis: source | target`. The legacy `crates/domain/src/adapter/`
module survives as a narrower home for `Brief`, `ChangeBrief`,
`CodexProvenance`, `CacheMeta`, and `PipelineView` — concepts that are
not part of the RFC-25 wire contract — but no new code should load a
manifest through it. Per RFC-25 §"Note to the implementing agent",
touching any of these symbols requires a cross-repo `rg` sweep against
`augentic/specify-cli` and `augentic/specify` in the same PR.

## Plugin loader axis routing

`specify_domain::plugin::Plugin::resolve(axis, name, project_dir)` is
the single entry point for loading a source or target adapter manifest.
Probe order is path-agnostic and matches RFC-25 §"Resolver and cache"
verbatim:

1. `<project_dir>/.specify/.cache/{sources,targets}/<name>/` —
   agent-populated cache, fetched by the plan/slice flow.
2. `<project_dir>/{sources,targets}/<name>/` — in-repo manifests
   checked into the project's source tree.

The axis segment (`sources` for `Axis::Source`, `targets` for
`Axis::Target`) keeps source and target adapters with colliding names
disambiguated by axis. Cache placement matches the probe layout —
`cache_dir(axis, name)` returns
`<project_dir>/.specify/.cache/{sources,targets}/<name>/`. Refer to
RFC-25 §"Resolver and cache" before changing the probe order or cache
layout.

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
RFC-25 §"Execution model" for the full state diagram.

## `SliceSourceBinding`: bare shorthand plus structured form

`plan.yaml.slices[].sources` accepts two shapes on parse:

- **Bare string shorthand** — `legacy` resolves to a binding whose
  `key` is `legacy` and whose `candidate` is the matching
  `plan.yaml.sources[].name` (one-to-one between source and slice).
- **Structured form** — `{ key: legacy, candidate: legacy-monolith }`
  is the canonical wire shape and is required whenever the key and
  the candidate id differ.

The CLI always serialises bindings in the structured form; the
shorthand parser exists so `/spec:plan` and operator hand edits stay
ergonomic. `plan amend --add-source <key>` accepts the same shorthand
on the wire. Refer to RFC-25 §`Slice.sources`.

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
Refer to RFC-25 §"Plan-time fusion".

## Journal event names

`crates/domain/src/journal.rs` emits a closed taxonomy of RFC-19
events. The wire ids are dotted kebab-case; the Rust `EventKind`
variants are `snake_case` and bridge to the wire via
`#[serde(rename = "…")]`. The taxonomy added in Wave 1.4
(`cli/W1.4`) is:

| Wire id | Emitted by |
|---|---|
| `plan.transition.reviewed` | `specify plan transition <plan> reviewed` (Gate 1 stamp). |
| `plan.propose.divergence` | `/spec:plan` `propose` sub-step when it flips a slice to `divergence: likely`. |
| `plan.amend.divergence` | `specify plan amend --divergence accepted\|rejected` on any transition into or out of `accepted`/`rejected`. |
| `slice.transition.refined` | `specify slice transition <slice> refined`. |
| `slice.extract.completed` | The `/spec:refine` skill, after the serial `extract` loop closes. |
| `slice.synthesis.conflict` / `.divergence` / `.unknown` | `specify slice validate`, one per requirement-block tag emitted by the synthesis substep. |

Events persist as newline-delimited JSON at
`<project_dir>/.specify/journal.jsonl`. The closed `from` / `to`
enum on the divergence events is
`none | likely | accepted | rejected`. Refer to RFC-25 §"Observability"
and the per-event row table.

## `$CAPABILITY_DIR` replaces `$ADAPTER_DIR`

The WASI tool runner's plugin-scope substitution variable is
`$CAPABILITY_DIR`. It expands to the resolved plugin's root directory
(`<project_dir>/.specify/.cache/{sources,targets}/<name>/` or the
in-repo equivalent) and is only valid in `permissions.{read,write}`
entries (and the `source:` URI of a plugin-scope tool); project-scope
references are rejected as `tool.capability-dir-out-of-scope` /
`tool.source-capability-dir-out-of-scope`. The tool cache scope
segment that pairs with it is `plugin--<axis>--<slug>` — e.g.
`plugin--target--contracts` for the `contracts` target adapter's
tools. Project-scope tools keep `project--<project-name>`
unchanged. Refer to RFC-25 §"Sandboxing".

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
to RFC-25 §"CLI surface" and §"Writer ownership".
