# Decisions

Standing architectural decisions for the `specify` CLI. Read before
changing error layering, exit codes, atomic writes, or the YAML library.

## Error layering

`specify-error` is the dependency leaf of the workspace. It depends only
on `thiserror` and `serde-saphyr`; every other `specify-*` crate may
depend on it, and it depends on none of them. The leaf stays free of
rich domain payloads: `Error::Validation { code, detail }` is
payload-free (see [§"Drained `Error::Validation` and the `Diagnostic`
substrate"](#drained-errorvalidation-and-the-diagnostic-substrate)) — the
top-level wire `error` is the carried `code` discriminant, and any
rendered findings travel on stdout as a `DiagnosticReport`, not inside
the error. Earlier revisions carried a `ValidationSummary` projection
type here; it was removed when the diagnostic substrate moved to its own
`specify-diagnostics` leaf.

## Exit codes

The binary commits to a four-slot exit-code table. `Exit::from(&Error)`
in `src/runtime/output.rs` is the single source of truth; every dispatcher routes
its error through it. `Exit::Code(u8)` is reserved for `specrun tool
run` WASI passthrough.

| Code | Name                     | When                                                                                          |
|------|--------------------------|-----------------------------------------------------------------------------------------------|
| 0    | `EXIT_SUCCESS`           | Command succeeded.                                                                            |
| 1    | `EXIT_GENERIC_FAILURE`   | Any `Error` variant not listed below (I/O, YAML, schema, merge, tool resolver/runtime, ...). |
| 2    | `EXIT_VALIDATION_FAILED` | Validation findings, `Error::Validation`, `Error::Argument`, or a tool request rejected as undeclared. Also the authority, provenance, and discovery kebab discriminants `slice-authority-override-orphan-source-key`, `slice-provenance-drift`, and `discovery-alias-collision`, routed through `Error::validation_failed`. |
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
shape; `Error::Validation` is the payload-free gate-failure signal whose
`code` is the specific discriminant). The two never need separate exit
codes — anything actionable by the operator is in the JSON envelope's
`code` discriminant, and any per-finding detail is on the stdout
`DiagnosticReport`.

`specrun lint run` is the one finding-driven exit slot in the table.
Its decision is **status-aware severity**: it returns `2` only when a
finding has `status: open` AND `severity ∈ {critical, important}`.
Findings with `status: ignored` or `status: false-positive` remain in
every formatter and in the JSON envelope, but they do not contribute
to the blocking decision. The full lint status / disposition contract
is captured in [§"Lint finding status, disposition, and exit"](#lint-finding-status-disposition-and-exit).

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

- `specrun init` enforces the `<adapter>` xor `--hub` invariant
  through clap. The historical post-parse
  `init-requires-adapter-or-hub` envelope is gone on the CLI
  surface; clap parse errors exit `2` with the standard "required
  arguments were not provided" / "the argument cannot be used with"
  diagnostics. The discriminant survives in the domain library
  (`crates/workflow/src/init/`) as defence-in-depth for embedders that
  call `init()` directly.

## Shell completions

`specrun completions <shell>` writes a clap-generated completion script
to stdout for any shell `clap_complete::Shell` covers (`bash`,
`elvish`, `fish`, `powershell`, `zsh`). The script is a pure function
of the live clap surface, so verb additions/removals are auto-tracked
without extra plumbing.

## Crate layout

Workspace crates: `specify-error` (leaf), `specify-digest` (leaf —
SHA-256 hex digest encoding), `specify-schema`
(embedded JSON Schemas), `specify-model` (artifact types and parsers,
plus the shared atomic writer), `specify-validate` (artifact validation
rule registry), `specify-standards` (standards layer, which also hosts
the framework authoring checks behind `specdev lint` in its `framework`
module), `specify-workflow` (workflow lifecycle authority),
`specify-tool` (WASI host, gated), and the root binary package.

`specify-digest` exists so siblings such as `specify-standards` can share
digest encoding without depending on `specify-tool` (and therefore
Wasmtime). It depends on no other workspace crate — only `sha2` and
`base16ct`. `specify-tool` re-exports the hash helpers for backward
compatibility; new call sites should import `specify_digest` directly.

`specify-model` and `specify-validate` are the two crates that earn a
new-crate paragraph under the rule below. `specify-model` exists so the
artifact types and parsers (`spec`, `task`, `evidence`, `discovery`)
sit on a lifecycle-free leaf: it depends only on `specify-error` and
preserves the `specify-error -> specify-model` edge. `specify-validate`
holds the artifact rule registry and depends on `specify-model` only —
never on `specify-workflow`. That dependency direction is the whole
point: a validation rule physically cannot reach a slice transition or
plan stamp, the same no-lifecycle-authority invariant `specify-standards`
already enforces. `specify-workflow` depends on `specify-model` but
**not** on `specify-validate` (only the root binary orchestrates
validation), so no cycle forms.

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

The framework authoring checks behind `specdev lint` (originally the
plugin repo's retired `tooling/` crate, then the publish-disabled
`specify-authoring` crate) were dissolved into the `specify_standards::framework`
module. The imperative `Check` predicates are retained as-is; only their
output type was unified — every predicate now emits the canonical
`Diagnostic` directly (via the `framework::builder` `framework_finding()`
/ `loc()` helpers and the `CORE_ID_TABLE`), and `framework::check::run`
runs the single finalize pass (rebase locations → fingerprint → assign
sequential `FIND-NNNN` ids). The lightweight `Finding` / `Location` types
and the binary-boundary `map_finding.rs` mapper are gone. `specdev`'s
`AuthoringProducer` stays as the lone `DiagnosticProducer` bridge because
the predicates need `&Context` (framework root + schema cache), which the
`DiagnosticProducer::produce(&WorkspaceModel, project_dir)` signature does
not carry. The dissolution does not change crates.io exposure: the root
`specify` crate (which builds both `specrun` and `specdev`) already pulled
the predicates into the published binary's dependency graph. The declarative
burn-down deletes this imperative code incrementally as each predicate
migrates to a `CORE-NNN` rule file.

## Tool architecture

`specify-tool` owns the declared WASI tool model, cache, resolver, and
Wasmtime-backed execution host. It is deliberately independent of
`specify-adapter`: the binary resolves adapters, then hands this
crate project-scope and adapter-scope tool declarations.

- **Declaration sites.** Tools are declared at *project scope* (a
  top-level `tools:` array in `.specify/project.yaml`) and / or
  *adapter scope* (a `tools.yaml` sidecar next to `adapter.yaml`
  inside the resolved adapter directory). Both shapes share
  `schemas/tool.schema.json`. `specrun tool` merges by `name`, with
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
- **Argument forwarding and environment.** `specrun tool run <name>
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
- **`specrun tool gc` scope.** Deletes any
  `<cache-root>/<scope-segment>/<tool-name>/<version>/` whose
  `(scope, name, version, source)` tuple is not referenced by the live
  merged manifest of the current project. It does not scan other
  projects on the host.
- **Registry resolution.** Wasm-pkg config is layered, last-write-wins:
  (1) wasm-pkg global defaults, (2) the project-local
  `.specify/wasm-pkg.toml` (when present), (3) the `WKG_CONFIG`
  override, (4) an embedded `specify -> augentic.io` namespace
  fallback applied only when no earlier layer mapped the `specify`
  namespace. `specrun init` (regular and hub modes) scaffolds
  `.specify/wasm-pkg.toml` with the canonical wasm-pkg namespace mapping; the
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

## Source and target adapter role names

The output-role domain types are spelled `Target*`
(`Target`, `Slice.target`, the `slice-create-target-missing` /
`plan.entry-needs-project-or-target` / `init-requires-adapter-or-hub`
discriminants, plus every fixture, JSON envelope, and call site). The
shared manifest *shape* is loaded by the axis-aware module
`crates/workflow/src/adapter/` (`SourceAdapter` / `TargetAdapter` /
`Axis` / `ResolvedAdapter` / `AdapterLocation`). Briefs are resolved by
path through `briefs.<op>` on the adapter manifest; they carry no YAML
frontmatter and the CLI never reads their bodies. `CacheMeta` lives in
[`crates/workflow/src/init/cache.rs`](./crates/workflow/src/init/cache.rs);
the slice-metadata wire uses `Operation { Shape, Build, Merge }`
(`phase: shape | build | merge`).

Per workflow §"Note to the implementing agent", touching any of these
symbols requires a cross-repo `rg` sweep against `augentic/specify-cli`
and `augentic/specify` in the same PR.

The Wave 0.2 / 0.3 / F9 collapse history that produced this layout —
including the names of the retired axis-generic types and the prior
`init-requires-target-or-workspace` discriminant — is recorded in
[`docs/explanation/decision-log.md` §"Source and target adapter role names — Wave 0 / F9 collapse history"](./docs/explanation/decision-log.md#source-and-target-adapter-role-names--wave-0--f9-collapse-history).

## Adapter loader axis routing

`specify_workflow::adapter::Adapter::resolve(axis, name, project_dir)` is
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
The sibling extraction cache lives in a disjoint tree under
`<project_dir>/.specify/.cache/extractions/<adapter>/`; see §"Cache
layout". Refer to workflow §"Resolver and cache" before changing the
probe order or manifest-cache layout.

## Plan lifecycle: two stored states

`plan.yaml.lifecycle` is `pending | approved`. No other plan-level
states ship in v1; `in-progress` and `drained` were dropped during
the plan lifecycle simplification in Wave 1.2 (`cli/W1.2`). Per-entry status remains a closed enum of
`pending | in-progress | done` and the writer ownership is split:
`plan add` / `plan amend` write `pending`, `plan next` is the sole
writer of `in-progress`, and `slice merge` (via `plan transition <entry>
done` invoked by the `/spec:merge` skill body) writes `done`. "Drained"
is computed at read time as "every entry is `done`", not stored.
`specrun plan transition <plan-name> approved` is Gate 1 and is
operator-only — the CLI does not gate it (the call is ungated so
operators can run it from any shell), but the `--help` text documents
the rule and `/spec:plan` skill bodies MUST NOT call it. Refer to
workflow §"Execution model" for the full state diagram.

Per-entry status walks backwards only via the dedicated
`specrun plan transition <entry> --undo` verb. The verb refuses to
skip rungs — it implements exactly `Done → InProgress` and
`InProgress → Pending` per call, so undoing a `done` entry to
`pending` MUST run twice. Each step emits one
`plan.transition.undone` journal event carrying `{ plan-name,
slice-name, from, to }` so replay traces line up with the
forward-direction cadence (`plan.transition.approved`,
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
plan archive` moves `change.md` + `plan.yaml` into
`.specify/archive/plans/`, but the plan-level lifecycle stamp
inside the archived `plan.yaml` stays at `reviewed`.
There is no `archived` enum variant on `plan.yaml.lifecycle` — the
on-disk location of the file is the archived signal, not a stored
state.

## `SliceSourceBinding`: bare shorthand plus structured form

`plan.yaml.slices[].sources` is a single in-memory struct
(`{ source_key: String, lead_id: Option<String> }`) with a custom
`Deserialize` impl that accepts two wire shapes and a custom
`Serialize` impl that emits whichever shape produced the value:

- **Bare string shorthand** — `legacy` parses to
  `source_key = "legacy"`, `lead_id = None`; serialises back as the
  bare string. The lead-id falls back to the owning slice's name at
  lookup time via `SliceSourceBinding::lead_id(slice_name)`,
  preserving the one-source-per-slice degenerate case (predominantly
  `intent`).
- **Structured form** — `{ source-key: legacy, lead-id: legacy-monolith }`
  parses to `source_key = "legacy"`,
  `lead_id = Some("legacy-monolith")`; serialises back as the same
  `{ source-key, lead-id }` map. Required whenever the source key and
  the lead id differ.

Collapsing the two variants into one struct means every consumer
(`validate`, `doctor`, `provenance`, CLI handlers) goes through the same
`source_key()` / `lead_id()` accessors instead of `match`-ing the
discriminator — the shorthand stays a pure parser concern. Construct in
tests via `SliceSourceBinding::bare(source_key)` or
`SliceSourceBinding::structured(source_key, lead_id)` so the discipline
stays consistent. `plan amend --add-source <key>` and `plan create`
share the same shorthand on the wire. Refer to workflow §`Slice.sources`.

## `Divergence` enum

`plan.yaml.slices[].divergence` is the closed enum
`none | likely | accepted | rejected` (kebab-case on the wire;
`snake_case` Rust variants joined by `#[serde(rename = "…")]`). `none`
is the implicit default and is elided from serialised output.
`specrun plan amend --divergence` only accepts `accepted | rejected`
from the wire — `none` is the absent default, and `likely` is reserved
for the `propose` sub-step of `/spec:plan`, which writes the value via
a direct YAML edit (per the W3.2 hand-off). Operators flipping the
field after Gate 1 review use `accepted | rejected` exclusively.
Refer to workflow §"Plan-time reconciliation".

## Evidence per-kind authority overrides

`evidence.schema.json` gains an
optional `authority-overrides` map keyed by claim kind, valued by
authority class. The document-level `authority:` field stays
required; the override applies to all claims of the named kind in
that Evidence document. Per-claim overrides remain explicitly
deferred. Synthesis consults the per-kind override first, then the
document-level `authority:`, then the workflow default ordering — a
byte-stable three-step fallback chain.

## Plan per-slice authority overrides

`plan.yaml.slices[]` gains an
optional `authority-override` map keyed by claim kind, valued by
source key. Keys come from the closed claim-kind enum; values MUST
be source keys present in the slice's own `sources[]` list. Orphan
keys are rejected by `specrun slice validate` with the
`slice-authority-override-orphan-source-key` kebab discriminant. The
map is scoped to one slice — plan-wide and project-wide overrides
are out of scope.

## `provenance.yaml` audit index

`schemas/slice/provenance.schema.json`
fixes the closed top-level shape (`version`, `slice`,
`generated-at`, `generator`, `requirements[]`). `/spec:refine`
writes the file atomically; downstream verbs read `spec.md` as the
authoritative artifact and treat `provenance.yaml` as an inspection
surface. `specrun slice validate` enforces id-set parity between
`spec.md` `REQ-*` ids and `provenance.yaml.requirements[].id` and
catches contributing-claim → Evidence-claim drift, both via the
`slice-provenance-drift` discriminant.

## Extraction cache fingerprint inputs

The closed list of fingerprint
inputs (`source path canonicalised | adapter name@version | brief
sha256 | sorted declared-tool versions | lead id`) lives on
[`crate::adapter::cache::CacheFingerprint`]. CI that pins the four inputs
common across runs can re-run any prior `/spec:execute` and expect
byte-stable cache hits; CI observing any of the five
`slice.extract.cache-miss` reasons knows exactly which input
drifted. Adapter authors opt out with `cache: opt-out` on
`adapter.yaml`; the matching journal event carries `reason:
adapter-opt-out`. `lead id` is the one input that distinguishes the
two source operations: `specrun source survey` keys the fingerprint
**without** a lead id (it runs at plan time and carries no slice),
while `specrun source extract` keys it **with** the lead id. See
§"Source operations (D1)".

## Journal event names

`crates/workflow/src/journal.rs` emits the closed journal event
taxonomy. The wire ids are dotted kebab-case; the Rust `EventKind`
variants are `snake_case` and bridge to the wire via
`#[serde(rename = "…")]`. The taxonomy added in Wave 1.4
(`cli/W1.4`) is:

| Wire id | Emitted by |
|---|---|
| `plan.transition.approved` | `specrun plan transition <plan> approved` (Gate 1 stamp). |
| `plan.transition.undone` | `specrun plan transition <entry> --undo` (per-entry reverse rung; one event per rung). |
| `plan.amend.divergence` | `specrun plan amend --divergence likely\|accepted\|rejected` on any change to a slice's `divergence` field (the `/spec:plan` agent stages `likely`; the operator flips `accepted`/`rejected`). |
| `slice.transition.refined` | `specrun slice transition <slice> refined`. |
| `slice.extract.completed` | The `/spec:refine` skill, after the serial `extract` loop closes. |
| `slice.synthesis.conflict` / `.divergence` / `.unknown` | `specrun slice validate`, one per requirement-block tag emitted by the synthesis substep. |
| `slice.extract.cache-hit` / `.cache-miss` | The extract code path; payloads carry the fingerprint sha256 (and the closed `reason` enum on misses). the extraction cache fingerprint contract. |
| `source.survey.cache-hit` / `.cache-miss` | The `specrun source survey` runner's cache probe; payloads carry `source-key`, `adapter`, the fingerprint sha256 (and the closed `CacheMissReason` enum on misses — a forced-opt-out survey reports `reason: adapter-opt-out`). |
| `source.execution.agent` | The `survey` / `extract` runner on every `execution: agent` invocation; payload carries `source-key`, `adapter`, and the closed `SourceOperation` (`survey` \| `extract`). |
| `slice.provenance.written` | `/spec:refine`'s atomic `provenance.yaml` writer (Change 2.6). `provenance.yaml` audit semantics. |
| `slice.replay.completed` | Target adapter's `build` step when it consumes runtime captures; optional in v1. runtime capture semantics. |
| `plan.amend.authority-override` | `specrun plan create --authority-override`, `specrun plan amend --authority-override` / `--clear-authority-override` / `--clear-authority-overrides`. per-slice authority override semantics. |
| `lint-completed` | `specrun lint run` after each scan; payload carries `scope`, `duration_ms`, per-status `counts.{open, ignored, false_positive}`, `baseline_present` (hard-coded `false` until RFC-33b lands), and the resolved `exit_code`. Wire field names are snake_case to match the journal payload verbatim. |

Events persist as newline-delimited JSON at
`<project_dir>/.specify/journal.jsonl`. The closed `from` / `to`
enum on the divergence events is
`none | likely | accepted | rejected`. Refer to workflow §"Observability"
and the per-event row table.

### `specrun journal emit` — guarded front door (D12)

Deterministic commands emit their own events. Agent-orchestrated
phases that have no deterministic emit command (e.g. the `execution:
agent` source operations) write through `specrun journal emit
<event-id> [--payload <json>] [--format json]`
([`src/runtime/commands/journal/emit.rs`](./src/runtime/commands/journal/emit.rs)).
The verb mints **no event kinds of its own** — it is a guarded front
door onto the same closed `EventKind` taxonomy, preserving "one closed
taxonomy, one writer". The closed enum is itself the per-kind payload
schema (there is no parallel JSON-schema registry), so the guard is a
single serde round-trip: the handler reassembles the adjacently-tagged
`{ event, payload }` shape and deserialises it into `EventKind`. An
unknown tag fails `journal-emit-unknown-event`; a payload that misses a
variant's required field fails `journal-emit-payload-schema`. Both are
`Error::Validation`, exit 2. The CLI — never the agent — stamps the
second-precision UTC `timestamp` and appends exactly one line via
`journal::append_batch`.

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

`$CAPABILITY_DIR` is also the read-only manifest-cache root of the
four-root source-operation sandbox (`$SOURCE_DIR` / `$CAPABILITY_DIR` /
`$SCRATCH_DIR` / `$PROJECT_DIR`); see §"Source operations (D1)".

## Lifecycle write-ownership

Per-entry status writes route to exactly one CLI verb. Skill bodies
never write status by hand; the CLI is the single source of truth for
each transition:

| State | Writer | Trigger |
|---|---|---|
| `pending` (per-entry) | `specrun plan add` / `specrun plan amend` | Operator (or `/spec:plan`) authors / edits a slice row. |
| `in-progress` (per-entry) | `specrun plan next` | Sole writer; the `/spec:execute` loop calls it once per slice. |
| `done` (per-entry) | `specrun plan transition <entry> done` | Called by `/spec:merge` after `specrun slice merge` succeeds. |
| `pending` (plan-level) | `specrun plan create` | `/spec:plan` scaffolds the plan in `pending`. |
| `reviewed` (plan-level) | `specrun plan transition <plan> approved` | Operator-only (Gate 1). The CLI is ungated; `/spec:plan` MUST NOT call this verb — `--help` text documents the rule and the skill body is the actual gate. |

The plan-level `reviewed` row is the lightest-touch shape the workflow
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
and the Rust loader (`specify_workflow::change::SourceBinding`).

The `specrun plan create --source` flag grammar mirrors the wire
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
`crates/workflow/src/change/plan/core/model.rs::SourceBinding`.

Source keys are plan-scoped; each key maps to exactly one binding
under `Plan::sources`, but slices may reference the same key with
different leads.

## Adapter manifest requireds

`description` is required at the top level of every adapter manifest —
sources and targets alike — alongside the existing `name`, `version`,
`axis`, and `briefs`. `tools[].version` is required for every declared
tool. The accepted shape is semver only: `x.y.z` with an optional
`-prerelease` suffix, locked by the schema pattern
`^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$`. No `v` prefix, no `sha256:` digest,
no free-form strings. Tools without a release must cut one before being
declared. The reproducibility argument is the extraction cache
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
mirrors). Eagerly enforced at `specrun init` time (inside
`crates/workflow/src/init/cache.rs::cache_adapter`, before the target
cache directory is rewritten) and at `*Adapter::resolve` time. The
resolve-time probe lives in
`crates/workflow/src/adapter/core.rs::locate_axis`, which checks the
opposite axis for a sibling `adapter.yaml` via `sibling_manifest_path`
on every resolve. `specrun` is fork-and-exit, so the pair of `is_file`
probes is cheaper than memoising them behind process-global state. The
public `check_axis_unique_for_name(axis, name, project_dir)` helper is the
one-sided variant `init` calls before the side it is about to
install exists on disk. Collisions surface as `Error::Validation`
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
  `crates/workflow/src/adapter/core.rs::cache_dir`.
- `extractions/<adapter>/<fingerprint>/` — per-source extraction result cache, with the append-only `index.jsonl` at the
  adapter root (`extractions/<adapter>/index.jsonl`). Per-adapter only —
  not per-axis — because extraction is a source-axis operation; the
  adapter name carries enough identity. Resolved by
  `crates/workflow/src/adapter/cache/io.rs::CacheLayout`.

Each cache owns its own root, so the loader no longer probes for an
`adapter.yaml` inside the cache directory to disambiguate manifest vs.
extraction co-tenancy — a manifest-cache directory is always a manifest
mirror, and the extraction tree never carries `adapter.yaml` at any
level. Refer to §"Extraction cache fingerprint inputs" for the extraction-cache fingerprint contract.

## Target adapter suffix policy

`plan.yaml.slices[].target` carries the `name@vN` form (e.g.
`omnia@v1`) and the integer `N` is a load-bearing wire field, not
decorative metadata:

- `schemas/plan/plan.schema.json` pins the wire shape with the regex
  `^[a-z][a-z0-9-]*@v\d+$`; bare names and non-kebab variants are
  rejected at schema-validation time.
- `crates/workflow/src/change/plan/core/model.rs::TargetRef` is the
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
  `crates/workflow/src/adapter/operation.rs` are the typed `briefs.keys()`
  carried by each manifest struct; manifest brief maps are enum-keyed
  and string literals at call sites are gone.
- **Wire invariant.** The `specrun source resolve` and
  `specrun target resolve` JSON envelopes' `operations: [...]` arrays
  iterate in kebab-alphabetical order (e.g. `["extract", "survey"]`,
  `["build", "merge", "shape"]`). Derived `Ord` on
  `{Source,Target}Operation` is intentional because enum variants are
  declared in kebab-alphabetical wire order.

## Adapter execution mode (D9)

Every adapter manifest declares a closed `execution` enum — `agent |
tool` — `required` at the top level of both `source.schema.json` and
`target.schema.json` (RFC-29 D9). The loader rejects a manifest that
omits the field with `adapter-execution-mode-required` rather than
defaulting silently, and rejects `execution: agent` declared alongside
any cache mode other than `opt-out` with
`adapter-execution-agent-cache-conflict`
(`check_execution` in [`crates/workflow/src/adapter/core.rs`](./crates/workflow/src/adapter/core.rs)).
Both are `Error::Validation`, exit 2. The conflict check is a
**forward-guard**: `CacheMode` is a single-variant enum (`OptOut`) and
`source.schema.json#/properties/cache` enumerates only `["opt-out"]`,
so no legal manifest can trigger it today — it exists to catch a
future-widened `CacheMode`. The `Execution` enum lives on
`SourceAdapter` / `TargetAdapter`; `execution: agent` forces the
effective cache mode to `opt-out` regardless of the declared `cache:`
field, which `SourceAdapter::effective_cache_mode` (consumed by the
source-operation runner, not the raw `cache` field) returns.

The two values branch dispatch:

- **`agent`** — the brief is run by an agent against the sandbox
  preopens. The CLI orchestrates inputs, validates outputs against the
  same schemas, never caches the result, and emits a `*.execution.agent`
  journal event per invocation. Dispatch is **two-phase** (prepare /
  finalize; see §"Source operations (D1)").
- **`tool`** — `survey` / `extract` (sources) or `build` / `merge`
  (targets) dispatch through a declared WASI tool or built-in
  deterministic Rust path, **single-phase** within one process, with the
  result cached under the extraction fingerprint.

All eight first-party manifests (five sources, three targets) ship
`execution: agent` — none owns a deterministic tool yet, so `agent` is
the truthful value and the `tool` branch is wired and schema-valid but
unexercised by first-party adapters until a source or target gains a
real tool. A `suggestion`-severity `adapter.execution-agent` standards
finding (CORE-051) flags first-party `agent` adapters only (never
third-party), nudging toward a future `tool` path. M1 lands the source
side; the target side (`build` / `merge` dispatch) follows with
RFC-29 M3, which is why target manifests carry `agent` as a placeholder
until then. Refer to workflow §"Adapter implementation shape".

## Source operations (D1)

`specrun source survey <source-key> [--plan <name>] [--phase
prepare|finalize]` and `specrun source extract <source-key> <lead-id>
--slice <slice> [--phase prepare|finalize]` are the CLI-owned source
adapter operations (RFC-29 D1; handlers under
[`src/runtime/commands/source/`](./src/runtime/commands/source)).
`<source-key>` resolves against `plan.yaml.sources.<key>` — **not** the
adapter name — and the adapter is then resolved from
`SourceBinding.adapter`. `survey` validates the lead set against
`schemas/discovery/lead.schema.json` and merges it into `discovery.md`;
`extract` validates the Evidence against `schemas/evidence.schema.json`
and persists it to `.specify/slices/<slice>/evidence/<source-key>.yaml`.

`discovery.md` stores **raw, unmerged, per-source leads**: each block is
one lead as surfaced by one source, identified by its `(source-key,
lead-id)` pair (the runner stamps `source-key` from the surveyed source,
so attribution is CLI-owned and a lead-set need not repeat it). "Merges
it into `discovery.md`" is a per-source re-survey fold, **not** a
cross-source collapse: a re-survey of one source replaces only that
source's blocks by `(source-key, lead-id)` and leaves every other
source's blocks untouched, so the same `lead-id` may legally appear under
different source keys. Alias-collision scoping is therefore per
`source-key`. Cross-source unification of leads is deferred to plan time
(D2 reconciliation), where the umbrella RFC places fan-in; `survey` never
unifies across sources.
Both gates are **validate-before-visible**: an invalid lead set leaves
`discovery.md` untouched, and a failed Evidence validation leaves the
slice in `refining` (see [`crates/workflow/src/schema.rs`](./crates/workflow/src/schema.rs)).

**Four-root sandbox.** Each operation runs under four preopen roots:
`$SOURCE_DIR` read-only (the bound source path; **absent** for
value-bound sources such as `intent`), `$CAPABILITY_DIR` read-only (the
resolved manifest cache), `$SCRATCH_DIR` write-only, and `$PROJECT_DIR`
**not visible** — lifecycle state stays off-limits to the adapter.
`$SCRATCH_DIR` nests under the per-adapter extraction tree but disjoint
from the fingerprint result cache so a scratch write never pollutes a
cache artifact: `extract` uses
`.specify/.cache/extractions/<adapter>/<slice>/scratch/`; `survey`
(plan-time, no slice) uses
`.specify/.cache/extractions/<adapter>/survey/scratch/`. A 64-char-hex
digest dir can never equal a `<slice>`/`survey` segment, so the two
trees provably never collide. See §"Cache layout" and
§"`$CAPABILITY_DIR` replaces `$ADAPTER_DIR`".

**Shared prep seam.** The adapter resolution, brief-directory
resolution (the `briefs-dir` resolve field), four-root sandbox layout,
and `evidence/` scaffolding are a single internal helper
([`src/runtime/commands/source/prep.rs`](./src/runtime/commands/source/prep.rs))
shared by the workflow-free `specrun source preview` (§"`specrun source
preview`") and the workflow-integrated `survey` / `extract` runners.
The runners add the `execution`-branched dispatch, the extraction-cache
fingerprint, the journal events, validate-before-visible, and the
`discovery.md` merge / Evidence persist on top of that seam — none of
which is re-implemented in `preview`.

**Agent dispatch is two-phase.** Under `execution: agent` the operation
splits: `--phase prepare` (the default) resolves the adapter, builds the
four-root sandbox, scaffolds the output target, emits
`source.execution.agent`, and prints a handoff envelope on stdout, then
returns control. `--phase finalize` runs after the agent has executed
the brief against the prepared sandbox: it validates, persists, caches,
and journals. The CLI never blocks waiting on agent work. The handoff
envelope is kebab-case JSON: `survey` prints `{ adapter, version,
briefs-dir, source-dir?, scratch-dir, leads[], execution: "agent" }`
(no `evidence-dir`; survey produces a lead set, not Evidence);
`extract` prints the same plus `evidence-dir` and a single-element
`leads` array. `tool`-execution adapters ignore the phase flag — a
single call runs the whole operation and never prints the envelope.

**Value-binding envelope.** For value-bound sources (`intent`),
`$SOURCE_DIR` is absent and the source request carries `value-inline:
<string>`; path bindings carry `source-path`. This reuses the existing
`FingerprintSource::{Path, Value}` (the value variant keys on the
sha256 of the literal body), so no new cache machinery — and no
RFC-29d build-request schema — is introduced.

## Tool-owned schemas

Every JSON Schema is owned by the repo of the WASI tool (or the CLI)
that runs it. Plugin briefs reference schemas exclusively by their
canonical `$id` URL and never contain schema bodies. The plugin repo's
`adapters/targets/vectis/schemas/` directory carries only a README
that documents the canonical URLs and the `specrun tool schema`
quickstart. The three Vectis runtime schemas (`tokens`, `assets`,
`composition`) live solely in
[`wasi-tools/vectis/embedded/`](./wasi-tools/vectis/embedded/); the
previous "byte-identity discipline" duplication and manual mirroring
obligation are retired.

## `specrun tool schema` verb

`specrun tool schema <tool> <name>` is a convenience wrapper that
delegates to the tool's `schema <name>` subcommand via the existing
`tool::run` path and passes through the guest's exit code. Exits `0`
when the schema is emitted to stdout; exits `2` for an unknown tool or
unknown schema name. Implementation:
[`src/runtime/commands/tool/schema.rs`](./src/runtime/commands/tool/schema.rs) on the
host side, [`wasi-tools/vectis/src/schema.rs`](./wasi-tools/vectis/src/schema.rs)
on the guest side. The contract tool returns exit `2` for any schema
name (no schemas declared).

## Schema `$id` convention

Tool-owned schemas use a stable `$id` of the form
`https://schemas.specify.dev/<tool>/<name>.schema.json`. The URL is a
logical identifier; it does not need to resolve to a hosted copy. The
convention settles the prior disagreement between
`adapters/vectis/...` and `targets/vectis/...` paths. CLI-owned
framework schemas (e.g. the component catalog) use
`https://schemas.specify.dev/specify/<path>`. The
`links.brief-schema-link-resolve` predicate in
[`crates/standards/src/framework/check/schema_links.rs`](./crates/standards/src/framework/check/schema_links.rs)
enforces that every `schemas.specify.dev` URL cited in adapter briefs
resolves to a known schema in the hardcoded registry.

## `specrun source preview`

`specrun source preview <adapter> --source <path> [--lead <id>...]
[--out <path>]` is a workflow-free verb: it resolves the source adapter,
validates `--source`, scaffolds `${out}/evidence/`, and emits a summary
of adapter info and brief paths. No `.specify/` writes, no journal
events. The verb uses `dispatch` (not `scoped`) so no `.specify/`
directory is required. Implementation:
[`src/runtime/commands/source/preview.rs`](./src/runtime/commands/source/preview.rs).
The v1 ships against the agent-run fallback (the agent reads the brief
and executes it into the scaffolded output directory); full runner
integration depends on first-class `specrun source survey` /
`specrun source extract` runner support.

## Component catalog

An operator-curated file at `.specify/design-system/components.yaml`
declares shared UI components (`status: confirmed | rejected`). The
schema is CLI-owned at
[`schemas/design-system/components.schema.json`](./schemas/design-system/components.schema.json);
the domain type is `ComponentsCatalog` in
[`crates/workflow/src/design_system.rs`](./crates/workflow/src/design_system.rs)
with `load()`, `confirmed_slugs()`, `rejected_slugs()`, and
`status_of()` accessors. The catalog is opt-in — projects without the
file work exactly as before. Slugs are kebab-case
(`^[a-z][a-z0-9]*(-[a-z0-9]+)*$`). `specrun slice validate` enforces
`slice-catalog-drift`: every Evidence claim carrying `component: <slug>`
must resolve to a confirmed catalog entry; absent or rejected entries
are findings. `notes.candidate_component` annotations are
informational-only and do not trigger drift. Validation gates at
position 4 in `validate_pre_adapter_gates` (after provenance drift,
authority override, and discovery alias).

## Vectis catalog consumer

The Vectis target's `build` brief reads `.specify/design-system/components.yaml`
and factors shared component code per confirmed entry per in-scope shell
tree. The brief additions live in the plugin repo under
`adapters/targets/vectis/briefs/build/`. The Vectis WASI tool's
`validate composition` mode (check 5 in
[`wasi-tools/vectis/src/validate/engine/composition.rs`](./wasi-tools/vectis/src/validate/engine/composition.rs))
enforces catalog cross-references: every `component: <slug>` in
`composition.yaml` must resolve to a confirmed catalog entry (missing
or rejected = error); every confirmed catalog entry should have at
least one reference (unreferenced = warning). Catalog discovery uses
[`wasi-tools/vectis/src/validate/engine/paths.rs`](./wasi-tools/vectis/src/validate/engine/paths.rs)
to locate the file from the project root. When the catalog is absent,
the check is silently skipped.

## Standards layer split into `specify-standards` and `specify-schema`

The standards surface (rules parser / resolver, `WorkspaceModel`,
indexer, deterministic hint interpreter, the `DiagnosticProducer`
trait, and `specrun lint` runner) lives in `specify-standards`, a sibling
of `specify-workflow` rather than a module inside it. `specify-schema`
is the shared leaf: it owns every embedded JSON Schema constant
(`PLAN_JSON_SCHEMA`, `EVIDENCE_JSON_SCHEMA`,
`PROVENANCE_JSON_SCHEMA`, `COMPONENTS_JSON_SCHEMA`, `RULE_JSON_SCHEMA`,
`RESOLVED_RULES_JSON_SCHEMA`, `DIAGNOSTIC_JSON_SCHEMA`,
`DIAGNOSTIC_REPORT_JSON_SCHEMA`, `WORKSPACE_MODEL_JSON_SCHEMA`) plus the
`jsonschema` plumbing (`compile_schema`, `validate_value`,
`validate_serialisable`, `read_yaml_as_json`). `specify-schema` shares
the leaf layer with `specify-error` and depends on no workspace crate
other than `specify-error` itself.

The neutral diagnostic substrate — the `Diagnostic` / `DiagnosticReport`
/ `DiagnosticSummary` currency, the fingerprint algorithm, the
`validate_diagnostic` validator, the four renderers, and the `blocking`
predicate — lives in its own `specify-diagnostics` leaf (see
[§"Drained `Error::Validation` and the `Diagnostic`
substrate"](#drained-errorvalidation-and-the-diagnostic-substrate)).
`specify-standards` depends on it for the report shape; so do the producer
crates (`specify-validate`, `specify-model`, `specify-digest`,
`specify-tool`, `specify-workflow`). `specify-standards` keeps the
lint-specific engine.

Dependency direction: `specify-standards` depends on `specify-error`,
`specify-schema`, `specify-diagnostics`, and `specify-digest` (digest
encoding for framework checks). The `kind: tool` lint evaluator is wired
through a `ToolRunner` trait at the CLI boundary — not via a
`specify-tool` dependency. It does **not** depend on `specify-workflow`, and
`specify-workflow` does **not** depend on `specify-standards`. The sibling
shape makes the §"Principles" / "No lifecycle authority in review" rule
a type-system invariant: review code cannot reach for slice or plan
lifecycle transitions because the workflow types are not visible from
the standards layer. The substrate split lets `specify-workflow` and
`specify-validate` mint diagnostics (via `specify-diagnostics`) without
ever depending on anything named `lint` — the litmus test that keeps
the lint-vs-validate concept split from re-appearing at the crate
graph.

The `specdev` predicate library (`specify_standards::framework`) sits inside
`specify-standards` itself, so codex predicates consume `RULE_JSON_SCHEMA`
and the typed `Rule` DTO without re-vendoring the schema. The root
`specify` binary wires both halves
together at the dispatcher boundary — `specrun lint` consumes the
standards layer for indexing and evaluation and the workflow layer for
project / slice context resolution; the two halves never call each
other directly. The dependency-direction rationale is captured in this topic and
[`docs/standards/architecture.md`](./docs/standards/architecture.md).

## Drained `Error::Validation` and the `Diagnostic` substrate

Every check surface — `specrun lint`, `specdev lint`, `specrun slice
validate`, plan validation, and the library-level validators — speaks
one currency: `Diagnostic` / `DiagnosticReport`, housed in the
`specify-diagnostics` leaf. The leaf depends only on `specify-error`,
`specify-schema`, and `specify-digest`; it must never depend
on `specify-standards`, `specify-model`, or `specify-workflow`, so it stays
cycle-free and importable by every producer.

**Lint and validate stay conceptually distinct surfaces.** They share
the substrate, not the authority:

- **validate** gates a lifecycle transition (`refining → refined`). It is
  workflow-owned, non-negotiable, and non-silenceable — ignore
  directives are *off* for the lifecycle gate.
- **lint** is standards/policy compliance. It is codex-owned, versioned,
  lifecycle-neutral (may block CI, never transitions a slice), and
  silenceable with an in-source rationale.

Convergence applies to the data type, fingerprint, validator, renderer,
and blocking predicate — never to the concepts or their gate policies.
The naming convention encodes the same neutrality one layer down:
surfaces keep concept names (`lint` / `validate`), the shared machinery
is neutral (`specify-diagnostics`). The litmus test is that `validate`
(or any non-lint producer) must not depend on a crate or module named
`lint`.

**Two orthogonal axes** keep the concepts queryable on the one type:

- `source` — provenance: `deterministic | model-assisted | hybrid |
  human | tool`.
- `kind` — nature: `violation` (a defect) vs `review` (a
  deterministically-raised request for agent/human judgment). The
  former `Deferred` classification and lint's `lint-mode:
  model-assisted` rules both surface as `kind: review`, `source:
  deterministic` (the CLI raised the question; it did not score it).

**Uniform blocking predicate, per-surface application.** `blocking()` in
`specify-diagnostics` returns true iff `kind == violation && status ==
open && severity ∈ {critical, important}`. `kind == review` never blocks
anywhere; the refine surface reads its judgment worklist as
`diagnostics.filter(kind == review)`. Each surface applies the same
predicate, differing only in whether ignore directives run first (lint:
yes; validate: no).

**`Error::Validation` is payload-free.** The variant is
`Error::Validation { code, detail }`; `variant_str()` returns the
carried `code`, so the top-level wire `error` is the specific
discriminant (e.g. `slice-pre-adapter-gate`, `plan-schema`,
`tool.name-format`) rather than the historical generic `"validation"`.
Handlers own rendering: a gate failure renders the full
`DiagnosticReport` on **stdout** and then returns the payload-free error
purely to carry the exit code (2) and the discriminant on stderr.
Single operational errors that are not findings (e.g.
`discovery-lead-unknown`, `adapter-name-axis-collision`,
`tool-not-declared`, `rules-root-required`) take the same payload-free
shape via `Error::validation_failed(code, detail)` but render no report.

**Widened `ruleId` namespace.** The diagnostic `ruleId` pattern accepts
both the closed codex family (`UNI-`/`CORE-`/`FRAME-`/… `-NNN`) and the
runtime-validation discriminant form (dotted/kebab lowercase, e.g.
`spec.requirement-id-missing`, `slice-provenance-drift`), so workflow
and validate producers can stamp their invariant ids onto the same
finding shape the codex engine uses. `validate_diagnostic` mirrors the
widened pattern.

## Vendored codex-rule schema removed

The standalone vendored copy of the codex-rule JSON Schema and its
drift-detection predicate are retired. Specifically, the following
artefacts are gone:

- the framework crate's vendored `schemas/rule.schema.json` copy
- `scripts/sync-codex-schema.sh` (the manual mirroring helper)
- the framework crate's `check/codex_schema_drift.rs` (the CH-09
  predicate implementation) and its integration test
- The `codex.schema-drift` rule-id registration in the `specdev` check
  registry

The `specify_standards::framework` predicates now consume the canonical schema directly via
`specify_schema::RULE_JSON_SCHEMA` (paired with the typed
`Rule` DTO from `specify_standards::rules`). One source of truth replaces the
prior vendored / canonical pair, so no drift check is required — the
predicate the vendored copy existed to support no longer has a class of
failures to catch. The canonical schema lives at
`schemas/rules/rule.schema.json` (its `$id` was corrected to
match the on-disk location as part of the same change); the `specify`
binary embeds it through `specify-schema` like every other workflow and
standards schema. Cross-repo prose that named `codex.schema-drift`
(CH-09) or the sync script is removed in the same change. This topic is the durable record for removing the vendored codex-rule schema.

## Lint finding status, disposition, and exit

`Diagnostic.status` (the finding type formerly named `LintFinding`,
relocated to `specify-diagnostics`) is a closed kebab-case enum on the
wire. The fingerprint algorithm excludes both `status` and
`disposition`, so demoting a finding from `open` to `ignored` (or
`false-positive`) never changes its identity.

| Value            | Set by                | Meaning                                                                                                                                |
|------------------|-----------------------|----------------------------------------------------------------------------------------------------------------------------------------|
| `open`           | scanner (default)     | Freshly emitted finding before post-passes run. The only value that contributes to the `specrun lint` exit-code decision by default.   |
| `ignored`        | directive pass        | An in-source `specify-ignore` directive matched the finding's `(path, line, rule-id)`. Carries `disposition.directive`.                |
| `false-positive` | directive pass        | A directive matched and the rationale was prefixed `false-positive:`. Reported separately in dashboards.                                |
| `fixed`          | reserved              | Reserved for the cross-run baseline diff verb. No producer in v1.                                                                      |
| `accepted`       | reserved              | Reserved for explicit operator acceptance via the baseline file. No producer in v1.                                                    |

`disposition` is an optional sibling object on `Diagnostic`, populated
only when `status != open`:

```text
disposition: { source, directive?, since? }
```

`disposition.source` is itself a closed enum. Today the only value is
`directive` (set by the directive-validation pass in
[`crates/standards/src/lint/ignore.rs`](./crates/standards/src/lint/ignore.rs)).
A future cross-run baseline producer can add `baseline` additively
without churning callers — every consumer that exhausts the enum is
already required to tolerate unknown values under the additive
schema-evolution policy.

`specrun lint run` resolves the process exit using **status-aware
severity** rather than severity alone:

> Exit `2` only when there is a finding with `status: open` AND
> `severity ∈ {critical, important}`. Findings with
> `status: ignored` or `status: false-positive` remain in every
> formatter and in the JSON envelope, but they do not contribute to
> the blocking decision.

The synthetic findings the directive pass emits for malformed
(`UNI-022`) and orphan (`UNI-023`) directives default to `status:
open`, so they block when their severities are critical or important
and stay non-blocking otherwise. The shared codex tree ships both
rules at `important`.

**Graceful degradation.** When the codex resolver does not produce
`UNI-022` or `UNI-023` — typically a consumer project that has not
yet picked up the shared codex tree — the directive pass silently
skips synthetic emission. Status stamping on matched directives
continues to run; only the policing of malformed and orphan
directives degrades. The fix is to pass `--rules-root` or to
distribute the shared codex into `.specify/.cache/codex/` via
`specrun init` / `specrun rules sync` (codex distribution, RM-07);
once the `universal/` pack resolves, `UNI-022` / `UNI-023` policing
stops degrading and fires on the consumer project. The
codex-distribution probe rung and pinning are recorded in
[§"Shared codex distribution"](#shared-codex-distribution).

**Operator-facing reference.** The directive grammar (comment-style
table, em-dash / `--` separator tolerance, target-line semantics,
inline-trailing form, and the 16-character rationale floor) lives in
the operator-facing reference at
[`docs/reference/ignore-directives.md`](https://github.com/augentic/specify/blob/main/docs/reference/ignore-directives.md)
in the parent plugin repo; it is not re-stated here. The journal
`lint-completed` event payload lives in
[§"Journal event names"](#journal-event-names).

## Shared codex distribution

Consumer projects resolve shared `UNI-*` rules without a co-located
framework checkout or a manual `--rules-root` (RM-07). The shared codex
ships beside the target adapter in its source repo
(`adapters/shared/rules/{universal,core}/`); `specrun init` and the
standalone `specrun rules sync` verb mirror it into the project codex
cache, **pinned to the same adapter source/ref**.

- **Cache location.** `<project_dir>/.specify/.cache/codex/`, mirroring
  `adapters/shared/rules/{universal,core}/` underneath. The codex
  resolver joins the same relative path onto its rules root, so the new
  rung needs no special-casing. This replaces the earlier lint-only
  `.specify/cache/rules/` fallback (now removed) and standardises on the
  dotted `.specify/.cache/` tree the manifest cache already uses.
- **Probe order.** `probe_rules_root` in
  [`crates/standards/src/rules/resolve.rs`](./crates/standards/src/rules/resolve.rs)
  is the closed precedence: (1) explicit `--rules-root`; (2) monorepo
  `{project_dir}/adapters/shared/rules/universal/`; (3) **new** codex
  cache `{project_dir}/.specify/.cache/codex/...`; (4)
  `rules-root-required`. Step 3 is a derived (non-explicit) root, so the
  rules-root fallback overlay step stays skipped, exactly like the
  monorepo case. The rung lives in the resolver so both `specrun lint`
  and `specrun rules export` honour it.
- **Distribution.** `cache_codex` /
  `sync_codex` in [`crates/workflow/src/init/`](./crates/workflow/src/init)
  walk up from the resolved adapter `source_dir` to the nearest ancestor
  carrying the `universal/` pack and copy it (and, under
  `--include-framework`, `core/`) into the cache. Git sources fetch
  `adapters/shared/rules/` in the same sparse checkout as the adapter —
  no second clone. **Fail-soft:** a source tree without the shared pack
  leaves the cache empty and the consumer falls back to `--rules-root`.
- **Provenance.** `CodexMeta` (`.specify/.cache/codex/.codex-meta.yaml`)
  records the pinned adapter `source` value, `include_framework`, and
  `fetched_at`. Audit-only; the resolver never reads it.
- **Distribution vs evaluation.** `--include-framework` (init / `rules
  sync`) controls what lands in the cache; the resolver's `include_core`
  (on `lint` / `rules export`) controls whether `CORE-*` rules are
  evaluated/exported. They are independent: consumer projects default to
  neither distributing nor evaluating the framework `core/` pack.
