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
`serde_json::Value`. Its separate deser/ser error types are wrapped
behind `specify_error::YamlError` / `YamlSerError` so the upstream crate
name does not leak through every public surface.

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
`ContextUnfenced`, `ContextDrift`, `InitNeedsCapability`,
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
  (`{"proposed-name": ..., "proposed-url": ..., "proposed-capability":
  ..., "proposed-description": ..., "rationale": ...}`) and pass it
  verbatim. The on-disk `outcome.outcome.registry-amendment-required.*`
  shape and the `outcome.proposal` JSON returned by `slice outcome
  show` are unchanged.
- `specify init` enforces the `<capability>` xor `--hub` invariant
  through clap. The historical post-parse
  `init-requires-capability-or-hub` envelope is gone on the CLI
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

Five workspace crates: `specify-error` (leaf), `specify-validate`
(carve-out-shared contract validation), `specify-domain` (every other
domain module), `specify-tool` (WASI host, gated), and the `specify`
binary.

History: until Phase 1B of the 2026-05 cleanup the workspace had 13
crates; the fragmentation cost more than it earned (wide build graph,
redundant `Cargo.toml` files, indirect re-export hops, repeated
`multiple_crate_versions` waivers). Module boundaries inside
`specify-domain` preserve the original separation; `pub` cross-module
surfaces match the prior cross-crate `pub use` exports.

`specify-validate` is the one cleanup-era re-extraction. It owns the
baseline-contract validation primitives (`ContractFinding`,
`validate_baseline`, `serialize_contract_findings`) and is consumed by
both `specify-domain` (for compatibility classification) and the
`wasi-tools/contract` carve-out (for the standalone `wasm32-wasip2`
binary). The carve-out invariant in `wasi-tools/Cargo.toml` forbids a
dep on `specify-domain` (would drag `wasmtime` / `tokio` / `ureq`),
and inlining the ~300 LOC of validation into the carve-out would lose
the single source of truth. The crate is dependency-minimal (`semver`,
`serde`, `serde-saphyr`, `serde_json`) so it does not regrow the
duplicate-version surface that motivated Phase 1B.

Rule: new functionality lands in an existing module by default. New
workspace crates require a paragraph in this file justifying why an
existing module cannot host the code, and what dependency-direction
invariant the new crate enforces (i.e. which leaf-→-root edge it
preserves, and which existing crate would have grown a cycle if the
code had gone there). A new crate that does not strengthen the
dependency direction is overhead; refactor within an existing module
instead.

## Tool architecture

`specify-tool` owns the declared WASI tool model, cache, resolver, and
Wasmtime-backed execution host. It is deliberately independent of
`specify-capability`: the binary resolves capabilities, then hands this
crate project-scope and capability-scope tool declarations.

- **Declaration sites.** Tools are declared at *project scope* (a
  top-level `tools:` array in `.specify/project.yaml`) and / or
  *capability scope* (a `tools.yaml` sidecar next to `capability.yaml`
  inside the resolved capability directory). Both shapes share
  `schemas/tool.schema.json`. `specify tool` merges by `name`, with
  project scope winning on collision and a typed `tool-name-collision`
  warning emitted once per session. `capability.yaml` itself is never
  modified and never gains a `tools:` field.
- **Cache layout.** The cache root resolves
  `$SPECIFY_TOOLS_CACHE` → `$XDG_CACHE_HOME/specify/tools/` →
  `$HOME/.cache/specify/tools/`. Within it, paths are
  `<scope-segment>/<tool-name>/<version>/{module.wasm,meta.yaml}` where
  `<scope-segment>` is `project--<project-name>` or
  `capability--<capability-slug>`. The `--` separator avoids collisions
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
  `$PROJECT_DIR` is always available; `$CAPABILITY_DIR` is available
  only to capability-scope tools — project-scope use is rejected as
  `tool.capability-dir-out-of-scope`. After substitution paths must be
  absolute, free of `..`, and canonicalise inside `PROJECT_DIR`
  (or `CAPABILITY_DIR` for capability-scope). `write:` entries that
  target Specify lifecycle state (`.specify/project.yaml`, slice /
  archive `.metadata.yaml`, `.specify/plan.lock`, etc.) are rejected.
