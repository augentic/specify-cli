# `vectis validate` decisions log

Provenance and rationale for the deterministic validation engine. Each
entry quotes (or paraphrases) the RFC that grounds the rule and names
the call site(s) that implement it. Inline comments in `src/validate/engine/`
state the rules without RFC numbers; this file carries the citation.

## RFC-11 — Vectis UI artifact surface

The umbrella RFC for `tokens.yaml`, `assets.yaml`, `layout.yaml`,
`composition.yaml`, the embedded JSON Schemas, and the
`vectis validate` command surface.

### §A — Unwired-subset rule

> `layout.yaml` is the unwired subset of the patched composition
> schema: it MUST NOT use the `delta` shape, and it MUST NOT carry any
> define-owned wiring keys (`maps_to`, `bind`, `event`, `error`,
> overlay `trigger`, conditional visual `*-when` keys). Wiring is
> added by `/spec:define` when it produces `composition.yaml`. The
> bare `when:` (`stateEntry.when`) is part of the unwired subset and
> is preserved.

_Codified in: `crates/vectis/src/validate/engine/layout.rs::validate_layout`,
`walk_unwired`, and `forbidden_wiring_key`._

### §E — Resolution checks live in the input validation gate

> Cross-artifact resolution checks (file existence for raster /
> vector assets, per-platform source coverage for composition-
> referenced assets, vector-source `sources.<plat>` requirement, and
> raster optional-density warnings) all live in `vectis validate`
> rather than in downstream consumers. Density warnings only fire for
> composition-referenced assets so unreferenced manifest entries do
> not generate noise.

_Codified in:
`crates/vectis/src/validate/engine/assets.rs::validate_assets`,
`check_asset_files`, `check_platform_coverage`, and `check_file`._

### §F — V1 token-reference categories

> Composition-document keys map to `tokens.yaml` categories as
> follows: `color`, `background`, `border.color` →
> `colors.<name>`; `elevation` (groupProps) → `elevation.<name>`;
> string-valued `gap`, `padding`, `padding.<side>` →
> `spacing.<name>`; string-valued `corner_radius` →
> `cornerRadius.<name>`. `style`, `size.width`, and `size.height`
> are deliberately excluded from V1 reference resolution.

_Codified in:
`crates/vectis/src/validate/engine/composition.rs::resolve_token_references`,
`walk_token_refs`, `token_category_for_key`, and `check_token_ref`._

### §G — Structural-identity rule

> Every group carrying the same `component: <slug>` directive MUST
> share a single canonical skeleton across the document. Slug
> instances MAY differ in `bind`, `event`, `error`, asset / token
> references, `*-when` condition values, and free text content, but
> their group skeleton MUST match across all base instances.
> `*-when` *key presence* participates in skeleton identity even
> though *condition values* do not. Per-instance `platforms.*`
> overrides MAY diverge from the base skeleton (edge case 3) and
> are exempt from base-equality.

_Codified in:
`crates/vectis/src/validate/engine/composition.rs::check_structural_identity`,
`walk_for_components`, `build_group_skeleton`, `build_node_skeleton`,
plus the `Skeleton` and `ComponentInstance` types. Layout mode
reuses the same engine via `engine/layout.rs::validate_layout`._

### §H — CLI validation modes and default-path resolution

> When no `[path]` positional is supplied, each per-mode validator
> walks up from the current working directory looking for a
> `.specify/` ancestor and expands the canonical path cascade with
> `<name>` resolved against the alphabetically-first directory
> under `.specify/slices/`. Sibling discovery (assets →
> composition, composition → tokens / assets) routes through the
> same resolver. `validate all` fans out across `layout` →
> `composition` → `tokens` → `assets` and folds each per-mode
> envelope into a combined `{ "mode": "all", "results": [...] }`
> shape. Sub-modes whose default-resolved input is missing surface
> as a synthetic `{ skipped: true }` sub-report so the combined run
> does not bail. The dispatcher exits non-zero on errors, zero with
> a printed warning report on warnings, zero silently on a clean
> run.

_Codified in:
`crates/vectis/src/validate/engine/paths.rs::{resolve_default_path,
resolve_default_path_with_root, default_project_root,
discover_artifact, find_project_root, paths_for_key,
expand_path_template, EMBEDDED_ARTIFACT_PATHS}` and
`engine/all.rs::validate_all` (the `validate all` fan-out)._

### §I — Validation gate

> Composition mode auto-invokes sibling `tokens.yaml` and
> `assets.yaml` validators (in that order) when the files exist, and
> folds their per-mode envelopes into `results: [{ mode, report }]`.
> The fold shape matches `validate all` so the recursion-aware exit
> code helper picks up nested findings without extra plumbing.

_Codified in:
`crates/vectis/src/validate/engine/composition.rs::validate_composition`
(auto-invoke + cross-artifact resolution layer) and
`engine/mod.rs::run_inner` (the re-entrant dispatch helper)._

### Appendix A — embedded `tokens.schema.json`

