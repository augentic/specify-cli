# `vectis validate` decisions log

Provenance and rationale for the deterministic validation engine. Each
entry records the decision that grounds the rule and names
the call site(s) that implement it. Inline comments in `src/validate/engine/`
state the rules without historical labels; this file carries the citation.

## Vectis UI artifact surface

The umbrella decision for `tokens.yaml`, `assets.yaml`, `layout.yaml`,
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
> `adapters/vectis/tokens.schema.json`. The two copies stay in
> lock-step: the upstream is canonical and any edit there must be
> mirrored here byte-for-byte.

_Codified in:
`crates/vectis/src/validate/engine/shared.rs::TOKENS_SCHEMA_SOURCE`
and `tokens_validator`._

### Appendix B — embedded `assets.schema.json`

> The embedded assets schema is vendored from `specify` at
> `adapters/vectis/assets.schema.json`. The order of platform
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

_Codified in: `wasi-tools/vectis/tests/engine/layout.rs::APPENDIX_C_LAYOUT_YAML`._

### Appendix D — example `tokens.yaml`

> Pinned verbatim as the happy-path tokens schema fixture; any
> future drift surfaces in the tokens-mode test suite first.

_Codified in: `wasi-tools/vectis/tests/engine/tokens.rs::APPENDIX_D_TOKENS_YAML`._

### Appendix E — example `assets.yaml`

> Pinned verbatim as the happy-path assets schema fixture; any
> future drift surfaces in the assets-mode test suite first.

_Codified in: `wasi-tools/vectis/tests/engine/assets.rs::APPENDIX_E_ASSETS_YAML`._

### Appendix F — patched `composition.schema.json`

> The embedded composition schema is the upstream
> `adapters/vectis/composition.schema.json` (in the `specify`
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

## Wiring resolution rules

> `maps_to` / `bind` / `event` / overlay `trigger` / navigation
> target full resolution against `design.md` / `specs/` is deferred
> to a follow-on contract. Composition mode's schema regex patterns
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

## WASI command surface

> `vectis validate` is a WASI command tool. The library crate carries
> the deterministic engine and the embedded schemas so the WASI
> command surface has a single source of truth. The dispatcher
> renders a flat body with `mode`, `errors: [...]`, `warnings: [...]`,
> and (for `all` / auto-invoke) `results: [...]`, and exits non-zero
> only when a real sub-report carries errors.

_Codified in: `crates/vectis/src/validate.rs` (the public
`Args`, `ValidateMode`, `render_json`, and `validate_exit_code`
surface) and `src/main.rs` (the binary entry point)._

## RFC-46 — asset materialization

Canonical draft: [`augentic/specify` `rfcs/rfc-46-asset-materialization.md`](https://github.com/augentic/specify/blob/rfc-46/rfcs/rfc-46-asset-materialization.md).

### §K — Materialization and render-by-`kind`

> Canonical `source:` files under `design-system/assets/` are designer-owned.
> Per-platform exports under `design-system/assets/exports/<platform>/` are
> tool-generated or operator-pinned derivatives recorded in `sources.<platform>`.
> Consumer repos version-control committed `exports/`; CI does not require
> image-processing deps on every job.
>
> **`vectis materialize assets`** (Phase 2) converts canonical masters into
> per-platform exports and auto-writes absent `sources.<platform>` pins.
> Operator pins win silently — when `sources.<platform>` is set and the path
> exists on disk, materialize skips that slot. Invocation: operator manual, or
> automatic at `specify slice build --phase prepare` for in-scope missing
> exports (§2.1 of RFC-46).
>
> **Render-by-`kind`:** shell writers copy materialized exports into shell
> resources and emit view code by entry `kind` — `vector` / `raster` from shell
> catalogs, `symbol` only via explicit `symbols.<platform>` at the call site.
> Build-time substitution of platform glyphs for `vector` / `raster` ids is
> forbidden.

_Codified in: `wasi-tools/vectis/src/validate/engine/assets/` (export presence,
`assets-materialization-missing`, `assets-app-icon-*`); materialize subcommand
lands Phase 2 (`wasi-tools/vectis/src/materialize.rs`, planned)._

### §L — Bootstrap `app-icon` gate

> **Bootstrap trigger (§6.1).** For a Vectis-bound project, UI shell bootstrap
> is implied when `project.yaml.platforms` includes `ios` and/or `android` **and**
> vectis shell-detect semantics report that platform in `missing[]` (same rules
> as `vectis verify --mode detect` / `specify-vectis-shell-detect`). Detection
> is filesystem-authoritative — host `propose --from` and `plan validate` call
> the shared library in-process; they do not dispatch vectis WASM for this probe.
> **`plan.yaml` slice names** (`app-foundation`, `bootstrap-*`) are **not** gate
> inputs. `core`-only missing does not trigger; the gate fires only when `ios`
> or `android` is among `missing[]`.
>
> **Validation rule (§6.2).** When §6.1 triggers, evaluate each missing UI
> platform `π`: (1) **shell-resident escape hatch** — if
> `shell_resident_app_icon(project_dir, π)` is true (§6.3), pass for `π`
> without design-system inventory; (2) otherwise require `design-system/assets.yaml`
> top-level `app-icon` pointing at a `role: app-icon` entry satisfiable for `π`
> via path A (canonical `source:` materializable) or path B (operator-pinned
> export tree at `exports/<π>/app-icon/`). Failure → `plan-bootstrap-app-icon-missing`
> (plan validate) or `assets-app-icon-invalid` / `assets-app-icon-export-invalid`
> (vectis validate assets).
>
> When §6.1 does not trigger, `app-icon` inventory is not gated.

_Codified in: `crates/workflow/src/platform/bootstrap.rs` (`BootstrapContext`);
`crates/vectis-shell-detect` (`shell_resident_app_icon`); plan doctor
(`plan-bootstrap-app-icon-missing`); `wasi-tools/vectis/src/validate/engine/assets/app_icon.rs`._

### Scaffold version-pin resolution

> `vectis scaffold` resolves Crux + toolchain pins from embedded
> defaults plus an optional explicit complete TOML override. It
> deliberately does not inspect project-local or user-local
> configuration, keeping the WASI command surface deterministic
> across hosts.

_Codified in: `crates/vectis/src/scaffold/versions.rs::Versions::resolve`,
`load_required`, and `load_embedded`._

## JSON Pointer

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

## Verify subcommand

### §J — Platform shell verification

> `vectis verify` reads `project.yaml.platforms` as authority and
> inspects on-disk shell trees to determine which declared platforms
> are present. Only three platforms have on-disk interpretations
> today: `core` → `shared/src/app.rs`; `ios` → `iOS/` with ≥ 1
> `.swift` file; `android` → `Android/` with ≥ 1 `.kt` file.
> `web` and `desktop` are accepted but have no on-disk
> interpretation — they emit a `platform-not-yet-supported` info
> finding and are treated as present.
>
> Two modes: `detect` returns the missing set (plan-time bootstrap
> insertion, always exits 0); `verify` emits `diagnostic.schema.json`-
> shaped findings with `severity: error` for missing supported
> platforms and exits non-zero (1) on any miss. Both modes exit 2
> on runtime failures (missing `project.yaml`, parse errors).

_Codified in: `src/verify.rs` (`run`, `check_platform`,
`render_detect`, `render_verify`, `verify_exit_code`)._