- **Argument forwarding and environment.** `specify tool run <name>
  [-- <args>...]` forwards everything after `--` verbatim with
  `<name>` as `argv[0]`. The module receives exactly two environment
  variables — `PROJECT_DIR` always, `CAPABILITY_DIR` only for
  capability-scope tools — plus stdio. No host environment is
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
- **Time crate.** UTC-only domain; `jiff::Timestamp` replaces
  `chrono::DateTime<Utc>` across every host crate. All persisted
  stamps route through `specify_error::serde_rfc3339` so the on-disk
  wire shape stays `%Y-%m-%dT%H:%M:%SZ` byte-for-byte across both the
  domain DTOs and `Sidecar.fetched_at`. `system_time_to_utc` consolidates
  the previous three `Error::Diag` codes (`merge-mtime-pre-epoch`,
  `merge-mtime-overflow`, `merge-mtime-out-of-range`) into a single
  `merge-mtime-out-of-range` whose `detail` carries the underlying
  `jiff` error.

## Follow-up: wasm-pkg-client HTTP duplication

`wasm-pkg-client` (0.15) is wired in as a non-optional dep of
`specify-tool` and used at one site (`crates/tool/src/package.rs`)
behind the `PackageClient` trait. It wraps both `warg-client`
(WebAssembly registries) and `oci-client` / `oci-wasm` (OCI), so it
pulls *two* full HTTP stacks:

- `reqwest 0.12.28` (via `warg-client`) and `reqwest 0.13.3` (via
  `oci-client` / `oci-wasm`) — both used concurrently.
- `base64 0.21.7` and `base64 0.22.1`.
- `hyper-util 0.1.x`, `hyper-rustls 0.27`, `tower-http 0.6` shared
  across both reqwest versions.
- `rustls-platform-verifier`, `security-framework 3.x`, `keyring`,
  `oci-spec`, the `warg-*` family, `pbjson`, `prost-build`,
  `dialoguer`, `config`, `ron`, `ptree`, `ordered-multimap`, etc.

With `--no-default-features` on `specify-tool` (Wasmtime gated off),
the dep tree still contains roughly 344 unique crates — about 90% of
which are `wasm-pkg-client` transitives. Building with the default
`host` feature pushes that to ~380 unique crates. The duplicate
`reqwest`/`base64`/`hyper`/`tower-http` versions are why
`crates/tool/src/lib.rs` carries an `allow(clippy::multiple_crate_versions)`
waiver covering "Wasmtime, WASI, and `wasm-pkg-client`".

Realistic options, ranked by cost:

1. **Gate `wasm-pkg-client` behind a `wasm-pkg` feature** (smallest
   delta). Make the dep optional, fold the package-resolution path in
   `package.rs` behind `cfg(feature = "wasm-pkg")`, and let callers
   that only need manifest/cache/resolver/validator surface drop the
   ~344-crate tail. The host CLI keeps the feature on; downstream
   embedders and the `--no-default-features` path get a stub
   `WasmPkgClient` analogous to the existing `WasiRunner`
   "tool-host-not-built" stub. Recommended.
2. **Replace with direct `ureq` + minimal OCI client** (medium).
   `specify-tool` already depends on `ureq` v3. Implementing the
   subset of OCI / warg resolution the CLI actually exercises (one
   first-party registry, one OCI prefix, sha-streaming download)
   removes the entire `reqwest`/`hyper`/`tower-http` duplication.
   Cost: writing and maintaining registry-resolution code that
   `wasm-pkg-client` currently provides for free, plus losing
   compatibility with `WKG_CONFIG`-flavoured registry configs.
3. **Accept as-is.** Status quo. Already covered by the
   `multiple_crate_versions` allow.

Recommendation: option (1). It removes dependency weight from the
non-host build path (where the duplication is least justified) at
roughly the same cost as the existing `host` feature pattern, and
leaves option (2) on the table for a later pass if `wasm-pkg-client`
becomes the long-pole on host-feature build times or audit surface.