> The embedded tokens schema is vendored from `specify` at
> `capabilities/vectis/tokens.schema.json`. The two copies stay in
> lock-step: the upstream is canonical and any edit there must be
> mirrored here byte-for-byte.

_Codified in:
`crates/vectis/src/validate/engine/shared.rs::TOKENS_SCHEMA_SOURCE`
and `tokens_validator`._

### Appendix B — embedded `assets.schema.json`

> The embedded assets schema is vendored from `specify` at
> `capabilities/vectis/assets.schema.json`. The order of platform
> densities (`1x`, `2x`, `3x` for iOS; `mdpi` … `xxxhdpi` for
> Android) matches the schema's `propertyNames` and is the order
> warnings render in. The same byte-identity discipline as the
> tokens copy applies.

_Codified in:
`crates/vectis/src/validate/engine/shared.rs::ASSETS_SCHEMA_SOURCE`,
`assets_validator`, and `engine/assets.rs::raster_densities`._

### Appendix C — example `layout.yaml`

> Pinned verbatim as the happy-path schema fixture; any future
> drift surfaces in the layout-mode test suite first.

_Codified in: `crates/vectis/tests/engine_layout.rs::APPENDIX_C_LAYOUT_YAML`._

### Appendix D — example `tokens.yaml`

> Pinned verbatim as the happy-path tokens schema fixture; any
> future drift surfaces in the tokens-mode test suite first.

_Codified in: `crates/vectis/tests/engine_tokens.rs::APPENDIX_D_TOKENS_YAML`._

### Appendix E — example `assets.yaml`

> Pinned verbatim as the happy-path assets schema fixture; any
> future drift surfaces in the assets-mode test suite first.

_Codified in: `crates/vectis/tests/engine_assets.rs::APPENDIX_E_ASSETS_YAML`._

### Appendix F — patched `composition.schema.json`

> The embedded composition schema is the upstream
> `capabilities/vectis/composition.schema.json` (in the `specify`
> repo) with the F-patch applied. The schema is shared between
> `layout` mode (unwired-subset runtime) and `composition` mode (full
> lifecycle runtime). The F.2 patch's `component.not.enum` rejects
> reserved slugs (`header`, `body`, `footer`, `fab`).

_Codified in:
`crates/vectis/src/validate/engine/shared.rs::COMPOSITION_SCHEMA_SOURCE`
and `composition_validator`. Reserved-slug rejection is exercised by
the layout- and composition-mode test suites under
`crates/vectis/tests/`._

### §J — Conservative directive emission

> The structural-identity validator only flags disagreement; it does
> not require ≥2 instances. A single `component:` instance passes
> silently because it has nothing to compare against.

_Codified in: `crates/vectis/src/validate/engine/composition.rs::check_structural_identity`
(early-exit when `base.len() < 2`)._

## RFC-7 — Wiring resolution rules

> `maps_to` / `bind` / `event` / overlay `trigger` / navigation
> target full resolution against `design.md` / `specs/` is deferred
> to a follow-on RFC. Composition mode's schema regex patterns
> (`bindValue`, `eventValue`, `triggerValue`) shape-check these
> fields at parse time; the runtime resolution layer is intentionally
> out of scope here. Phase 1.7's static-asset walker (the
> `image` / `icon` / `icon-button` / `fab` reference shape) is
> reused by composition mode for asset-id resolution.

_Codified in:
`crates/vectis/src/validate/engine/composition.rs::validate_composition`
(deliberate deferral note) and
`engine/assets.rs::collect_asset_references` (the shared walker
composition mode reuses)._

## RFC-16 — WASI command surface

> `vectis validate` is a WASI command tool. The library crate carries
> the deterministic engine and the embedded schemas so the WASI
> command surface has a single source of truth. The dispatcher
> renders a flat body with `mode`, `errors: [...]`, `warnings: [...]`,
> and (for `all` / auto-invoke) `results: [...]`, and exits non-zero
> only when a real sub-report carries errors.

_Codified in: `crates/vectis/src/validate.rs` (the public
`Args`, `ValidateMode`, `render_json`, and `validate_exit_code`
surface) and `src/main.rs` (the binary entry point)._

### Scaffold version-pin resolution

> `vectis scaffold` resolves Crux + toolchain pins from embedded
> defaults plus an optional explicit complete TOML override. It
> deliberately does not inspect project-local or user-local
> configuration, keeping the WASI command surface deterministic
> across hosts.

_Codified in: `crates/vectis/src/scaffold/versions.rs::Versions::resolve`,
`load_required`, and `load_embedded`._

## RFC 6901 — JSON Pointer

> Every error / warning entry carries a `path` field shaped like a
> JSON Pointer (the same `instance_path` the `jsonschema` crate
> reports for schema findings, and a hand-rolled equivalent for our
> own cross-artifact findings) so operators can locate the offending
> sub-document. Reference tokens are escaped per §3: `~` becomes
> `~0` and `/` becomes `~1`.

_Codified in:
`crates/vectis/src/validate/engine/shared.rs::escape_pointer_token`
and the path-construction call sites under `engine/assets.rs`,
`engine/layout.rs`, and `engine/composition.rs`._
