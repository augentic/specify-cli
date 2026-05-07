//! `vectis-validate <mode> [path]` -- schema and cross-artifact validation
//! surface (RFC-11 §H, §I).
//!
//! Phase 1.6 wired the `tokens` mode against the embedded
//! `schemas/vectis/tokens.schema.json` (Appendix A; vendored from the
//! `specify` repo at `crates/vectis-validate/embedded/tokens.schema.json`).
//! Phase 1.7 brings the `assets` mode online against the embedded
//! `schemas/vectis/assets.schema.json` (Appendix B), layering three
//! cross-artifact checks on top of the schema:
//!
//! 1. **File existence** -- every `filePath` in every raster /
//!    vector entry must resolve to a file under the directory
//!    containing `assets.yaml` (typically `design-system/assets/**`).
//!    Missing files are errors (RFC-11 §E "Resolution checks live in
//!    the input validation gate").
//! 2. **Composition reference resolution** -- when a sibling
//!    `composition.yaml` is found at the canonical paths from §H,
//!    every `image` / `icon` / `icon-button` / `fab` reference is
//!    resolved against the asset id set; unresolved refs become
//!    errors.
//! 3. **Per-platform source coverage** -- for raster + vector assets
//!    referenced by composition, both `sources.ios` and
//!    `sources.android` must be present (the formal "targeted shell
//!    platforms" wiring lands when the build brief invokes this mode
//!    in Phase 3.5; Phase 1.7 conservatively checks both platforms
//!    per the plan). Missing optional raster densities surface as
//!    warnings; a fully-missing platform surfaces as an error.
//!
//! Phase 1.8 brings the `layout` mode online against the patched
//! `schemas/vectis/composition.schema.json` (RFC-11 Appendix F;
//! vendored from the `specify` repo into
//! `crates/vectis-validate/embedded/composition.schema.json` -- the same
//! byte-identity discipline as the tokens / assets copies). The
//! mode performs three coordinated checks on top of YAML parsing:
//!
//! 1. **Schema validation** against the patched composition schema.
//! 2. **Unwired-subset enforcement** (RFC-11 §A) -- reject `delta`
//!    documents and any define-owned wiring keys (`maps_to`, `bind`,
//!    `event`, `error`, overlay `trigger`, conditional visual
//!    `*-when` keys). Define is the only writer for those keys; a
//!    layout document MUST stay flat-shape until `/spec:define`
//!    promotes it to `composition.yaml`.
//! 3. **Structural-identity** (RFC-11 §G) -- every group carrying a
//!    `component: <slug>` directive must share a single canonical
//!    skeleton across the document. The engine itself is generic and
//!    Phase 1.9 reuses it for `composition` mode (see
//!    `check_structural_identity`); the engine consumes
//!    `*-when`-key *presence* (forbidden in layout, allowed in
//!    composition) but ignores `*-when` *condition values* per §G's
//!    edge-case enumeration.
//!
//! Phase 1.9 brings `composition` mode online -- the lifecycle
//! artifact form (`screens` for baseline, `delta` for slice-local).
//! On top of YAML parsing the mode performs:
//!
//! 1. **Schema validation** against the same vendored composition
//!    schema (shared with `layout` mode -- one schema, two runtime
//!    layers).
//! 2. **Structural-identity** (RFC-11 §G) -- shared engine
//!    (`check_structural_identity`) with `layout` mode. Both
//!    `screens` and `delta` shapes are walked; instances inside
//!    `delta.added` and `delta.modified` participate in identity
//!    checks together (a slug introduced in `added` must agree with
//!    a slug modified in `modified`).
//! 3. **Auto-invoke** (RFC-11 §H "CLI validation modes" + §I
//!    "Validation gate") -- when a sibling `tokens.yaml` exists, run
//!    `validate tokens` and fold its envelope into `results: [{ mode,
//!    report }]`; same for `assets.yaml`. The folding shape matches
//!    what Phase 1.10's `validate all` will emit so the dispatcher's
//!    `validate_exit_code` (recursion-aware since Phase 1.6) needs no
//!    changes.
//! 4. **Cross-artifact reference resolution** -- when sibling
//!    `tokens.yaml` is present, every typed token reference in the
//!    composition (`color`, `background`, `border.color`, `elevation`)
//!    plus every string-valued spacing / corner-radius reference
//!    (`gap`, `padding`, `padding.<side>`, `corner_radius`) is
//!    resolved against the manifest's category. Unknown ids become
//!    composition-mode errors with JSON-Pointer-shaped paths. When
//!    sibling `assets.yaml` is present, the same `image` / `icon` /
//!    `icon-button` / `fab` walker Phase 1.7 introduced is reused to
//!    resolve static asset references against the manifest's id set.
//!
//! Composition mode deliberately defers full resolution of
//! `maps_to` / `bind` / `event` / overlay `trigger` / navigation
//! target references. The plan-§1.9 note ("RFC-7 already specifies
//! the field/event/ViewModel/overlay/navigation coverage rules; this
//! phase carries them forward through whatever helper RFC-7 left in
//! place") points at a helper that does not exist in the CLI today
//! -- those rules are design.md / specs/-driven and require a richer
//! project-wide context than `validate composition` has. The schema
//! patterns (`bindValue`, `eventValue`, `triggerValue`) shape-check
//! these fields at parse time; the runtime resolution layer is left
//! for a follow-on RFC.
//!
//! Phase 1.10 brings `all` mode online and lands the
//! `artifacts:`-block default-path resolver every mode shares:
//!
//! 1. **Default-path resolution** (RFC-11 §H field semantics) --
//!    when no `[path]` is supplied, walk up from CWD looking for a
//!    `.specify/` directory and expand the canonical Vectis path
//!    cascade (with `<name>` substituted from the alphabetically-first
//!    slice directory under `.specify/slices/`). Older projects that
//!    still carry an `artifacts:` block in a vendored `schema.yaml`
//!    continue to work, but post-RFC-13 capability manifests use the
//!    embedded canonical mapping.
//!    The helper is shared with the cross-artifact discovery layer
//!    Phase 1.7 (assets → composition) and Phase 1.9 (composition →
//!    tokens / assets) introduced; the previous `find_sibling_*`
//!    walks are now thin wrappers around the unified resolver.
//! 2. **`validate all`** (RFC-11 §H closing paragraph) -- runs
//!    `layout`, `composition`, `tokens`, `assets` against a project
//!    root (the optional `[path]` positional, defaulting to CWD)
//!    and folds the per-mode envelopes into
//!    `{ "mode": "all", "results": [{ "mode", "report" }, ...] }`.
//!    Sub-modes whose default-resolved input does not exist on disk
//!    are surfaced as a synthetic `{ skipped: true }` report rather
//!    than a hard `InvalidProject` error so the combined run does
//!    not bail when (e.g.) a project has no `tokens.yaml` yet. The
//!    dispatcher's `validate_exit_code` recurses through
//!    `results[*].report` (since Phase 1.6) and exits 1 when any
//!    sub-report carries errors.
//!
//! ## Per-mode envelope
//!
//! Phase 1.5 fixed the JSON shape every mode populates so the
//! dispatcher's `render_validate_text` and exit-code helper can stay
//! mode-agnostic:
//!
//! ```json
//! {
//!   "mode": "assets",
//!   "path": "design-system/assets.yaml",
//!   "errors":   [{ "path": "/assets/foo/sources/ios/1x", "message": "..." }],
//!   "warnings": [{ "path": "/assets/foo/sources/android", "message": "..." }]
//! }
//! ```
//!
//! Errors / warnings entries carry a JSON Pointer-shaped `path` (the
//! same `instance_path` jsonschema reports for schema findings, and
//! a hand-rolled equivalent for our own cross-artifact findings) so
//! operators can find the offending sub-document quickly. The
//! command renderer translates
//! `errors.is_empty() -> exit 0` and `errors.non_empty -> exit 1`
//! per RFC-11 §H ("non-zero on errors, zero with a printed warning
//! report on warnings, zero silently on a clean run").

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::{Value, json};

use crate::error::VectisError;
use crate::{CommandOutcome, ValidateArgs, ValidateMode};

/// Embedded `tokens.schema.json` (RFC-11 Appendix A). Vendored from
/// the `specify` repo at `schemas/vectis/tokens.schema.json` (Phase
/// 1.1). Keep the two files in lock-step -- the upstream copy is the
/// source of truth and any edit there must be mirrored here so the
/// CLI validator and the on-disk schema agree.
const TOKENS_SCHEMA_SOURCE: &str = include_str!("../embedded/tokens.schema.json");

/// Embedded `assets.schema.json` (RFC-11 Appendix B). Vendored from
/// the `specify` repo at `schemas/vectis/assets.schema.json` (Phase
/// 1.2). Same byte-identity discipline as the tokens copy: the
/// upstream is canonical and any edit there must be mirrored here.
const ASSETS_SCHEMA_SOURCE: &str = include_str!("../embedded/assets.schema.json");

/// Embedded `composition.schema.json` (RFC-11 Appendix F-patched).
/// Vendored from the `specify` repo at
/// `schemas/vectis/composition.schema.json` (Phase 1.3). Same
/// byte-identity discipline as the tokens / assets copies: upstream
/// is canonical and any edit there must be mirrored here. Shared
/// between `layout` mode (Phase 1.8, this file) and `composition`
/// mode (Phase 1.9) -- both validate against the same JSON Schema;
/// the difference is the runtime layer (`layout` enforces the
/// unwired subset, `composition` resolves wiring + cross-artifact
/// references).
const COMPOSITION_SCHEMA_SOURCE: &str = include_str!("../embedded/composition.schema.json");

/// Embedded default paths for the four `vectis-validate` modes. This
/// is the post-RFC-13 canonical Vectis cascade: slice-local files
/// first, then project-level inputs or the merged composition baseline.
/// Older projects with an on-disk `schema.yaml` `artifacts:` block can
/// still override these paths via [`read_artifacts_block`].
///
/// The order of the inner array is the resolution order; the first
/// existing file wins. The role label (first tuple element) is
/// retained for parity with the schema YAML even though Phase 1.10
/// only uses the template strings. The `<name>` placeholder is
/// expanded against `.specify/slices/<dir>/` (alphabetical first
/// match) at resolution time.
const EMBEDDED_ARTIFACT_PATHS: &[(&str, &[(&str, &str)])] = &[
    (
        "layout",
        &[
            ("change_local", ".specify/slices/<name>/layout.yaml"),
            ("project", "design-system/layout.yaml"),
        ],
    ),
    (
        "tokens",
        &[
            ("change_local", ".specify/slices/<name>/tokens.yaml"),
            ("project", "design-system/tokens.yaml"),
        ],
    ),
    (
        "assets",
        &[
            ("change_local", ".specify/slices/<name>/assets.yaml"),
            ("project", "design-system/assets.yaml"),
        ],
    ),
    (
        "composition",
        &[
            ("change_local", ".specify/slices/<name>/composition.yaml"),
            ("baseline", ".specify/specs/composition.yaml"),
        ],
    ),
];

/// Lazily compiled tokens validator. Compiling once per process avoids
/// re-parsing the embedded schema on every invocation; in practice the
/// CLI runs one mode per process today, but Phase 1.10's `validate
/// all` will fan out and exercise every mode in a single dispatch.
static TOKENS_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Lazily compiled assets validator (companion to [`TOKENS_VALIDATOR`]).
static ASSETS_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Lazily compiled composition validator (sister of
/// [`TOKENS_VALIDATOR`] / [`ASSETS_VALIDATOR`]). Shared between
/// `layout` mode (Phase 1.8) and `composition` mode (Phase 1.9): one
/// schema, two runtime layers on top.
static COMPOSITION_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Dispatch a `vectis validate` invocation to the per-mode handler.
///
/// Phases 1.6 / 1.7 / 1.8 / 1.9 implement `tokens` / `assets` /
/// `layout` / `composition`; Phase 1.10 lands `all`. Every mode
/// returns [`CommandOutcome::Success`] now -- the
/// [`CommandOutcome::Stub`] arm Phase 1.5 wired is unreachable and
/// retained only to keep the contract surface forward-compatible.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved
/// `tokens.yaml` / `assets.yaml` / `layout.yaml` / `composition.yaml`
/// is unreadable in single-mode runs (missing file, permission
/// denied; `validate all` instead surfaces the missing input as a
/// synthetic `skipped: true` sub-report) and [`VectisError::Internal`]
/// if an embedded schema fails to compile (a build-time invariant
/// violation -- all three schemas ship with the binary). YAML parse
/// failures and schema validation failures are *not* errors at this
/// layer; they are folded into the `errors` array of the per-mode
/// envelope so the operator sees the full report alongside any other
/// findings.
pub fn run(args: &ValidateArgs) -> Result<CommandOutcome, VectisError> {
    match args.mode {
        ValidateMode::Tokens => validate_tokens(args.path.as_deref()),
        ValidateMode::Assets => validate_assets(args.path.as_deref()),
        ValidateMode::Layout => validate_layout(args.path.as_deref()),
        ValidateMode::Composition => validate_composition(args.path.as_deref()),
        ValidateMode::All => validate_all(args.path.as_deref()),
    }
}

/// Validate `tokens.yaml` against the embedded Appendix A schema.
///
/// Resolution order for the file path (Phase 1.10):
/// 1. The explicit `[path]` positional, when supplied.
/// 2. The first existing file in
///    `artifacts.tokens.paths.{change_local, project}` (with
///    `<name>` expanded against the alphabetically-first directory
///    under `.specify/slices/`). The on-disk
///    `<root>/.specify/.cache/<schema>/schema.yaml` (or
///    `<root>/schemas/<schema>/schema.yaml`) wins; without one, the
///    embedded defaults from [`EMBEDDED_ARTIFACT_PATHS`] mirror the
///    same paths.
/// 3. The last candidate template (`design-system/tokens.yaml`)
///    when nothing exists, so the read error names the most
///    operator-friendly path.
fn validate_tokens(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target = path
        .map_or_else(|| resolve_default_path(ValidateMode::Tokens), std::path::Path::to_path_buf);

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("tokens.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => {
            let validator = tokens_validator()?;
            for err in validator.iter_errors(&instance) {
                errors.push(json!({
                    "path": err.instance_path().to_string(),
                    "message": err.to_string(),
                }));
            }
        }
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
        }
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::Tokens.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        // Tokens validation has no warning class today (Appendix A is
        // pure structural shape). The `assets` mode introduces warnings
        // (missing optional densities) and Phase 1.8's `layout` mode
        // adds candidate-component flags; the array stays here so the
        // envelope shape is uniform across modes.
        "warnings": Vec::<Value>::new(),
    })))
}

/// Validate `assets.yaml` against the embedded Appendix B schema and
/// layer the cross-artifact checks RFC-11 §E demands.
///
/// Resolution order for the file path mirrors [`validate_tokens`]:
/// the explicit `[path]` positional wins, otherwise the
/// `artifacts.assets.paths` cascade (`change_local` → `project`)
/// resolves the default; nothing-exists falls back to
/// `design-system/assets.yaml`.
///
/// On top of schema validation the function performs three
/// cross-artifact checks:
///
/// 1. **File existence**: every raster density entry, every vector
///    `source`, and every vector `sources.<platform>` is resolved
///    relative to the directory containing `assets.yaml`. Missing
///    files become errors with a JSON-Pointer-shaped `path` that
///    points at the offending sub-document, e.g.
///    `/assets/empty-tasks-hero/sources/ios/1x`.
/// 2. **Composition discovery**: walk up from `assets.yaml`'s
///    parent until a project root marked by `.specify/` is found,
///    then look at `.specify/slices/<name>/composition.yaml`
///    (alphabetical first match) before `.specify/specs/composition.yaml`.
///    The first existing path wins. If no sibling composition is
///    found, the cross-artifact checks below are skipped silently.
///    Phase 1.10 will replace this walk with the `artifacts:`-block
///    cascade.
/// 3. **Cross-artifact reference checks**: every `image`, `icon`,
///    `icon-button`, and `fab` asset reference in the discovered
///    composition is resolved against the asset id set. Unknown ids
///    become errors. For raster + vector assets that ARE referenced,
///    both `sources.ios` and `sources.android` must be present
///    (missing platform = error); raster assets surface a warning
///    per missing optional density slot when the platform itself is
///    populated.
fn validate_assets(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target = path
        .map_or_else(|| resolve_default_path(ValidateMode::Assets), std::path::Path::to_path_buf);

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("assets.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();

    let instance = match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => Some(instance),
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
            None
        }
    };

    if let Some(instance) = instance.as_ref() {
        let validator = assets_validator()?;
        for err in validator.iter_errors(instance) {
            errors.push(json!({
                "path": err.instance_path().to_string(),
                "message": err.to_string(),
            }));
        }

        let assets_dir = target.parent().unwrap_or_else(|| Path::new("."));

        // File-existence checks always run, regardless of whether a
        // sibling composition exists. An asset can validly carry
        // dangling files even when no composition references it (yet)
        // -- but the operator should know.
        if let Some(assets) = instance.get("assets").and_then(Value::as_object) {
            for (id, entry) in assets {
                check_asset_files(id, entry, assets_dir, &mut errors);
            }
        }

        // Cross-artifact composition reference resolution. Phase
        // 1.10 routes this through the unified
        // [`discover_artifact`] helper which reads the project's
        // on-disk `schema.yaml` `artifacts:` block when present and
        // otherwise falls back to the embedded defaults that mirror
        // `schemas/vectis/schema.yaml` v2.
        if let Some(comp_path) = discover_artifact(&target, ValidateMode::Composition)
            && let Some(comp_value) = parse_yaml_file(&comp_path)
        {
            let assets_map = instance.get("assets").and_then(Value::as_object);
            let refs = collect_asset_references(&comp_value);
            for asset_ref in &refs {
                let entry = assets_map.and_then(|m| m.get(&asset_ref.id));
                if entry.is_none() {
                    errors.push(json!({
                        "path": asset_ref.path,
                        "message": format!(
                            "composition.yaml at {} references unknown asset id `{}`",
                            comp_path.display(),
                            asset_ref.id,
                        ),
                    }));
                    continue;
                }
                let Some(entry) = entry else {
                    continue;
                };
                check_platform_coverage(&asset_ref.id, entry, &mut errors, &mut warnings);
            }
        }
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::Assets.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        "warnings": warnings,
    })))
}

/// Validate `layout.yaml` as the unwired subset of the patched
/// composition schema (RFC-11 §A, §G, §H "`layout` mode", Appendix
/// F).
///
/// Resolution order for the file path mirrors [`validate_tokens`]
/// and [`validate_assets`]: the explicit `[path]` positional wins,
/// otherwise the `artifacts.layout.paths` cascade (`change_local` →
/// `project`) resolves the default; nothing-exists falls back to
/// `design-system/layout.yaml`.
///
/// The mode performs three checks:
///
/// 1. **Schema validation** against the embedded composition schema
///    (Phase 1.3-patched). The schema permits both `screens` and
///    `delta` shapes; the unwired-subset check below rejects
///    `delta`-shaped layout documents.
/// 2. **Unwired-subset enforcement** -- reject `delta:` and any
///    occurrence of define-owned wiring keys (`maps_to`, `bind`,
///    `event`, `error`, overlay `trigger`, conditional visual
///    `*-when` keys). The walker descends only the `screens`
///    sub-tree (the only place where wiring keys can appear in a
///    valid composition document); other top-level keys
///    (`provenance`, `version`, `custom_items`) carry no wiring.
///    Bare `when:` (the required `stateEntry.when` from the schema)
///    is *not* a `*-when` key and is preserved.
/// 3. **Structural-identity** for `component:` directives -- every
///    group carrying the same `component: <slug>` MUST share the
///    same skeleton (RFC-11 §G). The engine ignores leaf wiring
///    values (`bind`, `event`, `error`, free text content, token /
///    asset references) and `*-when` *condition values*, but is
///    sensitive to `*-when` key *presence*. Per-instance
///    `platforms.*` overrides are exempt from base-skeleton match
///    per §G's third edge case.
fn validate_layout(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target = path
        .map_or_else(|| resolve_default_path(ValidateMode::Layout), std::path::Path::to_path_buf);

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("layout.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    let warnings: Vec<Value> = Vec::new();

    match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => {
            let validator = composition_validator()?;
            for err in validator.iter_errors(&instance) {
                errors.push(json!({
                    "path": err.instance_path().to_string(),
                    "message": err.to_string(),
                }));
            }

            // Reject `delta:` documents at the top level. The
            // schema's `oneOf` permits either `screens` or `delta`;
            // layout.yaml is restricted to the `screens` half.
            if instance.get("delta").is_some() {
                errors.push(json!({
                    "path": "/delta",
                    "message": "layout.yaml MUST NOT use the `delta` shape (RFC-11 §A unwired-subset rule); only `screens` documents are permitted. Use composition.yaml for change-local delta artifacts.",
                }));
            }

            // Walk the `screens` sub-tree for forbidden wiring keys
            // and `component:` directive instances. Both walks are
            // scoped to `screens` because (a) other top-level keys
            // never carry wiring per the schema, and (b) keeping
            // the scope tight avoids descending into a `delta:`
            // sub-tree (which would surface noisy redundant
            // wiring-key errors after we've already rejected the
            // shape itself).
            if let Some(screens) = instance.get("screens") {
                walk_unwired(screens, "/screens", &mut errors);
                check_structural_identity(screens, "/screens", &mut errors);
            }
        }
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
        }
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::Layout.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        "warnings": warnings,
    })))
}

/// Validate `composition.yaml` as the lifecycle artifact (RFC-11 §G,
/// §H "`composition` mode", §I "Validation gate").
///
/// Resolution order for the file path mirrors the other modes:
/// the explicit `[path]` positional wins, otherwise the
/// `artifacts.composition.paths` cascade (`change_local` →
/// `baseline`) resolves the default; nothing-exists falls back to
/// `.specify/specs/composition.yaml`.
///
/// The mode performs four checks:
///
/// 1. **Schema validation** against the embedded composition schema
///    (shared with `layout` mode -- one schema, two runtime
///    layers).
/// 2. **Structural-identity** for `component:` directives (RFC-11
///    §G), reusing Phase 1.8's [`check_structural_identity`]
///    engine. The walk covers both `screens` (baseline shape) and
///    `delta.added` / `delta.modified` (change-local shape) so
///    instances introduced or modified in a delta participate in
///    identity checks together.
/// 3. **Auto-invoke** sibling `tokens.yaml` / `assets.yaml` modes
///    when the files exist; their envelopes are folded into
///    `results: [{ mode, report }]` (the same shape Phase 1.10's
///    `validate all` will emit).
/// 4. **Cross-artifact reference resolution** -- token references
///    (`color`, `background`, `border.color`, `elevation`, plus
///    string-valued `gap` / `padding` / `padding.<side>` /
///    `corner_radius`) and asset references (`image.name`,
///    `icon.name`, `icon-button.icon`, `fab.icon`) are resolved
///    against the discovered manifests' id sets. Unresolved
///    references become composition-mode errors with
///    JSON-Pointer-shaped paths.
///
/// `maps_to` / `bind` / `event` / overlay `trigger` / navigation
/// target full resolution is deferred -- the schema's regex
/// patterns shape-check these fields at parse time, but resolution
/// against `design.md` / `specs/` belongs to a follow-on RFC.
fn validate_composition(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target = path.map_or_else(
        || resolve_default_path(ValidateMode::Composition),
        std::path::Path::to_path_buf,
    );

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("composition.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    // Composition mode has no warning class in v1 -- reference
    // mismatches are errors and structural-identity divergence is
    // an error. The empty `warnings` array stays in the envelope so
    // the shape matches the other modes; if a future phase
    // introduces a soft-finding (e.g. operator-flagged
    // candidate-component comments) it can push here without
    // disturbing the envelope contract.
    let warnings: Vec<Value> = Vec::new();
    let mut results: Vec<Value> = Vec::new();

    match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => {
            let validator = composition_validator()?;
            for err in validator.iter_errors(&instance) {
                errors.push(json!({
                    "path": err.instance_path().to_string(),
                    "message": err.to_string(),
                }));
            }

            // Structural identity walks both shapes. The schema's
            // `oneOf` ensures only one of `screens` / `delta` is
            // present at a time; the `if let` guards keep the call
            // site shape-agnostic so a malformed document
            // (`oneOf`-rejected by the validator, but still loaded)
            // doesn't trip a NPE here.
            if let Some(screens) = instance.get("screens") {
                check_structural_identity(screens, "/screens", &mut errors);
            }
            if let Some(delta) = instance.get("delta") {
                // The walker descends into `delta.added`,
                // `delta.modified`, and `delta.removed` -- collecting
                // every `component:` directive into a single instance
                // list so a slug that appears in both `added` and
                // `modified` is checked for skeleton agreement.
                check_structural_identity(delta, "/delta", &mut errors);
            }

            // Sibling discovery + auto-invoke. Both helpers are
            // ordered: `tokens` before `assets` so the envelope's
            // `results` array matches the dispatch order operators
            // see in `vectis validate all` (Phase 1.10). Both sites
            // resolve through the unified [`discover_artifact`]
            // helper Phase 1.10 introduced; the on-disk
            // `artifacts:` block (if any) wins, embedded defaults
            // otherwise.
            let tokens_sibling = discover_artifact(&target, ValidateMode::Tokens);
            let assets_sibling = discover_artifact(&target, ValidateMode::Assets);

            if let Some(ref tokens_path) = tokens_sibling {
                let report = run_inner(ValidateMode::Tokens, tokens_path)?;
                results.push(json!({
                    "mode": ValidateMode::Tokens.as_str(),
                    "report": report,
                }));
            }
            if let Some(ref assets_path) = assets_sibling {
                let report = run_inner(ValidateMode::Assets, assets_path)?;
                results.push(json!({
                    "mode": ValidateMode::Assets.as_str(),
                    "report": report,
                }));
            }

            // Cross-artifact reference resolution. Token / asset
            // walks run against the *content* of the sibling
            // manifests, separately from the auto-invoked
            // structural validation above. This is the layer that
            // catches "composition references a name that does not
            // exist in tokens.yaml / assets.yaml" -- the auto-invoke
            // catches "tokens.yaml / assets.yaml is itself
            // structurally broken".
            if let Some(ref tokens_path) = tokens_sibling
                && let Some(tokens_value) = parse_yaml_file(tokens_path)
            {
                resolve_token_references(&instance, &tokens_value, &mut errors);
            }
            if let Some(ref assets_path) = assets_sibling
                && let Some(assets_value) = parse_yaml_file(assets_path)
            {
                resolve_asset_references(&instance, &assets_value, &mut errors);
            }
        }
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
        }
    }

    let mut envelope = json!({
        "mode": ValidateMode::Composition.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        "warnings": warnings,
    });
    // Only emit `results` when we actually folded something in.
    // The `validate all` envelope (Phase 1.10) ALWAYS carries a
    // `results` array; the per-mode envelope keeps it optional so
    // operators of pure-composition runs see a clean shape.
    if !results.is_empty()
        && let Value::Object(ref mut map) = envelope
    {
        map.insert("results".to_string(), Value::Array(results));
    }

    Ok(CommandOutcome::Success(envelope))
}

/// Re-enter [`run`] for the auto-invoke path -- runs the named
/// sub-mode against the supplied path and returns its envelope (the
/// `Value` inside [`CommandOutcome::Success`]).
///
/// Used by `composition` mode (Phase 1.9) to fold sibling `tokens` /
/// `assets` envelopes into its own report. Phase 1.10's `all` mode
/// will use the same helper to dispatch each sub-mode in turn.
///
/// A [`CommandOutcome::Stub`] from a sub-mode is treated as an
/// invariant breach today (every mode `composition` auto-invokes is
/// already wired) and surfaces as [`VectisError::Internal`] so the
/// caller sees a clean failure rather than a silently-empty report.
fn run_inner(mode: ValidateMode, path: &Path) -> Result<Value, VectisError> {
    let inner_args = ValidateArgs {
        mode,
        path: Some(path.to_path_buf()),
    };
    match run(&inner_args)? {
        CommandOutcome::Success(value) => Ok(value),
        CommandOutcome::Stub { command } => Err(VectisError::Internal {
            message: format!(
                "auto-invoke folded a stub sub-mode `{command}`; this should never happen now that Phases 1.6/1.7 have landed",
            ),
        }),
    }
}

/// Read `path` and parse it as YAML into a [`serde_json::Value`].
///
/// The `Option<Value>` return shape is intentional -- we only call
/// this from inside `validate_composition` after we've already
/// loaded the manifest through `validate_tokens` /
/// `validate_assets`, so the auto-invoked envelope already carries
/// any read / parse findings. This helper just gets at the
/// *content* for the reference-resolution pass; if it fails for any
/// reason, the caller silently skips ref resolution against that
/// manifest (the auto-invoke envelope will already point at the
/// problem). Returning `None` lets the call site stay flat with
/// `if let Some(...)` instead of dragging a synthetic error type
/// through.
fn parse_yaml_file(path: &Path) -> Option<Value> {
    let source = std::fs::read_to_string(path).ok()?;
    serde_saphyr::from_str::<Value>(&source).ok()
}

/// Compile the embedded tokens schema once and re-use the
/// [`Validator`] for every invocation in this process.
///
/// Returns [`VectisError::Internal`] if the embedded JSON is
/// unparseable or the schema fails to compile -- both build-time
/// invariants enforced by `make checks` over the upstream
/// `tokens.schema.json`.
fn tokens_validator() -> Result<&'static Validator, VectisError> {
    lazy_validator(&TOKENS_VALIDATOR, TOKENS_SCHEMA_SOURCE, "tokens.schema.json")
}

/// Compile the embedded assets schema once and re-use the
/// [`Validator`] for every invocation in this process. Sister of
/// [`tokens_validator`]; same build-time invariants apply.
fn assets_validator() -> Result<&'static Validator, VectisError> {
    lazy_validator(&ASSETS_VALIDATOR, ASSETS_SCHEMA_SOURCE, "assets.schema.json")
}

/// Compile the embedded composition schema once and re-use the
/// [`Validator`] for every invocation in this process. Sister of
/// [`tokens_validator`] / [`assets_validator`]; same build-time
/// invariants apply. Shared between `layout` mode (Phase 1.8) and
/// `composition` mode (Phase 1.9).
fn composition_validator() -> Result<&'static Validator, VectisError> {
    lazy_validator(&COMPOSITION_VALIDATOR, COMPOSITION_SCHEMA_SOURCE, "composition.schema.json")
}

/// Generic helper for the embedded-schema lazy-compile pattern shared
/// across `validate <mode>` handlers. Phases 1.8 / 1.9 will reuse
/// this helper for `composition.schema.json`.
///
/// The cell stores `Result<Validator, String>` so a build-time
/// invariant breach (the embedded JSON is unparseable, or the schema
/// itself is invalid) survives across `OnceLock` initialisation
/// without re-running the failing branch on every call.
fn lazy_validator(
    cell: &'static OnceLock<Result<Validator, String>>, source: &'static str, name: &'static str,
) -> Result<&'static Validator, VectisError> {
    let entry = cell.get_or_init(|| {
        let schema: Value = serde_json::from_str(source)
            .map_err(|err| format!("embedded {name} is not JSON: {err}"))?;
        jsonschema::validator_for(&schema)
            .map_err(|err| format!("embedded {name} failed to compile: {err}"))
    });
    match entry {
        Ok(validator) => Ok(validator),
        Err(message) => Err(VectisError::Internal {
            message: message.clone(),
        }),
    }
}

/// Walk a single asset entry's filePaths and append a "file not
/// found" error for each one that does not resolve to a regular file
/// under `dir`. Symbol assets carry no filePaths so they are a
/// no-op here. Schema-invalid entries (missing or non-string `kind`,
/// non-object `sources`, etc.) are skipped silently because the
/// schema validator already reported them; this function is a
/// best-effort second pass over what the schema accepts.
fn check_asset_files(id: &str, entry: &Value, dir: &Path, errors: &mut Vec<Value>) {
    let Some(kind) = entry.get("kind").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "raster" => {
            for plat in PLATFORMS {
                let densities =
                    entry.get("sources").and_then(|s| s.get(plat)).and_then(Value::as_object);
                if let Some(map) = densities {
                    for (density, value) in map {
                        if let Some(file) = value.as_str() {
                            check_file(
                                &format!("/assets/{id}/sources/{plat}/{density}"),
                                file,
                                dir,
                                errors,
                            );
                        }
                    }
                }
            }
        }
        "vector" => {
            if let Some(file) = entry.get("source").and_then(Value::as_str) {
                check_file(&format!("/assets/{id}/source"), file, dir, errors);
            }
            for plat in PLATFORMS {
                if let Some(file) =
                    entry.get("sources").and_then(|s| s.get(plat)).and_then(Value::as_str)
                {
                    check_file(&format!("/assets/{id}/sources/{plat}"), file, dir, errors);
                }
            }
        }
        // `symbol` and any future variants: no filePaths to verify.
        _ => {}
    }
}

/// Resolve `file_rel` (a path relative to the directory containing
/// `assets.yaml`) and append an error to `errors` when the path does
/// not exist on disk or is not a regular file.
fn check_file(json_path: &str, file_rel: &str, dir: &Path, errors: &mut Vec<Value>) {
    let resolved = dir.join(file_rel);
    if !resolved.is_file() {
        errors.push(json!({
            "path": json_path,
            "message": format!("file not found: {}", resolved.display()),
        }));
    }
}

/// Per-platform source coverage for a composition-referenced asset
/// (RFC-11 §E "Resolution checks live in the input validation
/// gate"). For v1 we conservatively check both `ios` and `android`
/// per the Phase 1.7 plan note ("for v1 just check both `ios` and
/// `android` if the platform is plausibly present"); the formal
/// "targeted shell platforms" wiring (driven by the proposal
/// `Platforms` field) lands when the build brief invokes this mode
/// in Phase 3.5.
///
/// - **Raster**: `sources.<plat>` must be present (else "no usable
///   source" → error). When present, every density slot the schema
///   recognises but the entry omits is a warning.
/// - **Vector**: `sources.<plat>` (a single filePath) must be
///   present; the canonical `source` does not satisfy a per-platform
///   reference per RFC-11 §E "Vector support".
/// - **Symbol**: the schema already requires `symbols.<plat>` to be
///   non-empty when present, but the schema permits an entry with
///   just `symbols.ios` (or just `symbols.android`). Phase 1.7 does
///   NOT enforce per-platform symbol coverage; that lives in Phase
///   1.9's composition-mode (which has the full proposal context to
///   know which platforms are targeted). Symbol references that
///   resolve to a known asset id pass here.
fn check_platform_coverage(
    id: &str, entry: &Value, errors: &mut Vec<Value>, warnings: &mut Vec<Value>,
) {
    let Some(kind) = entry.get("kind").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "raster" => {
            for plat in PLATFORMS {
                let plat_node = entry.get("sources").and_then(|s| s.get(plat));
                let Some(plat_node) = plat_node else {
                    errors.push(json!({
                        "path": format!("/assets/{id}/sources/{plat}"),
                        "message": format!(
                            "raster asset `{id}` is referenced by composition.yaml but has no `sources.{plat}` source for the targeted shell platform"
                        ),
                    }));
                    continue;
                };
                if let Some(map) = plat_node.as_object() {
                    for &density in raster_densities(plat) {
                        if !map.contains_key(density) {
                            warnings.push(json!({
                                "path": format!("/assets/{id}/sources/{plat}"),
                                "message": format!(
                                    "raster asset `{id}` is missing optional `{density}` density for {plat}"
                                ),
                            }));
                        }
                    }
                }
            }
        }
        "vector" => {
            for plat in PLATFORMS {
                let plat_node = entry.get("sources").and_then(|s| s.get(plat));
                if plat_node.is_none() {
                    errors.push(json!({
                        "path": format!("/assets/{id}/sources/{plat}"),
                        "message": format!(
                            "vector asset `{id}` is referenced by composition.yaml but has no `sources.{plat}` export for the targeted shell platform"
                        ),
                    }));
                }
            }
        }
        // `symbol` falls through: see doc comment above.
        _ => {}
    }
}

/// Platform set Phase 1.7 conservatively considers "targeted". When
/// Phase 3.5 wires the build brief, the actual platform set comes
/// from the proposal's `Platforms` field; this constant becomes the
/// fallback when the proposal is unavailable.
const PLATFORMS: [&str; 2] = ["ios", "android"];

/// Raster density slot order per platform. Matches the property
/// shape `assets.schema.json` accepts on `rasterEntry.sources.<plat>`
/// (RFC-11 Appendix B). The order here is the one warnings render in.
const fn raster_densities(plat: &str) -> &'static [&'static str] {
    match plat.as_bytes() {
        b"ios" => &["1x", "2x", "3x"],
        b"android" => &["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"],
        _ => &[],
    }
}

/// Recorded asset reference from a composition document. The `path`
/// is a JSON-Pointer-shaped indicator that points at the source of
/// the reference inside `composition.yaml` (e.g.
/// `/screens/task-list/header/trailing/0/icon-button/icon`), so the
/// operator can locate the offending node when the reference fails
/// to resolve.
struct AssetRef {
    /// Asset id the composition references (the kebab-case key under
    /// `assets:` it expects to find).
    id: String,
    /// JSON-Pointer-shaped location of the reference inside the
    /// composition document.
    path: String,
}

/// Walk a composition document and collect every static asset
/// reference (`image`, `icon`, `icon-button`, `fab`). Dynamic
/// references (`bind: assets.<id>`) are out of scope for Phase 1.7;
/// Phase 1.9's composition mode inherits RFC-7's bind resolver.
fn collect_asset_references(value: &Value) -> Vec<AssetRef> {
    let mut refs = Vec::new();
    walk_node(value, "", &mut refs);
    refs
}

/// Recursive walker driving [`collect_asset_references`]. We match
/// only the four item-type / region keys that point at a static
/// asset id in v1 to keep the walker tight; the recursion still
/// descends into every value so nested groups, overlay content,
/// state-replaced bodies, and `platforms.*` overrides are all
/// covered.
fn walk_node(node: &Value, json_path: &str, refs: &mut Vec<AssetRef>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                match key.as_str() {
                    // `image:` and `icon:` item types: the asset id
                    // lives under `name:`. We deliberately ignore the
                    // string-shorthand form (`image: foo`) because
                    // the v1 schema requires the object form for
                    // both items, and accepting shorthand here would
                    // double-count the `icon: <string>` property
                    // inside `icon-button` / `fab`.
                    "image" | "icon" => {
                        if let Some(name) = val.get("name").and_then(Value::as_str) {
                            refs.push(AssetRef {
                                id: name.to_string(),
                                path: format!("{child_path}/name"),
                            });
                        }
                    }
                    // `icon-button:` and `fab:` carry the asset id
                    // directly under `icon:`.
                    "icon-button" | "fab" => {
                        if let Some(icon) = val.get("icon").and_then(Value::as_str) {
                            refs.push(AssetRef {
                                id: icon.to_string(),
                                path: format!("{child_path}/icon"),
                            });
                        }
                    }
                    _ => {}
                }
                walk_node(val, &child_path, refs);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_node(v, &format!("{json_path}/{i}"), refs);
            }
        }
        _ => {}
    }
}

/// Escape a JSON Pointer reference token (RFC 6901 §3): `~` becomes
/// `~0` and `/` becomes `~1`. Asset ids are kebab-case so neither
/// substitution fires for the common case, but composition keys (e.g.
/// screen slugs) MAY in principle contain slashes if a future schema
/// relaxation permits, so the escape is safe rather than redundant.
fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

// -------------------------------------------------------------------
// Phase 1.10: artifacts:-block default-path resolver
// -------------------------------------------------------------------
//
// The resolver answers two related questions in one place:
//
// * "What file should `validate <mode>` read when no `[path]`
//   positional is supplied?" -- that's [`resolve_default_path`].
// * "What sibling artifact should cross-artifact resolution chase
//   from this calling artifact's location?" -- that's
//   [`discover_artifact`].
//
// Both walk up from a starting path looking for `.specify/`, parse
// the project's legacy `schema.yaml` (if any) for an on-disk
// `artifacts:` block, and otherwise use the embedded defaults at
// [`EMBEDDED_ARTIFACT_PATHS`]. The walk replaces the Phase 1.7
// `find_sibling_composition` / Phase 1.9 `find_sibling_input` helpers
// the previous phases left behind; both call sites consume
// `Option<PathBuf>` so the migration is purely a body-level
// refactor.

/// Resolve a per-mode default path for `validate <mode>` when no
/// `[path]` positional was supplied.
///
/// Walks up from CWD looking for a project root (the directory
/// containing `.specify/`); falls through to a fixed canonical path
/// (the project-mode template, which is the most operator-friendly
/// "where to put it" home) when the resolver yields nothing.
///
/// The returned `PathBuf` is suitable for `read_to_string`; if no
/// file exists at any candidate location, the *last* candidate is
/// returned so the caller's "<file>.yaml not readable at <path>"
/// error message names the location operators most likely expect.
fn resolve_default_path(mode: ValidateMode) -> PathBuf {
    resolve_default_path_with_root(mode, &default_project_root())
}

/// Return the default project root for omitted `[path]` positionals.
///
/// WASI tool invocations receive `PROJECT_DIR` from the host and normal
/// preopened paths rooted there. Native development keeps the legacy behavior:
/// walk up from CWD to a `.specify/` root when present, otherwise use CWD.
fn default_project_root() -> PathBuf {
    if let Some(project_dir) = std::env::var_os("PROJECT_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(project_dir);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_project_root(&cwd).unwrap_or(cwd)
}

/// Resolve a per-mode default path against an explicit project root.
///
/// Used both by [`resolve_default_path`] (where the root is derived
/// from CWD) and by [`validate_all`] (where the root is the operator's
/// `[path]` positional, defaulting to CWD). When no candidate exists
/// the function returns the *last* candidate it considered; if the
/// candidate list itself is empty (an unknown mode key in a
/// hand-edited `artifacts:` block), it falls back to the embedded
/// canonical name under `<root>/`.
fn resolve_default_path_with_root(mode: ValidateMode, project_root: &Path) -> PathBuf {
    let artifacts = read_artifacts_block(project_root);
    let key = artifact_key_for_mode(mode).unwrap_or("composition");
    let templates = paths_for_key(artifacts.as_ref(), key);

    let mut last_candidate: Option<PathBuf> = None;
    for template in &templates {
        for resolved in expand_path_template(template, project_root) {
            if resolved.is_file() {
                return resolved;
            }
            last_candidate = Some(resolved);
        }
    }
    last_candidate.unwrap_or_else(|| project_root.join(canonical_default_template(key)))
}

/// Locate a sibling artifact (in the [`ValidateMode`] sense) for a
/// caller anchored at `start`. Returns `Some(path)` only when an
/// existing file is found; `None` otherwise.
///
/// Phase 1.7's `validate_assets` calls this with `start = assets.yaml`
/// and `mode = Composition` to pick up sibling compositions for
/// reference resolution; Phase 1.9's `validate_composition` calls it
/// with `mode = Tokens` / `mode = Assets` for auto-invoke. Both call
/// sites keep working unchanged because the helper preserves the
/// `Option<PathBuf>` return shape.
///
/// Resolution order mirrors what the previous `find_sibling_*` helpers
/// did, layered with the new artifacts:-block cascade:
///
/// 1. **Same directory as `start`** -- catches the change-local case
///    where every artifact sits next to its caller (e.g.
///    `.specify/slices/<name>/{composition,tokens,assets}.yaml`),
///    plus standalone "files in the same folder" usage that does not
///    rely on a Specify project layout.
/// 2. **Artifacts:-block cascade against the project root** -- walks
///    up from `start` to find `.specify/`, reads the project's
///    legacy `schema.yaml` `artifacts:` block (or the embedded fallback),
///    and tries every `paths.<role>` template in canonical order.
fn discover_artifact(start: &Path, mode: ValidateMode) -> Option<PathBuf> {
    let key = artifact_key_for_mode(mode)?;

    // (1) Same-directory check. Mirrors the Phase 1.7 / 1.9
    // `find_sibling_*` preamble: if the operator placed the calling
    // artifact next to a sibling of the requested mode, that
    // co-location is the most direct signal of intent.
    let filename = canonical_filename_for_key(key);
    if let Some(parent) = start.parent() {
        let local = parent.join(filename);
        if local.is_file() {
            return Some(local);
        }
    }

    // (2) Artifacts:-block cascade.
    let project_root = find_project_root(start)?;
    let artifacts = read_artifacts_block(&project_root);
    let templates = paths_for_key(artifacts.as_ref(), key);

    for template in &templates {
        for resolved in expand_path_template(template, &project_root) {
            if resolved.is_file() {
                return Some(resolved);
            }
        }
    }
    None
}

/// Filename half of the canonical-default template for a given
/// artifact key. Used by [`discover_artifact`]'s same-directory
/// preamble. Stays in lock-step with [`canonical_default_template`].
fn canonical_filename_for_key(key: &str) -> &'static str {
    match key {
        "layout" => "layout.yaml",
        "tokens" => "tokens.yaml",
        "assets" => "assets.yaml",
        _ => "composition.yaml",
    }
}

/// Walk up from `start` (treated as a directory if it is one,
/// otherwise its parent) looking for the first ancestor that
/// contains a `.specify/` directory. Returns the project root --
/// i.e. the directory containing `.specify/`, *not* `.specify/`
/// itself.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor =
        if start.is_dir() { start.to_path_buf() } else { start.parent()?.to_path_buf() };
    loop {
        if cursor.join(".specify").is_dir() {
            return Some(cursor);
        }
        if !cursor.pop() {
            return None;
        }
    }
}

/// Read the `artifacts:` block from the project's on-disk
/// `schema.yaml` (cached under `.specify/.cache/<schema>/schema.yaml`
/// or vendored under `<root>/schemas/<schema>/schema.yaml`). Returns
/// `None` when:
///
/// - There is no `.specify/project.yaml` at the supplied root.
/// - `project.yaml` is unparseable or has no `schema:` key.
/// - Neither candidate `schema.yaml` is on disk or parseable.
/// - The schema has no `artifacts:` key.
///
/// The caller treats `None` as "use the embedded defaults" so the
/// resolver still works for projects that have not vendored a
/// `schema.yaml` (e.g. early-life projects, or projects whose schema
/// only ships via the agent cache).
fn read_artifacts_block(project_root: &Path) -> Option<Value> {
    let project_yaml = project_root.join(".specify/project.yaml");
    let project_text = std::fs::read_to_string(&project_yaml).ok()?;
    let project: Value = serde_saphyr::from_str(&project_text).ok()?;
    let schema_value = project.get("schema").and_then(Value::as_str)?;
    let schema_name = schema_name_from_value(schema_value);
    let candidates = [
        project_root.join(".specify/.cache").join(&schema_name).join("schema.yaml"),
        project_root.join("schemas").join(&schema_name).join("schema.yaml"),
    ];
    for candidate in &candidates {
        let Some(schema) = parse_yaml_file(candidate) else {
            continue;
        };
        if let Some(artifacts) = schema.get("artifacts") {
            return Some(artifacts.clone());
        }
    }
    None
}

/// Derive a schema directory name from a `schema:` value in
/// `project.yaml`. Mirrors the resolution in
/// `crates/schema/src/schema.rs::locate_schema_root`:
///
/// - URL-shaped values (`https://.../<name>@<ref>`) → take the last
///   non-empty path segment, drop any `@<ref>` suffix.
/// - Bare names (`vectis`, `omnia`, ...) → use as-is.
fn schema_name_from_value(value: &str) -> String {
    if value.contains("://") {
        value.rsplit('/').find(|seg| !seg.is_empty()).map_or_else(
            || value.to_string(),
            |seg| seg.split('@').next().unwrap_or(seg).to_string(),
        )
    } else {
        value.to_string()
    }
}

/// Map a [`ValidateMode`] to the `artifacts:` map key it resolves
/// against. `ValidateMode::All` has no per-mode key (the convenience
/// verb dispatches each per-mode handler in turn) and returns `None`.
const fn artifact_key_for_mode(mode: ValidateMode) -> Option<&'static str> {
    match mode {
        ValidateMode::Layout => Some("layout"),
        ValidateMode::Composition => Some("composition"),
        ValidateMode::Tokens => Some("tokens"),
        ValidateMode::Assets => Some("assets"),
        ValidateMode::All => None,
    }
}

/// Return the ordered list of `paths.<role>` templates for the given
/// artifact `key`. When `artifacts` carries an on-disk entry, its
/// `paths` map's values become the resolution order
/// (`change_local`, `project`, `baseline`); otherwise the embedded
/// defaults from [`EMBEDDED_ARTIFACT_PATHS`] are used.
fn paths_for_key(artifacts: Option<&Value>, key: &str) -> Vec<String> {
    if let Some(artifacts) = artifacts
        && let Some(entry) = artifacts.get(key)
        && let Some(paths) = entry.get("paths")
    {
        let mut out = Vec::new();
        // Honour the canonical resolution order regardless of the
        // YAML map's insertion order. `change_local` is the
        // active-change-first signal; `project` (inputs) and
        // `baseline` (composition) are mutually exclusive in v1 but
        // we walk both keys defensively so a hand-edited schema with
        // both still works.
        for role in ["change_local", "project", "baseline"] {
            if let Some(template) = paths.get(role).and_then(Value::as_str) {
                out.push(template.to_string());
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    EMBEDDED_ARTIFACT_PATHS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, paths)| paths.iter().map(|(_, t)| (*t).to_string()).collect())
        .unwrap_or_default()
}

/// Return the operator-friendly fallback template (the project /
/// baseline location, *not* the change-local one) for a given
/// artifact key. Used as the very last resort when neither the
/// on-disk `artifacts:` block nor the embedded defaults yield any
/// candidate -- which only happens for an unknown key.
fn canonical_default_template(key: &str) -> &'static str {
    match key {
        "layout" => "design-system/layout.yaml",
        "tokens" => "design-system/tokens.yaml",
        "assets" => "design-system/assets.yaml",
        // Composition + unknown keys: the baseline composition path
        // is the most defensible default since `composition.yaml`
        // is the lifecycle artifact every other mode points back at.
        _ => ".specify/specs/composition.yaml",
    }
}

/// Expand a `paths.<role>` template against `project_root`,
/// substituting `<name>` with each directory under
/// `.specify/slices/` (sorted alphabetically). Templates without
/// `<name>` resolve to a single absolute path.
fn expand_path_template(template: &str, project_root: &Path) -> Vec<PathBuf> {
    if !template.contains("<name>") {
        return vec![project_root.join(template)];
    }
    let slices_dir = project_root.join(".specify/slices");
    let Ok(entries) = std::fs::read_dir(&slices_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names.into_iter().map(|name| project_root.join(template.replace("<name>", &name))).collect()
}

// -------------------------------------------------------------------
// Phase 1.10: `validate all`
// -------------------------------------------------------------------

/// Run every per-mode validator against the supplied project root
/// (or CWD when none is given) and fold the per-mode envelopes into
/// a combined `{ "mode": "all", "results": [...] }` envelope (RFC-11
/// §H closing paragraph).
///
/// Sub-mode order: `layout`, `composition`, `tokens`, `assets` --
/// matches the operator-friendly "structural input → wired
/// composition → cross-artifact references" pipeline. When a sub-mode's
/// default-resolved input does not exist on disk, the sub-report is a
/// synthetic `{ mode, path, errors: [], warnings: [], skipped: true,
/// message: ... }` so the combined run continues; the dispatcher's
/// `validate_exit_code` (recursion-aware since Phase 1.6) only flips
/// to non-zero when a real sub-report has errors.
fn validate_all(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let project_root = path.map_or_else(default_project_root, Path::to_path_buf);

    let mut results: Vec<Value> = Vec::new();
    for mode in [
        ValidateMode::Layout,
        ValidateMode::Composition,
        ValidateMode::Tokens,
        ValidateMode::Assets,
    ] {
        let target = resolve_default_path_with_root(mode, &project_root);
        let report = if target.is_file() {
            run_inner(mode, &target)?
        } else {
            json!({
                "mode": mode.as_str(),
                "path": target.display().to_string(),
                "errors": Vec::<Value>::new(),
                "warnings": Vec::<Value>::new(),
                "skipped": true,
                "message": format!(
                    "no input found at {}; default-resolved via the artifacts: block (or its embedded fallback)",
                    target.display(),
                ),
            })
        };
        results.push(json!({
            "mode": mode.as_str(),
            "report": report,
        }));
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::All.as_str(),
        "path": project_root.display().to_string(),
        "results": results,
    })))
}

/// Walk a composition document and append an error for every token
/// reference whose value is not present in `tokens` under the
/// expected category.
///
/// V1 token-ref categories (RFC-11 §F + Appendix D shape):
///
/// - `color`, `background`, `border.color` → `colors.<name>`
/// - `elevation` (groupProps) → `elevation.<name>`
/// - `gap`, `padding`, `padding.<side>` (when string-valued) →
///   `spacing.<name>`
/// - `corner_radius` (when string-valued) → `cornerRadius.<name>`
///
/// Skipped for v1 (deliberately ambiguous, deferred to a follow-on
/// RFC):
///
/// - `style` -- the schema declares `style: { type: string }` with
///   no enum; it is a typography ref on `text` items but a
///   presentation enum on `button`/`list`/etc. Without a
///   per-item-kind classifier, autoresolving it generates false
///   positives.
/// - `size.width` / `size.height` -- the schema's `sizingValue`
///   only permits `"fill"` / `"hug"` strings, so these never
///   reference tokens.
fn resolve_token_references(composition: &Value, tokens: &Value, errors: &mut Vec<Value>) {
    walk_token_refs(composition, "", tokens, errors);
}

/// Recursive walker driving [`resolve_token_references`]. Matches on
/// the well-known token-bearing keys and recurses through the rest
/// of the document. The category lookup is centralised in
/// [`token_category_for_key`] so the walker stays small.
fn walk_token_refs(node: &Value, json_path: &str, tokens: &Value, errors: &mut Vec<Value>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));

                // String-valued token refs: look up the category
                // for `key` and resolve `val` (as a string) against
                // the tokens manifest's category map. `gap` /
                // `padding` / `corner_radius` skip resolution when
                // the value is a number (literal pixel value).
                if let Some(category) = token_category_for_key(key)
                    && let Some(name) = val.as_str()
                {
                    check_token_ref(category, name, &child_path, tokens, errors);
                }

                // `padding` may also be a paddingSpec object. Walk
                // each side as a spacing ref. The string-valued
                // `padding: md` case is already handled above.
                if key == "padding"
                    && let Some(side_map) = val.as_object()
                {
                    for (side, side_val) in side_map {
                        if let Some(name) = side_val.as_str() {
                            let side_path = format!("{child_path}/{}", escape_pointer_token(side));
                            check_token_ref("spacing", name, &side_path, tokens, errors);
                        }
                    }
                }

                walk_token_refs(val, &child_path, tokens, errors);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_token_refs(v, &format!("{json_path}/{i}"), tokens, errors);
            }
        }
        _ => {}
    }
}

/// Map a composition-document key to the `tokens.yaml` category its
/// string value resolves against, or `None` when the key does not
/// carry a deterministic token reference in v1.
const fn token_category_for_key(key: &str) -> Option<&'static str> {
    match key.as_bytes() {
        b"color" | b"background" => Some("colors"),
        b"elevation" => Some("elevation"),
        b"gap" | b"padding" => Some("spacing"),
        b"corner_radius" => Some("cornerRadius"),
        _ => None,
    }
}

/// Resolve `name` against `tokens.<category>` and append an error to
/// `errors` when it is absent. The error message names both the
/// category and the offending name so an operator can fix it
/// without re-reading the manifest.
fn check_token_ref(
    category: &str, name: &str, json_path: &str, tokens: &Value, errors: &mut Vec<Value>,
) {
    let exists =
        tokens.get(category).and_then(Value::as_object).is_some_and(|m| m.contains_key(name));
    if !exists {
        errors.push(json!({
            "path": json_path,
            "message": format!(
                "composition references unknown {category} token `{name}` -- not present in tokens.yaml under `{category}.{name}`",
            ),
        }));
    }
}

/// Walk a composition document and append an error for every static
/// asset reference whose name is not declared under
/// `assets.<id>` in the supplied assets manifest. Reuses Phase 1.7's
/// [`collect_asset_references`] walker so the reference shapes
/// (`image.name`, `icon.name`, `icon-button.icon`, `fab.icon`)
/// stay in lock-step between composition mode (this function) and
/// assets mode's own composition-discovery path.
fn resolve_asset_references(composition: &Value, assets: &Value, errors: &mut Vec<Value>) {
    let asset_ids = assets.get("assets").and_then(Value::as_object);
    let refs = collect_asset_references(composition);
    for asset_ref in &refs {
        let exists = asset_ids.is_some_and(|m| m.contains_key(&asset_ref.id));
        if !exists {
            errors.push(json!({
                "path": asset_ref.path,
                "message": format!(
                    "composition references unknown asset id `{}` -- not present in assets.yaml",
                    asset_ref.id,
                ),
            }));
        }
    }
}

// -------------------------------------------------------------------
// Layout-mode helpers (Phase 1.8). The unwired-subset walker is
// layout-only; the structural-identity engine is shared with Phase
// 1.9's composition mode.
// -------------------------------------------------------------------

/// Walk a YAML sub-tree (typically the `screens` value) and append
/// an error for every define-owned wiring key the unwired subset
/// (RFC-11 §A) forbids:
///
/// - `maps_to` (screen route binding).
/// - `bind` (field binding on items).
/// - `event` (event handler on items).
/// - `error` (validation-error string on items).
/// - `trigger` (overlay trigger).
/// - any key matching the pattern `*-when` (e.g. `strikethrough-when`,
///   `visible-when`) -- conditional visual keys that ride the wiring
///   layer. The bare `when:` key (`stateEntry.when`) is part of the
///   unwired subset and explicitly preserved.
///
/// The walker recurses through every nested object and array so a
/// `bind:` buried in `screens.<name>.body.list.item[0].group.items[0]
/// .checkbox` is reported with a precise JSON Pointer. Tokens such
/// as `style: plain` or `align: center` are property *values* and
/// never trigger a finding -- the walker matches keys, not strings.
fn walk_unwired(node: &Value, json_path: &str, errors: &mut Vec<Value>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                if let Some(reason) = forbidden_wiring_key(key) {
                    errors.push(json!({
                        "path": child_path,
                        "message": format!(
                            "{reason} -- remove this key from layout.yaml (RFC-11 §A unwired-subset rule); wiring is added by /spec:define when it produces composition.yaml"
                        ),
                    }));
                }
                walk_unwired(val, &child_path, errors);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_unwired(v, &format!("{json_path}/{i}"), errors);
            }
        }
        _ => {}
    }
}

/// Classify `key` as a forbidden define-owned wiring key. Returns
/// the human-readable reason string when the key is forbidden, or
/// `None` when the key is allowed in unwired layout documents.
///
/// Edge cases pinned here:
/// - `when` (bare) is the required `stateEntry.when` field; allowed.
/// - `<x>-when` patterns require both the hyphen and the `-when`
///   suffix, so `when` alone never matches. The minimum kebab-case
///   form is at least 6 characters (`a-when`) which the length
///   guard enforces.
fn forbidden_wiring_key(key: &str) -> Option<&'static str> {
    match key {
        "maps_to" => Some("`maps_to` is define-owned screen-to-route wiring"),
        "bind" => Some("`bind` is define-owned field binding"),
        "event" => Some("`event` is define-owned event wiring"),
        "error" => Some("`error` is define-owned validation-error wiring"),
        "trigger" => Some("overlay `trigger` is define-owned"),
        _ if key.ends_with("-when") && key.len() > 5 => {
            Some("conditional visual `*-when` keys are define-owned wiring")
        }
        _ => None,
    }
}

/// Recorded `component: <slug>` instance for the structural-identity
/// engine. The `path` is a JSON Pointer that points at the group
/// that bears the directive, so an identity violation can name both
/// halves.
struct ComponentInstance {
    /// Kebab-case component slug declared by the directive.
    slug: String,
    /// Normalised skeleton derived from the group's `items:` array.
    skeleton: Skeleton,
    /// JSON Pointer indicating where this instance's group lives.
    path: String,
    /// `true` when the instance lives inside a
    /// `screens.<name>.platforms.<plat>.*` sub-tree. Per RFC-11 §G
    /// edge case 3, platform overrides MAY diverge from the base
    /// skeleton -- we collect them but do not enforce base-equality
    /// against them.
    in_platform_override: bool,
}

/// Normalised structural skeleton for a group's children. Keeps just
/// enough information to detect material divergence (item kinds,
/// nested-group nesting, `*-when` key presence) while ignoring leaf
/// wiring values per RFC-11 §G:
///
/// > Slug instances [...] MAY differ in `bind`, `event`, `error`,
/// > `asset`, token references, `*-when` keys, and free text
/// > content.
///
/// (`*-when` keys' *condition values* are wiring; their *presence*
/// participates in skeleton identity per §G edge case 1.)
#[derive(Debug, Eq, PartialEq, Clone)]
enum Skeleton {
    /// A leaf item identified by its single property key (e.g.
    /// `text`, `icon-button`, `checkbox`, `image`). Item leaf
    /// properties are deliberately ignored.
    Item(String),
    /// A group: ordered children plus the sorted, deduplicated set
    /// of `*-when`-keyed properties present on the group props
    /// (presence-only; condition values do not participate).
    Group { when_keys: Vec<String>, items: Vec<Self> },
}

/// Walk a YAML sub-tree (typically the `screens` value) and validate
/// the §G structural-identity rule for every `component: <slug>`
/// directive present. The engine is shared with Phase 1.9's
/// `composition` mode -- the same skeleton-derivation rules apply
/// because §G is artifact-agnostic.
fn check_structural_identity(node: &Value, json_path: &str, errors: &mut Vec<Value>) {
    let mut instances: Vec<ComponentInstance> = Vec::new();
    walk_for_components(node, json_path, false, &mut instances);

    let mut by_slug: BTreeMap<String, Vec<&ComponentInstance>> = BTreeMap::new();
    for inst in &instances {
        by_slug.entry(inst.slug.clone()).or_default().push(inst);
    }

    for (slug, group) in by_slug {
        // Per-instance `platforms.*` overrides MAY diverge from the
        // base skeleton (§G edge case 3). We only enforce identity
        // across the base instances; platform-override instances
        // are collected for completeness but not compared here.
        let base: Vec<&ComponentInstance> =
            group.iter().filter(|i| !i.in_platform_override).copied().collect();
        if base.len() < 2 {
            continue;
        }
        let canonical = base[0];
        for other in base.iter().skip(1) {
            if other.skeleton != canonical.skeleton {
                errors.push(json!({
                    "path": other.path,
                    "message": format!(
                        "component slug `{slug}` has a different skeleton at {} than the canonical instance at {} (RFC-11 §G structural-identity rule); slug instances may differ in `bind`, `event`, `error`, asset / token references, `*-when` condition values, and free text content but their group skeleton MUST match across all base instances",
                        other.path,
                        canonical.path,
                    ),
                }));
            }
        }
    }
}

/// Recursive walker for [`check_structural_identity`]. Every group
/// shaped as `{ "group": { "component": <slug>, "items": [...], ... } }`
/// produces a [`ComponentInstance`]; every nested group inside it
/// is also visited (so `component:` directives nested inside a
/// component group are still picked up). The `in_platform`
/// parameter tracks whether we are currently descending through a
/// `screens.<name>.platforms.<plat>.*` sub-tree.
fn walk_for_components(
    node: &Value, json_path: &str, in_platform: bool, out: &mut Vec<ComponentInstance>,
) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                // Detect the start of a `screens.<name>.platforms`
                // sub-tree. Anything below this point is treated as
                // a per-platform override per §G edge case 3.
                let descend_in_platform = in_platform || key == "platforms";
                if key == "group"
                    && let Some(component) = val.get("component").and_then(Value::as_str)
                {
                    out.push(ComponentInstance {
                        slug: component.to_string(),
                        skeleton: build_group_skeleton(val),
                        path: child_path.clone(),
                        in_platform_override: in_platform,
                    });
                }
                walk_for_components(val, &child_path, descend_in_platform, out);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_for_components(v, &format!("{json_path}/{i}"), in_platform, out);
            }
        }
        _ => {}
    }
}

/// Build a [`Skeleton::Group`] from a `groupProps` JSON value. The
/// `*-when` key set is sorted + deduplicated so two groups carrying
/// the same `*-when`-keyed props (in any author order) compare
/// equal. Children are derived from the `items:` array; missing
/// `items` (schema-invalid) becomes an empty children list.
fn build_group_skeleton(group_props: &Value) -> Skeleton {
    let mut when_keys: Vec<String> = group_props
        .as_object()
        .map(|m| m.keys().filter(|k| k.ends_with("-when") && k.len() > 5).cloned().collect())
        .unwrap_or_default();
    when_keys.sort();
    when_keys.dedup();

    let items: Vec<Skeleton> = group_props
        .get("items")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(build_node_skeleton).collect())
        .unwrap_or_default();

    Skeleton::Group { when_keys, items }
}

/// Build a skeleton fragment for a single `contentNode` (an item or
/// a nested group). Each content node is either:
///
/// - `{ group: { ... } }` -- a nested group, recursed via
///   [`build_group_skeleton`].
/// - `{ <kind>: <itemProps-or-null> }` -- an item identified by its
///   single key (`text`, `checkbox`, `icon`, etc.). Item kind is
///   the only datum the skeleton retains; itemProps (text content,
///   bindings, colors, sizes) are wiring per §G and ignored.
///
/// Schema-invalid shapes (zero or multi-key objects) collapse to a
/// stable `<unknown>` placeholder so the schema validator's own
/// findings remain the authoritative diagnostic.
fn build_node_skeleton(node: &Value) -> Skeleton {
    let Some(map) = node.as_object() else {
        return Skeleton::Item(String::from("<unknown>"));
    };
    if map.len() != 1 {
        return Skeleton::Item(String::from("<unknown>"));
    }
    let (key, val) = map.iter().next().expect("len 1");
    if key == "group" { build_group_skeleton(val) } else { Skeleton::Item(key.clone()) }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::{NamedTempFile, TempDir};

    use super::*;

    /// Appendix D verbatim. Pinned here as a unit test so the embedded
    /// schema stays in lock-step with the RFC's worked example -- if a
    /// future drift breaks Appendix D, this is where the breakage
    /// surfaces first.
    //
    // Uses the `r##"..."##` raw-string delimiter so the embedded
    // `"#0066CC"` patterns don't close the literal early.
    const APPENDIX_D_TOKENS_YAML: &str = r##"version: 1

provenance:
  sources:
    - kind: figma-variables
      uri: "https://www.figma.com/file/ABC123/Design-System"
      captured_at: "2026-04-10T09:15:00Z"
    - kind: manual

colors:
  primary:
    light: "#0066CC"
    dark: "#3399FF"
  on-primary:
    light: "#FFFFFF"
    dark: "#001F3F"
  surface:
    light: "#FFFFFF"
    dark: "#121212"
  on-surface:
    light: "#1C1B1F"
    dark: "#E6E1E5"
  on-surface-variant:
    light: "#49454F"
    dark: "#CAC4D0"
  outline:
    light: "#79747E"
    dark: "#938F99"
  error:
    light: "#B3261E"
    dark: "#F2B8B5"

typography:
  caption:
    size: 12
    weight: regular
    lineHeight: 16
  body:
    size: 16
    weight: regular
    lineHeight: 24
  title:
    size: 22
    weight: semibold
    lineHeight: 28
  display:
    size: 32
    weight: bold
    lineHeight: 40
    letterSpacing: -0.5

spacing:
  xs: 4
  sm: 8
  md: 16
  lg: 24
  xl: 32

cornerRadius:
  sm: 4
  md: 8
  lg: 16

elevation:
  card: 2
  modal: 8

border:
  subtle:
    width: 1
    color: outline
  emphasis:
    width: 2
    color: primary
    radius: 8

opacity:
  disabled: 0.38
  scrim: 0.4
"##;

    /// Appendix E verbatim (RFC-11 §"Appendix E. Example
    /// `assets.yaml` (non-normative)"). Pinned here as the
    /// happy-path schema fixture so any future drift surfaces first
    /// in this test.
    const APPENDIX_E_ASSETS_YAML: &str = r#"version: 1

provenance:
  sources:
    - kind: manual

assets:
  empty-tasks-hero:
    kind: raster
    role: illustration
    alt: "Empty clipboard with a relaxed character beside it"
    sources:
      ios:
        1x: assets/empty-tasks-hero.png
        2x: assets/empty-tasks-hero@2x.png
        3x: assets/empty-tasks-hero@3x.png
      android:
        mdpi: assets/android/empty-tasks-hero-mdpi.png
        hdpi: assets/android/empty-tasks-hero-hdpi.png
        xhdpi: assets/android/empty-tasks-hero-xhdpi.png
        xxhdpi: assets/android/empty-tasks-hero-xxhdpi.png

  brand-logo:
    kind: vector
    role: illustration
    alt: "Acme logo"
    source: assets/brand-logo.svg
    sources:
      ios: assets/ios/brand-logo.pdf
      android: assets/android/brand-logo.xml

  settings:
    kind: symbol
    role: icon
    symbols:
      ios: gearshape
      android: settings
    tint: on-surface

  chevron-left:
    kind: symbol
    role: icon
    symbols:
      ios: chevron.left
      android: arrow_back
    tint: on-surface

  chevron-right:
    kind: symbol
    role: icon
    symbols:
      ios: chevron.right
      android: chevron_right
    tint: on-surface-variant

  plus:
    kind: symbol
    role: icon
    symbols:
      ios: plus
      android: add
    tint: on-primary
"#;

    fn write_tokens(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("tempfile");
        file.write_all(content.as_bytes()).expect("write tokens.yaml");
        file
    }

    fn extract_envelope(outcome: CommandOutcome) -> Value {
        match outcome {
            CommandOutcome::Success(value) => value,
            CommandOutcome::Stub { command } => {
                panic!("expected Success envelope from active mode, got Stub({command})")
            }
        }
    }

    fn errors_array(envelope: &Value) -> &[Value] {
        envelope.get("errors").and_then(Value::as_array).expect("errors array").as_slice()
    }

    fn warnings_array(envelope: &Value) -> &[Value] {
        envelope.get("warnings").and_then(Value::as_array).expect("warnings array").as_slice()
    }

    /// Build a project tree under a fresh tempdir matching the
    /// canonical Specify layout: `<root>/design-system/assets.yaml`
    /// and `<root>/design-system/assets/**` for raster + vector
    /// files. Returns the tempdir and the assets.yaml path.
    fn write_assets_project(yaml: &str, raster_files: &[&str]) -> (TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let design = tmp.path().join("design-system");
        std::fs::create_dir_all(design.join("assets/android")).expect("mkdir assets/android");
        std::fs::create_dir_all(design.join("assets/ios")).expect("mkdir assets/ios");
        let assets_path = design.join("assets.yaml");
        std::fs::write(&assets_path, yaml).expect("write assets.yaml");
        for rel in raster_files {
            let p = design.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("mkdir parent");
            }
            std::fs::write(&p, b"PNGSTUB").expect("write fixture file");
        }
        (tmp, assets_path)
    }

    /// Files referenced by `APPENDIX_E_ASSETS_YAML`: every raster
    /// density, the canonical SVG source, and both vector exports.
    /// Pinned here so the happy-path test stays in lock-step with
    /// the fixture.
    const APPENDIX_E_FILES: &[&str] = &[
        "assets/empty-tasks-hero.png",
        "assets/empty-tasks-hero@2x.png",
        "assets/empty-tasks-hero@3x.png",
        "assets/android/empty-tasks-hero-mdpi.png",
        "assets/android/empty-tasks-hero-hdpi.png",
        "assets/android/empty-tasks-hero-xhdpi.png",
        "assets/android/empty-tasks-hero-xxhdpi.png",
        "assets/brand-logo.svg",
        "assets/ios/brand-logo.pdf",
        "assets/android/brand-logo.xml",
    ];

    #[test]
    fn embedded_tokens_schema_compiles() {
        tokens_validator().expect("embedded tokens.schema.json must compile");
    }

    #[test]
    fn embedded_assets_schema_compiles() {
        assets_validator().expect("embedded assets.schema.json must compile");
    }

    #[test]
    fn appendix_d_validates_cleanly() {
        let file = write_tokens(APPENDIX_D_TOKENS_YAML);
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert_eq!(envelope["mode"], "tokens");
        assert!(errors_array(&envelope).is_empty(), "Appendix D unexpectedly errored: {envelope}");
        assert!(warnings_array(&envelope).is_empty(), "no warnings expected: {envelope}");
    }

    #[test]
    fn minimal_version_only_document_is_valid() {
        let file = write_tokens("version: 1\n");
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert!(errors_array(&envelope).is_empty(), "{envelope}");
    }

    #[test]
    fn broken_hex_reports_a_pathful_error() {
        let yaml = "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n";
        let file = write_tokens(yaml);
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        let errors = errors_array(&envelope);
        assert!(!errors.is_empty(), "expected at least one error for invalid hex: {envelope}");
        let any_path_hits_primary_light = errors.iter().any(|e| {
            e.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| p.contains("/colors/primary/light"))
        });
        assert!(
            any_path_hits_primary_light,
            "expected an error pointing at /colors/primary/light, got: {errors:?}"
        );
    }

    #[test]
    fn unknown_provenance_kind_is_rejected() {
        let yaml = "version: 1\nprovenance:\n  sources:\n    - kind: screenshots\n";
        let file = write_tokens(yaml);
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        // tokens.schema.json's provenance enum is the §F six values
        // (`manual, figma-variables, style-dictionary, tokens-studio,
        // dtcg, legacy`); `screenshots` is the composition-schema
        // value (Phase 1.3) and MUST NOT leak into tokens.
        let errors = errors_array(&envelope);
        assert!(
            !errors.is_empty(),
            "expected `screenshots` to be rejected by tokens schema: {envelope}"
        );
    }

    #[test]
    fn invalid_yaml_surfaces_as_a_single_error_entry() {
        let file = write_tokens(": : not valid yaml :::\n");
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(file.path().to_path_buf()),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        let errors = errors_array(&envelope);
        assert_eq!(errors.len(), 1, "expected one YAML-parse error: {envelope}");
        assert!(
            errors[0]["message"].as_str().unwrap_or("").contains("invalid YAML"),
            "expected `invalid YAML` prefix, got {:?}",
            errors[0]
        );
    }

    #[test]
    fn missing_file_returns_invalid_project_error() {
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(PathBuf::from("/definitely/not/here/tokens.yaml")),
        };
        match run(&args) {
            Err(VectisError::InvalidProject { message }) => {
                assert!(
                    message.contains("tokens.yaml not readable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected InvalidProject for missing file, got {other:?}"),
        }
    }

    #[test]
    fn validate_mode_as_str_matches_value_enum_spelling() {
        for (mode, expected) in [
            (ValidateMode::Layout, "layout"),
            (ValidateMode::Composition, "composition"),
            (ValidateMode::Tokens, "tokens"),
            (ValidateMode::Assets, "assets"),
            (ValidateMode::All, "all"),
        ] {
            assert_eq!(mode.as_str(), expected);
        }
    }

    // -------------------------------------------------------------
    // assets-mode unit tests (Phase 1.7)
    // -------------------------------------------------------------

    /// Appendix E validates cleanly when paired with an
    /// Appendix-C-shaped composition that references every asset id
    /// the manifest declares: empty-tasks-hero, settings,
    /// chevron-left, chevron-right, plus. With both ios and android
    /// densities present (Appendix E's android side also lacks
    /// xxxhdpi -- which surfaces as a warning, not an error -- so
    /// the run is "errors-clean" rather than "absolutely silent").
    #[test]
    fn assets_appendix_e_paired_with_composition_validates_cleanly() {
        let (tmp, assets_path) = write_assets_project(APPENDIX_E_ASSETS_YAML, APPENDIX_E_FILES);
        write_specs_composition(
            tmp.path(),
            // A trimmed composition that references the same asset
            // ids as Appendix C (icon-button, fab, image, icon
            // items). We do not depend on Appendix C verbatim
            // because its "layout" shape has no `delta` / `screens`
            // root in the wired-form sense; this fixture is wiring-
            // free but already valid as a composition document for
            // reference-resolution purposes.
            r"version: 1
screens:
  task-list:
    name: Task list
    header:
      title: My tasks
      trailing:
        - icon-button:
            icon: settings
            label: Open settings
    body:
      list:
        each: tasks
        item:
          - group:
              direction: row
              items:
                - icon:
                    name: chevron-right
    fab:
      icon: plus
      label: Add task
    states:
      empty:
        when: tasks.is_empty
        replaces: body
        body:
          - group:
              direction: column
              items:
                - image:
                    name: empty-tasks-hero
  settings:
    name: Settings
    header:
      title: Settings
      leading:
        - icon-button:
            icon: chevron-left
            label: Back
    body:
      form: []
",
        );

        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert_eq!(envelope["mode"], "assets");
        let errors = errors_array(&envelope);
        assert!(
            errors.is_empty(),
            "Appendix E + composition pairing unexpectedly errored: {errors:?}"
        );
        // `xxxhdpi` is omitted on the android side of empty-tasks-hero,
        // so a warning is the expected shape -- not a failure.
        let warnings = warnings_array(&envelope);
        assert!(
            warnings.iter().any(|w| w["message"]
                .as_str()
                .unwrap_or("")
                .contains("missing optional `xxxhdpi`")),
            "expected at least one missing-density warning for xxxhdpi: {warnings:?}"
        );
    }

    /// A missing 1x raster file produces an error pointing at the
    /// asset entry and the missing path. The `path` field uses the
    /// JSON-Pointer-shaped indicator `/assets/<id>/sources/ios/1x`.
    #[test]
    fn assets_missing_raster_file_is_a_pathful_error() {
        // Same Appendix E manifest, but skip the 1x file when
        // materialising the fixture tree.
        let mut files = APPENDIX_E_FILES.to_vec();
        files.retain(|p| *p != "assets/empty-tasks-hero.png");
        let (_tmp, assets_path) = write_assets_project(APPENDIX_E_ASSETS_YAML, &files);

        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        let errors = errors_array(&envelope);
        let any_hits = errors.iter().any(|e| {
            e["path"].as_str().unwrap_or("") == "/assets/empty-tasks-hero/sources/ios/1x"
                && e["message"].as_str().unwrap_or("").contains("file not found")
        });
        assert!(any_hits, "expected a file-not-found error for 1x: {errors:?}");
    }

    /// Missing optional density is a warning, not an error. The
    /// fixture below trims the empty-tasks-hero raster down to just
    /// 2x and 3x on iOS (and full android coverage so the android
    /// side stays clean) -- and crucially adds a sibling composition
    /// that references the asset, because density warnings only
    /// fire for composition-referenced assets per RFC-11 §E.
    #[test]
    fn assets_missing_optional_density_is_a_warning() {
        let yaml = r"version: 1
assets:
  empty-tasks-hero:
    kind: raster
    role: illustration
    sources:
      ios:
        2x: assets/empty-tasks-hero@2x.png
        3x: assets/empty-tasks-hero@3x.png
      android:
        mdpi: assets/android/empty-tasks-hero-mdpi.png
        hdpi: assets/android/empty-tasks-hero-hdpi.png
        xhdpi: assets/android/empty-tasks-hero-xhdpi.png
        xxhdpi: assets/android/empty-tasks-hero-xxhdpi.png
        xxxhdpi: assets/android/empty-tasks-hero-xxxhdpi.png
";
        let files = [
            "assets/empty-tasks-hero@2x.png",
            "assets/empty-tasks-hero@3x.png",
            "assets/android/empty-tasks-hero-mdpi.png",
            "assets/android/empty-tasks-hero-hdpi.png",
            "assets/android/empty-tasks-hero-xhdpi.png",
            "assets/android/empty-tasks-hero-xxhdpi.png",
            "assets/android/empty-tasks-hero-xxxhdpi.png",
        ];
        let (tmp, assets_path) = write_assets_project(yaml, &files);
        write_specs_composition(
            tmp.path(),
            r"version: 1
screens:
  s:
    name: S
    body:
      list:
        item:
          - image:
              name: empty-tasks-hero
",
        );

        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert!(errors_array(&envelope).is_empty(), "errors unexpected: {envelope}");
        let warnings = warnings_array(&envelope);
        assert!(
            warnings
                .iter()
                .any(|w| w["message"].as_str().unwrap_or("").contains("missing optional `1x`")),
            "expected a missing-1x warning, got: {warnings:?}"
        );
    }

    /// Composition referencing an asset id that is NOT in
    /// `assets.yaml` is an error.
    #[test]
    fn assets_unresolved_composition_reference_is_an_error() {
        let (tmp, assets_path) = write_assets_project(APPENDIX_E_ASSETS_YAML, APPENDIX_E_FILES);
        write_specs_composition(
            tmp.path(),
            // `mystery-glyph` is not in Appendix E.
            r"version: 1
screens:
  s:
    name: S
    header:
      title: T
      trailing:
        - icon-button:
            icon: mystery-glyph
            label: Mystery
    body:
      list:
        item: []
",
        );

        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("references unknown asset id `mystery-glyph`")),
            "expected unresolved-reference error, got: {errors:?}"
        );
    }

    /// Vector asset referenced by composition but missing
    /// `sources.android` is an error (the targeted shell platform
    /// has no usable source).
    #[test]
    fn assets_vector_missing_platform_export_is_an_error() {
        let yaml = r"version: 1
assets:
  brand-logo:
    kind: vector
    role: illustration
    source: assets/brand-logo.svg
    sources:
      ios: assets/ios/brand-logo.pdf
";
        let files = ["assets/brand-logo.svg", "assets/ios/brand-logo.pdf"];
        let (tmp, assets_path) = write_assets_project(yaml, &files);
        write_specs_composition(
            tmp.path(),
            r"version: 1
screens:
  s:
    name: S
    body:
      list:
        item:
          - image:
              name: brand-logo
",
        );

        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| {
                e["path"].as_str().unwrap_or("") == "/assets/brand-logo/sources/android"
                    && e["message"].as_str().unwrap_or("").contains("vector asset `brand-logo`")
            }),
            "expected android-coverage error, got: {errors:?}"
        );
    }

    /// When NO sibling composition is found, density warnings and
    /// platform-coverage errors do not fire -- only schema and
    /// file-existence checks. (The raster below has only ios sources
    /// -- valid at the schema layer because `sources.minProperties:
    /// 1` -- and is fine without composition reference.)
    #[test]
    fn assets_without_sibling_composition_only_runs_schema_and_files() {
        let yaml = r"version: 1
assets:
  empty-tasks-hero:
    kind: raster
    role: illustration
    sources:
      ios:
        2x: assets/empty-tasks-hero@2x.png
";
        let (_tmp, assets_path) = write_assets_project(yaml, &["assets/empty-tasks-hero@2x.png"]);

        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert!(errors_array(&envelope).is_empty(), "no errors expected: {envelope}");
        assert!(
            warnings_array(&envelope).is_empty(),
            "no warnings expected without composition: {envelope}"
        );
    }

    #[test]
    fn assets_missing_file_returns_invalid_project_error() {
        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(PathBuf::from("/definitely/not/here/assets.yaml")),
        };
        match run(&args) {
            Err(VectisError::InvalidProject { message }) => {
                assert!(
                    message.contains("assets.yaml not readable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected InvalidProject for missing file, got {other:?}"),
        }
    }

    /// Schema rejection still fires for assets-mode (e.g. invalid
    /// `kind`); the rejection rides the same envelope shape as the
    /// cross-artifact errors and the dispatcher exits non-zero.
    #[test]
    fn assets_schema_violation_reports_pathful_error() {
        let yaml = r"version: 1
assets:
  bad:
    kind: raster
    role: photograph
    sources:
      ios:
        1x: assets/bad.png
";
        let (_tmp, assets_path) = write_assets_project(yaml, &["assets/bad.png"]);
        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["path"].as_str().unwrap_or("").contains("/assets/bad")),
            "expected a schema error pointing at /assets/bad: {errors:?}"
        );
    }

    /// Asset id case violation (uppercase letter) is rejected by the
    /// schema's `propertyNames` pattern and surfaces as an error
    /// rooted at the assets map.
    #[test]
    fn assets_kebab_case_violation_is_a_schema_error() {
        let yaml = r"version: 1
assets:
  Bad-Id:
    kind: symbol
    role: icon
    symbols:
      ios: foo
";
        let (_tmp, assets_path) = write_assets_project(yaml, &[]);
        let args = ValidateArgs {
            mode: ValidateMode::Assets,
            path: Some(assets_path),
        };
        let envelope = extract_envelope(run(&args).expect("run succeeds"));
        assert!(
            !errors_array(&envelope).is_empty(),
            "expected at least one schema error for `Bad-Id`: {envelope}"
        );
    }

    /// Helper: drop a `.specify/specs/composition.yaml` under
    /// `<project>/` so the asset-validator's `find_sibling_composition`
    /// walk picks it up.
    fn write_specs_composition(project: &Path, yaml: &str) {
        let dir = project.join(".specify").join("specs");
        std::fs::create_dir_all(&dir).expect("mkdir .specify/specs");
        std::fs::write(dir.join("composition.yaml"), yaml).expect("write composition.yaml");
    }

    // -------------------------------------------------------------
    // layout-mode unit tests (Phase 1.8)
    // -------------------------------------------------------------

    /// Appendix C verbatim (RFC-11 §"Appendix C. Example
    /// `layout.yaml` (non-normative)"). Pinned here as the
    /// happy-path schema fixture so any future drift surfaces in
    /// this test first. The example exercises the unwired subset
    /// end-to-end: regions, groups (one with `component: task-row`),
    /// items, token references, asset references, states with the
    /// `stateEntry.when` field (which is the bare `when:` -- not a
    /// `*-when` key -- and explicitly preserved), overlays without
    /// `trigger`, and a `platforms.{ios,android}` block.
    const APPENDIX_C_LAYOUT_YAML: &str = r#"version: 1

provenance:
  sources:
    - kind: screenshots
      captured_at: "2026-04-12T10:30:00Z"
    - kind: manual

screens:
  task-list:
    name: Task list
    description: Primary screen showing all open tasks for the signed-in user.
    header:
      title: My tasks
      trailing:
        - icon-button:
            icon: settings
            label: Open settings
    body:
      list:
        each: tasks
        style: plain
        item:
          - group:
              component: task-row
              direction: row
              gap: md
              padding: md
              align: center
              items:
                - checkbox:
                    label: Mark task complete
                - group:
                    direction: column
                    gap: xs
                    size:
                      width: fill
                    items:
                      - text:
                          role: heading
                          style: body
                      - text:
                          style: caption
                          color: on-surface-variant
                - icon:
                    name: chevron-right
                    color: on-surface-variant
    fab:
      icon: plus
      label: Add task
    states:
      empty:
        when: tasks.is_empty
        replaces: body
        body:
          - group:
              direction: column
              gap: md
              padding: lg
              align: center
              justify: center
              items:
                - image:
                    name: empty-tasks-hero
                - text:
                    content: No tasks yet
                    style: title
                - text:
                    content: Tap the + button to add your first task.
                    style: body
                    color: on-surface-variant
      loading:
        when: tasks.is_loading
        replaces: body
        body:
          - progress-indicator:
              style: circular
    overlays:
      delete-confirm:
        kind: dialog
        title: Delete task?
        content:
          - text:
              content: This task will be removed permanently.
          - group:
              direction: row
              gap: sm
              justify: end
              items:
                - button:
                    label: Cancel
                    style: text
                - button:
                    label: Delete
                    style: text
                    color: error

  settings:
    name: Settings
    header:
      title: Settings
      leading:
        - icon-button:
            icon: chevron-left
            label: Back
    body:
      form:
        - group:
            direction: column
            gap: lg
            padding: md
            items:
              - text:
                  content: Appearance
                  role: heading
                  style: title
              - segmented-control:
                  options:
                    - System
                    - Light
                    - Dark
              - text:
                  content: Account
                  role: heading
                  style: title
              - button:
                  label: Sign out
                  style: outlined
                  color: error
    platforms:
      ios:
        header:
          title: Settings
      android:
        header:
          title: Settings
"#;

    fn write_layout(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("tempfile");
        file.write_all(content.as_bytes()).expect("write layout.yaml");
        file
    }

    fn run_layout(content: &str) -> Value {
        let file = write_layout(content);
        let args = ValidateArgs {
            mode: ValidateMode::Layout,
            path: Some(file.path().to_path_buf()),
        };
        extract_envelope(run(&args).expect("run succeeds"))
    }

    #[test]
    fn embedded_composition_schema_compiles() {
        composition_validator().expect("embedded composition.schema.json must compile");
    }

    /// Acceptance bullet 1: Appendix C's `layout.yaml` validates
    /// cleanly. Schema passes (the `screens`-shape `oneOf` branch),
    /// no forbidden wiring keys are present, and the single
    /// `component: task-row` instance has nothing to compare
    /// against -- so structural-identity is a no-op.
    #[test]
    fn layout_appendix_c_validates_cleanly() {
        let envelope = run_layout(APPENDIX_C_LAYOUT_YAML);
        assert_eq!(envelope["mode"], "layout");
        assert!(errors_array(&envelope).is_empty(), "Appendix C unexpectedly errored: {envelope}");
        assert!(
            warnings_array(&envelope).is_empty(),
            "no warnings expected for Appendix C: {envelope}"
        );
    }

    /// Acceptance bullet 2: a `bind:` key anywhere in the document
    /// produces an error pointing at the offending node.
    #[test]
    fn layout_bind_key_is_rejected_with_pathful_error() {
        let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - checkbox:
              bind: tasks.completed
";
        let envelope = run_layout(yaml);
        let errors = errors_array(&envelope);
        let any_hit = errors.iter().any(|e| {
            e["path"].as_str().unwrap_or("").ends_with("/checkbox/bind")
                && e["message"].as_str().unwrap_or("").contains("`bind` is define-owned")
        });
        assert!(any_hit, "expected a `bind` rejection with the offending JSON Pointer: {errors:?}");
    }

    /// `event:`, `error:`, `maps_to:`, overlay `trigger:`, and a
    /// representative `*-when` key (`strikethrough-when`) are all
    /// rejected by the unwired-subset walker. The bare `when:` on
    /// `stateEntry` -- which appears in Appendix C as
    /// `when: tasks.is_empty` -- MUST stay allowed; the matrix
    /// pinned below also asserts that.
    #[test]
    fn layout_every_forbidden_wiring_key_is_rejected_but_bare_when_passes() {
        let yaml = r"version: 1
screens:
  s:
    name: S
    maps_to: SomeRoute
    body:
      list:
        each: tasks
        item:
          - text:
              content: hello
              event: Tapped
              error: required
              strikethrough-when: tasks.completed
    overlays:
      sheet:
        kind: sheet
        trigger: OpenSheet
        content:
          - text:
              content: hi
    states:
      empty:
        when: tasks.is_empty
        replaces: body
        body:
          - text:
              content: nothing here
";
        let envelope = run_layout(yaml);
        let errors = errors_array(&envelope);
        let messages: Vec<String> =
            errors.iter().map(|e| e["message"].as_str().unwrap_or("").to_string()).collect();

        for key in [
            "`maps_to` is define-owned",
            "`event` is define-owned",
            "`error` is define-owned",
            "overlay `trigger` is define-owned",
            "`*-when` keys are define-owned",
        ] {
            assert!(
                messages.iter().any(|m| m.contains(key)),
                "expected a finding mentioning {key:?}, got: {messages:?}"
            );
        }

        // The bare `when:` on stateEntry is *not* a forbidden key.
        // No error message should reference `/states/empty/when`.
        assert!(
            !errors
                .iter()
                .any(|e| e["path"].as_str().unwrap_or("").ends_with("/states/empty/when")),
            "stateEntry.when (bare `when:`) MUST stay allowed: {errors:?}"
        );
    }

    /// Acceptance bullet 3: a `delta:` document is rejected, even
    /// when it would otherwise pass the schema (the schema's
    /// `oneOf` permits `delta`). The error points at `/delta`.
    #[test]
    fn layout_delta_document_is_rejected() {
        let yaml = r"version: 1
delta:
  added:
    new-screen:
      name: New
      body:
        list:
          each: things
          item:
            - text:
                content: hello
";
        let envelope = run_layout(yaml);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["path"].as_str().unwrap_or("") == "/delta"
                && e["message"].as_str().unwrap_or("").contains("MUST NOT use the `delta` shape")),
            "expected `/delta` rejection: {errors:?}"
        );
    }

    /// Acceptance bullet 4 (positive half): two groups in different
    /// screens carrying the same `component:` slug with the *same*
    /// skeleton but different free text content / token references
    /// validate cleanly. The wiring-difference dimension that
    /// composition mode (Phase 1.9) cares about (`bind` / `event` /
    /// etc.) cannot be exercised in layout mode because those keys
    /// are forbidden by the unwired subset; the structural-identity
    /// engine still ignores leaf wiring values across all
    /// invocations, so the tightest test we can land here exercises
    /// content + token-ref divergence with skeleton match.
    #[test]
    fn layout_same_skeleton_different_wiring_validates_cleanly() {
        let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: First card heading
                style: title
                color: on-surface
            - text:
                content: First card body
                style: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: Second card heading
                style: title
                color: primary
            - text:
                content: Second card body
                style: caption
";
        let envelope = run_layout(yaml);
        assert!(
            errors_array(&envelope).is_empty(),
            "same skeleton + differing leaf values must validate: {envelope}"
        );
    }

    /// Acceptance bullet 4 (negative half): two groups in different
    /// screens carrying the same `component:` slug with materially
    /// different skeletons (different ordered nested item kinds)
    /// produce a structural-identity error.
    #[test]
    fn layout_different_skeletons_same_slug_is_an_error() {
        let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - text:
                content: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - icon:
                name: chevron-right
            - text:
                content: body
";
        let envelope = run_layout(yaml);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("component slug `card` has a different skeleton")),
            "expected a structural-identity error for `card`: {errors:?}"
        );
    }

    /// Edge case: differing nested-group depth between two slug
    /// instances also triggers a structural-identity error. This
    /// pins §G's "same nested item kinds, same nesting shape" rule
    /// concretely.
    #[test]
    fn layout_different_nested_group_depth_is_an_error() {
        let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: row
          direction: row
          items:
            - text:
                content: a
            - text:
                content: b
  two:
    name: Two
    body:
      - group:
          component: row
          direction: row
          items:
            - text:
                content: a
            - group:
                direction: column
                items:
                  - text:
                      content: b
";
        let envelope = run_layout(yaml);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("component slug `row` has a different skeleton")),
            "expected a structural-identity error for `row`: {errors:?}"
        );
    }

    /// Edge case: per-instance `platforms.*` overrides MAY diverge
    /// from the base skeleton (RFC-11 §G edge case 3). The base
    /// instances must still match, but a `screens.<n>.platforms.ios.body`
    /// instance with a different shape does not trigger the rule.
    #[test]
    fn layout_platforms_override_instance_is_exempt_from_base_match() {
        let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
    platforms:
      ios:
        body:
          - group:
              component: card
              direction: column
              items:
                - text:
                    content: heading
                - icon:
                    name: chevron-right
                - text:
                    content: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
";
        let envelope = run_layout(yaml);
        assert!(
            errors_array(&envelope).is_empty(),
            "platforms.* override instance MUST be exempt from base-skeleton match: {envelope}"
        );
    }

    /// A single `component:` instance has nothing to compare
    /// against; the structural-identity rule is a no-op until a
    /// second base instance appears (matches §J's conservative
    /// emission policy: directives only emitted when ≥2 instances
    /// agree on a slug, but the validator does not require that --
    /// it is only sensitive to disagreement).
    #[test]
    fn layout_single_component_instance_passes_silently() {
        let yaml = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
";
        let envelope = run_layout(yaml);
        assert!(
            errors_array(&envelope).is_empty(),
            "single component instance should pass silently: {envelope}"
        );
    }

    /// Schema rejection still fires for layout-mode (e.g. an
    /// unknown screen-property name); the rejection rides the same
    /// envelope shape as the unwired-subset / structural-identity
    /// errors and the dispatcher exits non-zero.
    #[test]
    fn layout_schema_violation_reports_pathful_error() {
        let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - text:
              content: hi
        unknown_listpattern_field: nope
";
        let envelope = run_layout(yaml);
        let errors = errors_array(&envelope);
        assert!(
            !errors.is_empty(),
            "expected at least one schema error for unknown_listpattern_field: {envelope}"
        );
    }

    /// Reserved component slug (e.g. `header`) is rejected by
    /// `composition.schema.json`'s F.2 patch (`component.not.enum`).
    /// The layout-mode validator surfaces it as a schema error.
    #[test]
    fn layout_reserved_component_slug_is_rejected() {
        let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          component: header
          direction: column
          items:
            - text:
                content: hi
";
        let envelope = run_layout(yaml);
        let errors = errors_array(&envelope);
        assert!(
            !errors.is_empty(),
            "reserved slug `header` MUST be rejected by the F.2 patch: {envelope}"
        );
    }

    #[test]
    fn layout_invalid_yaml_surfaces_as_a_single_error_entry() {
        let envelope = run_layout(": : not valid yaml :::\n");
        let errors = errors_array(&envelope);
        assert_eq!(errors.len(), 1, "expected one YAML-parse error: {envelope}");
        assert!(
            errors[0]["message"].as_str().unwrap_or("").contains("invalid YAML"),
            "expected `invalid YAML` prefix, got {:?}",
            errors[0]
        );
    }

    #[test]
    fn layout_missing_file_returns_invalid_project_error() {
        let args = ValidateArgs {
            mode: ValidateMode::Layout,
            path: Some(PathBuf::from("/definitely/not/here/layout.yaml")),
        };
        match run(&args) {
            Err(VectisError::InvalidProject { message }) => {
                assert!(
                    message.contains("layout.yaml not readable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected InvalidProject for missing file, got {other:?}"),
        }
    }

    // -------------------------------------------------------------
    // composition-mode unit tests (Phase 1.9)
    // -------------------------------------------------------------

    /// Materialise a composition document plus optional sibling
    /// `tokens.yaml` and `assets.yaml` on disk under a fresh
    /// tempdir, returning the tempdir and the composition path.
    /// The two helpers default to placing the inputs in the same
    /// directory (the change-local-shape that
    /// [`find_sibling_input`] picks up first).
    fn write_composition_project(
        composition: &str, tokens: Option<&str>, assets: Option<&str>,
    ) -> (TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Mark the tree as a Specify project so
        // `find_sibling_input`'s walk-up can stop at the right
        // anchor even when we're testing the design-system fallback
        // shape elsewhere.
        std::fs::create_dir_all(tmp.path().join(".specify")).expect("mkdir .specify");
        let comp_path = tmp.path().join("composition.yaml");
        std::fs::write(&comp_path, composition).expect("write composition.yaml");
        if let Some(yaml) = tokens {
            std::fs::write(tmp.path().join("tokens.yaml"), yaml).expect("write tokens.yaml");
        }
        if let Some(yaml) = assets {
            std::fs::write(tmp.path().join("assets.yaml"), yaml).expect("write assets.yaml");
        }
        (tmp, comp_path)
    }

    /// Run `validate_composition` against a composition fixture and
    /// return the unwrapped JSON envelope. Mirrors `run_layout` but
    /// runs through the public `run` dispatcher so the dispatch
    /// arm stays exercised.
    fn run_composition(comp_path: &Path) -> Value {
        let args = ValidateArgs {
            mode: ValidateMode::Composition,
            path: Some(comp_path.to_path_buf()),
        };
        extract_envelope(run(&args).expect("run succeeds"))
    }

    /// Acceptance baseline: a minimal valid composition with no
    /// sibling tokens / assets validates cleanly. The envelope
    /// SHOULD NOT carry a `results` array when no sibling files
    /// were found (the array is only emitted when auto-invoke
    /// folded something in).
    #[test]
    fn composition_clean_run_validates_silently_without_siblings() {
        let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - text:
              content: hello
";
        let (_tmp, comp_path) = write_composition_project(yaml, None, None);
        let envelope = run_composition(&comp_path);
        assert_eq!(envelope["mode"], "composition");
        assert!(errors_array(&envelope).is_empty(), "errors unexpected: {envelope}");
        assert!(warnings_array(&envelope).is_empty(), "warnings unexpected: {envelope}");
        assert!(
            envelope.get("results").is_none(),
            "results array should be absent without auto-invoke: {envelope}"
        );
    }

    /// Composition mode (unlike layout mode) MUST allow
    /// define-owned wiring keys (`bind`, `event`, `error`,
    /// overlay `trigger`, `*-when`) and `delta:` shape. This pins
    /// the contract distinction that justifies two runtime layers
    /// over the same schema.
    #[test]
    fn composition_permits_wired_keys_layout_rejects() {
        let yaml = r"version: 1
screens:
  s:
    name: S
    maps_to: SomeRoute
    body:
      list:
        each: tasks
        item:
          - checkbox:
              bind: tasks.completed
              event: ToggleTask
              strikethrough-when: tasks.completed
    overlays:
      sheet:
        kind: sheet
        trigger: OpenSheet
        content:
          - text:
              content: hi
";
        let (_tmp, comp_path) = write_composition_project(yaml, None, None);
        let envelope = run_composition(&comp_path);
        assert!(
            errors_array(&envelope).is_empty(),
            "wired keys MUST validate cleanly in composition mode: {envelope}"
        );
    }

    /// `delta:` documents are valid in composition mode (the
    /// change-local lifecycle shape RFC-11 §H names). The schema's
    /// `oneOf` accepts either `screens` or `delta`.
    #[test]
    fn composition_accepts_delta_documents() {
        let yaml = r"version: 1
delta:
  added:
    new-screen:
      name: New
      body:
        list:
          each: things
          item:
            - text:
                content: hello
  modified:
    other:
      name: Other
      body:
        - text:
            content: hi
";
        let (_tmp, comp_path) = write_composition_project(yaml, None, None);
        let envelope = run_composition(&comp_path);
        assert!(errors_array(&envelope).is_empty(), "delta MUST validate cleanly: {envelope}");
    }

    /// A token reference (`color: nonexistent`) that is absent from
    /// the sibling `tokens.yaml` produces a composition-mode error
    /// pointing at the offending node. This is the cross-artifact
    /// resolution layer the auto-invoke does NOT cover (the
    /// auto-invoke catches "tokens.yaml is itself broken" -- this
    /// catches "composition references something tokens.yaml does
    /// not declare").
    #[test]
    fn composition_unresolved_color_token_is_an_error() {
        let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
          color: nonexistent
";
        let tokens = r##"version: 1
colors:
  primary:
    light: "#0066CC"
    dark: "#3399FF"
"##;
        let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("unknown colors token `nonexistent`")
                && e["path"].as_str().unwrap_or("").ends_with("/text/color")),
            "expected an unresolved-color error: {errors:?}"
        );
    }

    /// String-valued `gap: <name>` references resolve against
    /// `spacing.<name>`. A typo (`gap: mid` instead of `md`) MUST
    /// surface as an error.
    #[test]
    fn composition_unresolved_spacing_token_is_an_error() {
        let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          gap: mid
          items:
            - text:
                content: hi
";
        let tokens = r"version: 1
spacing:
  xs: 4
  sm: 8
  md: 16
  lg: 24
";
        let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("unknown spacing token `mid`")),
            "expected an unresolved-spacing error: {errors:?}"
        );
    }

    /// Numeric `gap: 16` MUST NOT surface a token-resolution error
    /// -- it is a literal pixel value. This pins the
    /// string-or-number split at the resolver layer.
    #[test]
    fn composition_numeric_spacing_is_not_a_token_ref() {
        let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          gap: 16
          padding: 8
          items:
            - text:
                content: hi
";
        let tokens = r"version: 1
spacing:
  xs: 4
";
        let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
        let envelope = run_composition(&comp_path);
        assert!(
            errors_array(&envelope).is_empty(),
            "numeric spacing values MUST NOT trip the resolver: {envelope}"
        );
    }

    /// `padding` may be a paddingSpec object with per-side string
    /// values (`top: md`, etc.). Each side resolves against
    /// `spacing.<name>` independently.
    #[test]
    fn composition_padding_object_resolves_per_side() {
        let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          padding:
            top: md
            bottom: lg
            left: nope
          items:
            - text:
                content: hi
";
        let tokens = r"version: 1
spacing:
  md: 16
  lg: 24
";
        let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["path"].as_str().unwrap_or("").ends_with("/padding/left")
                && e["message"].as_str().unwrap_or("").contains("unknown spacing token `nope`")),
            "expected an unresolved-padding-side error: {errors:?}"
        );
        // The other two sides should NOT produce findings.
        assert!(
            !errors.iter().any(|e| e["path"].as_str().unwrap_or("").ends_with("/padding/top")
                || e["path"].as_str().unwrap_or("").ends_with("/padding/bottom")),
            "valid padding sides must not surface: {errors:?}"
        );
    }

    /// Elevation tokens resolve against `elevation.<name>` and
    /// `corner_radius` tokens against `cornerRadius.<name>`. A typo
    /// in either category surfaces as an error.
    #[test]
    fn composition_unresolved_elevation_and_corner_radius_are_errors() {
        let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          direction: column
          elevation: floating
          corner_radius: huge
          items:
            - text:
                content: hi
";
        let tokens = r"version: 1
elevation:
  card: 2
cornerRadius:
  md: 8
";
        let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), None);
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("unknown elevation token `floating`")),
            "expected an unresolved-elevation error: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("unknown cornerRadius token `huge`")),
            "expected an unresolved-cornerRadius error: {errors:?}"
        );
    }

    /// Asset references (`image.name`, `icon.name`, `icon-button.icon`,
    /// `fab.icon`) that point at unknown ids in the sibling
    /// `assets.yaml` produce composition-mode errors via Phase
    /// 1.7's [`collect_asset_references`] walker.
    #[test]
    fn composition_unresolved_asset_id_is_an_error() {
        let composition = r"version: 1
screens:
  s:
    name: S
    header:
      title: T
      trailing:
        - icon-button:
            icon: mystery
            label: Mystery
    body:
      - image:
          name: empty-tasks-hero
";
        let assets = r"version: 1
assets:
  empty-tasks-hero:
    kind: symbol
    role: icon
    symbols:
      ios: foo
      android: bar
";
        let (_tmp, comp_path) = write_composition_project(composition, None, Some(assets));
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("unknown asset id `mystery`")
                && e["path"].as_str().unwrap_or("").ends_with("/icon-button/icon")),
            "expected an unresolved-asset error: {errors:?}"
        );
        // The valid `empty-tasks-hero` ref must NOT surface.
        assert!(
            !errors
                .iter()
                .any(|e| e["message"].as_str().unwrap_or("").contains("`empty-tasks-hero`")),
            "valid asset id MUST resolve cleanly: {errors:?}"
        );
    }

    /// Auto-invoke: when a sibling `tokens.yaml` exists, the
    /// composition envelope's `results` array MUST contain a
    /// `tokens` report. A broken hex inside that tokens.yaml
    /// surfaces as an error inside `results[].report.errors`,
    /// which the dispatcher's `validate_exit_code` recurses
    /// through.
    #[test]
    fn composition_auto_invokes_tokens_and_folds_into_results() {
        let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
";
        let broken_tokens = r##"version: 1
colors:
  primary:
    light: "#xyz"
    dark: "#000000"
"##;
        let (_tmp, comp_path) = write_composition_project(composition, Some(broken_tokens), None);
        let envelope = run_composition(&comp_path);
        let results = envelope["results"].as_array().expect("results array present");
        assert_eq!(results.len(), 1, "expected exactly one folded sub-report: {envelope}");
        assert_eq!(results[0]["mode"], "tokens");
        let tokens_errors =
            results[0]["report"]["errors"].as_array().expect("nested tokens.errors is an array");
        assert!(
            !tokens_errors.is_empty(),
            "expected the broken hex to surface in the folded tokens report: {envelope}"
        );
    }

    /// Auto-invoke: when both sibling tokens and assets exist, both
    /// reports surface and the order in `results` is `tokens`
    /// before `assets` (matches the order Phase 1.10's
    /// `validate all` will ship).
    #[test]
    fn composition_auto_invokes_tokens_and_assets_in_order() {
        let composition = r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
";
        let tokens = r"version: 1
spacing:
  md: 16
";
        let assets = r"version: 1
assets: {}
";
        let (_tmp, comp_path) = write_composition_project(composition, Some(tokens), Some(assets));
        let envelope = run_composition(&comp_path);
        let results = envelope["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2, "expected two folded sub-reports: {envelope}");
        assert_eq!(results[0]["mode"], "tokens");
        assert_eq!(results[1]["mode"], "assets");
    }

    /// Structural-identity (RFC-11 §G) reuses Phase 1.8's engine.
    /// Two `component: card` instances with materially different
    /// skeletons in the `screens` shape MUST produce a
    /// composition-mode error.
    #[test]
    fn composition_structural_identity_violation_in_screens() {
        let composition = r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - text:
                content: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - icon:
                name: chevron-right
            - text:
                content: body
";
        let (_tmp, comp_path) = write_composition_project(composition, None, None);
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("component slug `card` has a different skeleton")),
            "expected structural-identity error in screens shape: {errors:?}"
        );
    }

    /// Structural-identity walks the `delta` sub-tree too: a slug
    /// added in `delta.added` must agree with the same slug
    /// modified in `delta.modified`. This is the cross-shape
    /// behavior the plan called out explicitly for Phase 1.9.
    #[test]
    fn composition_structural_identity_violation_in_delta() {
        let composition = r"version: 1
delta:
  added:
    one:
      name: One
      body:
        - group:
            component: card
            direction: column
            items:
              - text:
                  content: heading
              - text:
                  content: body
  modified:
    two:
      name: Two
      body:
        - group:
            component: card
            direction: column
            items:
              - text:
                  content: heading
              - icon:
                  name: chevron-right
              - text:
                  content: body
";
        let (_tmp, comp_path) = write_composition_project(composition, None, None);
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.iter().any(|e| e["message"]
                .as_str()
                .unwrap_or("")
                .contains("component slug `card` has a different skeleton")),
            "expected structural-identity error in delta shape: {errors:?}"
        );
    }

    /// Acceptance bullet 4: the Appendix C/D/E example trio
    /// validates cleanly end-to-end. Appendix C is reused as the
    /// composition (it already validates against the schema -- the
    /// `oneOf` accepts the unwired-subset shape -- and contains
    /// every token / asset reference shape composition mode
    /// resolves). Appendix D supplies the tokens, Appendix E the
    /// assets.
    #[test]
    fn composition_appendix_trio_validates_cleanly() {
        let (tmp, comp_path) = write_composition_project(
            APPENDIX_C_LAYOUT_YAML,
            Some(APPENDIX_D_TOKENS_YAML),
            Some(APPENDIX_E_ASSETS_YAML),
        );
        // Materialise every referenced asset file under
        // `<tmp>/assets/**` so the auto-invoked assets mode finds
        // them on disk.
        for rel in APPENDIX_E_FILES {
            let p = tmp.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("mkdir parent");
            }
            std::fs::write(&p, b"PNGSTUB").expect("write fixture file");
        }

        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert!(
            errors.is_empty(),
            "Appendix C/D/E trio unexpectedly produced composition-mode errors: {errors:?}"
        );

        // Both sub-reports must be present and error-free. (The
        // assets sub-report MAY carry warnings -- Appendix E omits
        // `xxxhdpi` on the empty-tasks-hero android side -- which
        // is the expected "missing optional density" warning shape
        // and not a failure.)
        let results = envelope["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2, "expected tokens + assets sub-reports: {envelope}");
        for entry in results {
            let mode = entry["mode"].as_str().unwrap_or("?");
            let report_errors = entry["report"]["errors"]
                .as_array()
                .unwrap_or_else(|| panic!("[{mode}] missing errors array: {envelope}"));
            assert!(
                report_errors.is_empty(),
                "[{mode}] sub-report unexpectedly errored: {report_errors:?}"
            );
        }
    }

    /// The design-system-shape sibling fallback: when the
    /// composition lives at `<root>/.specify/specs/composition.yaml`
    /// (the canonical baseline location), `find_sibling_input`
    /// walks up to `<root>/` and picks up
    /// `<root>/design-system/tokens.yaml` /
    /// `<root>/design-system/assets.yaml`.
    #[test]
    fn composition_design_system_fallback_picks_up_siblings() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let specs_dir = tmp.path().join(".specify/specs");
        let design_dir = tmp.path().join("design-system");
        std::fs::create_dir_all(&specs_dir).expect("mkdir .specify/specs");
        std::fs::create_dir_all(&design_dir).expect("mkdir design-system");
        let comp_path = specs_dir.join("composition.yaml");
        std::fs::write(
            &comp_path,
            r"version: 1
screens:
  s:
    name: S
    body:
      - text:
          content: hi
          color: surface
",
        )
        .expect("write composition.yaml");
        std::fs::write(
            design_dir.join("tokens.yaml"),
            r##"version: 1
colors:
  surface:
    light: "#FFFFFF"
    dark: "#000000"
"##,
        )
        .expect("write design-system/tokens.yaml");
        std::fs::write(design_dir.join("assets.yaml"), "version: 1\nassets: {}\n")
            .expect("write design-system/assets.yaml");

        let envelope = run_composition(&comp_path);
        assert!(
            errors_array(&envelope).is_empty(),
            "design-system fallback path MUST resolve cleanly: {envelope}"
        );
        let results = envelope["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2, "expected tokens + assets fallback fold: {envelope}");
    }

    /// Reserved component slugs (header / body / footer / fab) are
    /// rejected by the F.2 patch's `not.enum` -- composition mode
    /// surfaces this as a schema error just like layout mode does.
    #[test]
    fn composition_reserved_component_slug_is_rejected() {
        let yaml = r"version: 1
screens:
  s:
    name: S
    body:
      - group:
          component: header
          direction: column
          items:
            - text:
                content: hi
";
        let (_tmp, comp_path) = write_composition_project(yaml, None, None);
        let envelope = run_composition(&comp_path);
        assert!(
            !errors_array(&envelope).is_empty(),
            "reserved slug `header` MUST be rejected by the F.2 patch: {envelope}"
        );
    }

    #[test]
    fn composition_invalid_yaml_surfaces_as_a_single_error_entry() {
        let (_tmp, comp_path) = write_composition_project(": : not valid yaml :::\n", None, None);
        let envelope = run_composition(&comp_path);
        let errors = errors_array(&envelope);
        assert_eq!(errors.len(), 1, "expected one YAML-parse error: {envelope}");
        assert!(
            errors[0]["message"].as_str().unwrap_or("").contains("invalid YAML"),
            "expected `invalid YAML` prefix, got {:?}",
            errors[0]
        );
    }

    #[test]
    fn composition_missing_file_returns_invalid_project_error() {
        let args = ValidateArgs {
            mode: ValidateMode::Composition,
            path: Some(PathBuf::from("/definitely/not/here/composition.yaml")),
        };
        match run(&args) {
            Err(VectisError::InvalidProject { message }) => {
                assert!(
                    message.contains("composition.yaml not readable"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected InvalidProject for missing file, got {other:?}"),
        }
    }

    // -------------------------------------------------------------
    // Phase 1.10: artifacts:-block resolver + `validate all`
    // -------------------------------------------------------------

    /// Materialise a minimal Specify project under a fresh tempdir
    /// matching the canonical layout `read_artifacts_block` walks:
    /// `<root>/.specify/project.yaml` plus
    /// `<root>/schemas/vectis/schema.yaml` (the local-shape schema
    /// path; the cached shape under `.specify/.cache/<name>/` works
    /// the same way and is exercised by a sibling test below). The
    /// schema content embeds the v2 `artifacts:` block from
    /// `schemas/vectis/schema.yaml` (so the resolver picks up the
    /// on-disk shape rather than the embedded fallback).
    fn write_specify_project(extra_schema_yaml: Option<&str>) -> TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dot_specify = tmp.path().join(".specify");
        std::fs::create_dir_all(&dot_specify).expect("mkdir .specify");
        std::fs::write(dot_specify.join("project.yaml"), "name: demo\nschema: vectis\n")
            .expect("write project.yaml");
        if let Some(yaml) = extra_schema_yaml {
            let schema_dir = tmp.path().join("schemas").join("vectis");
            std::fs::create_dir_all(&schema_dir).expect("mkdir schemas/vectis");
            std::fs::write(schema_dir.join("schema.yaml"), yaml).expect("write schema.yaml");
        }
        tmp
    }

    /// Minimal `schema.yaml` body carrying just the v2 `artifacts:`
    /// block. Mirrors the four artifact entries the resolver knows
    /// about; deliberately scoped to v1 keys so a future schema
    /// extension (e.g. `components.yaml`) does not silently change
    /// what this test exercises.
    const V2_SCHEMA_YAML: &str = r"name: vectis
version: 2
description: test
pipeline:
  define: []
  build: []
  merge: []
artifacts:
  layout:
    role: input
    paths:
      change_local: .specify/slices/<name>/layout.yaml
      project: design-system/layout.yaml
  tokens:
    role: input
    paths:
      change_local: .specify/slices/<name>/tokens.yaml
      project: design-system/tokens.yaml
  assets:
    role: input
    paths:
      change_local: .specify/slices/<name>/assets.yaml
      project: design-system/assets.yaml
  composition:
    role: define-output
    paths:
      change_local: .specify/slices/<name>/composition.yaml
      baseline: .specify/specs/composition.yaml
";

    /// `find_project_root` walks up from a starting path until it
    /// finds a `.specify/` ancestor. A starting path that is itself
    /// the project root resolves cleanly; a starting path nested
    /// under the root walks up to find it. A path with no Specify
    /// ancestor returns `None`.
    #[test]
    fn find_project_root_walks_up_to_specify_dir() {
        let tmp = write_specify_project(None);
        let nested = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&nested).expect("mkdir nested");

        // Direct hit: the start path is the project root itself.
        assert_eq!(find_project_root(tmp.path()).as_deref(), Some(tmp.path()));
        // Nested directory: walks up.
        assert_eq!(find_project_root(&nested).as_deref(), Some(tmp.path()));
        // A file inside a nested directory: walks up from its parent.
        let file = nested.join("file.yaml");
        std::fs::write(&file, b"version: 1\n").expect("write file");
        assert_eq!(find_project_root(&file).as_deref(), Some(tmp.path()));

        // No ancestor with `.specify/`: a fresh tempdir without it.
        let bare = tempfile::tempdir().expect("tempdir");
        assert!(find_project_root(bare.path()).is_none());
    }

    /// `paths_for_key` reads the on-disk `artifacts:` block when
    /// present and falls back to [`EMBEDDED_ARTIFACT_PATHS`]
    /// otherwise. Both paths produce the same canonical resolution
    /// order for v1.
    #[test]
    fn paths_for_key_prefers_on_disk_block_over_embedded_default() {
        // Embedded fallback (no project.yaml resolved).
        let embedded = paths_for_key(None, "tokens");
        assert_eq!(
            embedded,
            vec![
                ".specify/slices/<name>/tokens.yaml".to_string(),
                "design-system/tokens.yaml".to_string(),
            ]
        );

        // On-disk override that swaps the project path. The on-disk
        // value MUST win.
        let custom = json!({
            "tokens": {
                "paths": {
                    "change_local": ".specify/slices/<name>/tokens.yaml",
                    "project": "custom/path/tokens.yaml",
                }
            }
        });
        let resolved = paths_for_key(Some(&custom), "tokens");
        assert_eq!(
            resolved,
            vec![
                ".specify/slices/<name>/tokens.yaml".to_string(),
                "custom/path/tokens.yaml".to_string(),
            ]
        );

        // Unknown key (e.g. a future artifact) returns an empty
        // candidate list so the caller can fall back to the
        // canonical-default-template helper.
        assert!(paths_for_key(None, "components").is_empty());
    }

    /// `expand_path_template` substitutes `<name>` against every
    /// directory under `.specify/slices/`, sorted alphabetically.
    /// Templates without `<name>` resolve to a single absolute path
    /// rooted at the project root. Templates with `<name>` against a
    /// project that has no `.specify/slices/` directory resolve to
    /// an empty list so the caller skips to the next template.
    #[test]
    fn expand_path_template_handles_name_substitution() {
        let tmp = write_specify_project(None);
        let slices_dir = tmp.path().join(".specify/slices");
        std::fs::create_dir_all(slices_dir.join("zeta")).expect("mkdir zeta");
        std::fs::create_dir_all(slices_dir.join("alpha")).expect("mkdir alpha");

        let with_name = expand_path_template(".specify/slices/<name>/layout.yaml", tmp.path());
        // Sorted: alpha first, zeta second.
        assert_eq!(with_name.len(), 2);
        assert!(with_name[0].ends_with(".specify/slices/alpha/layout.yaml"));
        assert!(with_name[1].ends_with(".specify/slices/zeta/layout.yaml"));

        let without_name = expand_path_template("design-system/layout.yaml", tmp.path());
        assert_eq!(without_name.len(), 1);
        assert!(without_name[0].ends_with("design-system/layout.yaml"));

        // Project with no `.specify/slices/` directory: the
        // template resolves to no candidates rather than panicking.
        let empty = tempfile::tempdir().expect("tempdir");
        let no_changes = expand_path_template(".specify/slices/<name>/x.yaml", empty.path());
        assert!(no_changes.is_empty());
    }

    /// The default-path resolver's primary acceptance bullet: when
    /// no `[path]` is supplied, `validate layout` discovers
    /// `.specify/slices/<active>/layout.yaml` first (the
    /// `change_local` template) before falling back to
    /// `design-system/layout.yaml` (the `project` template).
    #[test]
    fn resolve_default_path_prefers_change_local_over_project() {
        let tmp = write_specify_project(Some(V2_SCHEMA_YAML));
        let change_dir = tmp.path().join(".specify/slices/active");
        std::fs::create_dir_all(&change_dir).expect("mkdir change");
        std::fs::write(change_dir.join("layout.yaml"), "version: 1\nscreens: {}\n")
            .expect("write layout.yaml");
        // Also create the project-shape file so the resolver could
        // pick either; assert that change-local wins.
        let design = tmp.path().join("design-system");
        std::fs::create_dir_all(&design).expect("mkdir design-system");
        std::fs::write(design.join("layout.yaml"), "version: 1\nscreens: {}\n")
            .expect("write design-system/layout.yaml");

        let resolved = resolve_default_path_with_root(ValidateMode::Layout, tmp.path());
        assert!(
            resolved.ends_with(".specify/slices/active/layout.yaml"),
            "expected change-local resolution, got: {}",
            resolved.display(),
        );
    }

    /// When the change-local file is absent but the project-shape
    /// exists, `validate layout` falls back to `design-system/`.
    #[test]
    fn resolve_default_path_falls_back_to_project_when_change_local_missing() {
        let tmp = write_specify_project(Some(V2_SCHEMA_YAML));
        let design = tmp.path().join("design-system");
        std::fs::create_dir_all(&design).expect("mkdir design-system");
        std::fs::write(design.join("tokens.yaml"), "version: 1\n").expect("write tokens.yaml");

        let resolved = resolve_default_path_with_root(ValidateMode::Tokens, tmp.path());
        assert!(
            resolved.ends_with("design-system/tokens.yaml"),
            "expected project-shape resolution, got: {}",
            resolved.display(),
        );
    }

    /// When neither template resolves, the resolver returns the
    /// last candidate (the project / baseline shape) so the caller's
    /// "<file>.yaml not readable" error names the most
    /// operator-friendly path.
    #[test]
    fn resolve_default_path_returns_last_candidate_when_nothing_exists() {
        let tmp = write_specify_project(Some(V2_SCHEMA_YAML));
        // No layout.yaml / tokens.yaml / etc. on disk.
        let layout = resolve_default_path_with_root(ValidateMode::Layout, tmp.path());
        assert!(
            layout.ends_with("design-system/layout.yaml"),
            "expected design-system/layout.yaml fallback, got: {}",
            layout.display(),
        );
        let composition = resolve_default_path_with_root(ValidateMode::Composition, tmp.path());
        assert!(
            composition.ends_with(".specify/specs/composition.yaml"),
            "expected baseline composition fallback, got: {}",
            composition.display(),
        );
    }

    /// Removing the `artifacts:` block from `schema.yaml` (the v1
    /// shape every other Specify schema ships today) MUST fall back
    /// to the embedded defaults cleanly. Identical resolution shape
    /// to the on-disk-block tests above.
    #[test]
    fn resolve_default_path_falls_back_to_embedded_defaults_without_block() {
        // Schema file without an `artifacts:` block (mirroring the
        // omnia / contracts schemas that don't carry the v1
        // contract).
        let schema_no_artifacts = r"name: vectis
version: 2
description: test
pipeline:
  define: []
  build: []
  merge: []
";
        let tmp = write_specify_project(Some(schema_no_artifacts));
        let design = tmp.path().join("design-system");
        std::fs::create_dir_all(&design).expect("mkdir design-system");
        std::fs::write(design.join("tokens.yaml"), "version: 1\n").expect("write tokens.yaml");

        let resolved = resolve_default_path_with_root(ValidateMode::Tokens, tmp.path());
        assert!(
            resolved.ends_with("design-system/tokens.yaml"),
            "expected embedded-default resolution, got: {}",
            resolved.display(),
        );
    }

    /// `discover_artifact` is the cross-artifact discovery helper
    /// `validate_assets` (Phase 1.7) and `validate_composition`
    /// (Phase 1.9) call. It returns `Some(path)` only when the file
    /// is actually on disk -- never the "best guess" fallback path
    /// the per-mode resolver returns. This pins that contract
    /// distinction: `Some` means "we found it"; `None` means "no
    /// sibling was found, skip cross-artifact resolution".
    #[test]
    fn discover_artifact_returns_some_only_for_existing_files() {
        let tmp = write_specify_project(Some(V2_SCHEMA_YAML));
        let comp_dir = tmp.path().join(".specify/specs");
        std::fs::create_dir_all(&comp_dir).expect("mkdir specs");
        std::fs::write(comp_dir.join("composition.yaml"), "version: 1\nscreens: {}\n")
            .expect("write composition.yaml");
        let assets_path = tmp.path().join("design-system/assets.yaml");
        std::fs::create_dir_all(assets_path.parent().expect("parent"))
            .expect("mkdir design-system");
        std::fs::write(&assets_path, "version: 1\nassets: {}\n").expect("write assets.yaml");

        // Discovering composition from assets.yaml: composition is
        // on disk → Some.
        let found = discover_artifact(&assets_path, ValidateMode::Composition);
        assert!(
            found.as_deref().is_some_and(|p| p.ends_with(".specify/specs/composition.yaml")),
            "expected composition discovery to succeed, got: {found:?}",
        );

        // Discovering tokens from assets.yaml: tokens is NOT on
        // disk (no design-system/tokens.yaml) → None. The
        // per-mode resolver would return a "best guess" path here;
        // discover_artifact does not.
        let missing = discover_artifact(&assets_path, ValidateMode::Tokens);
        assert!(missing.is_none(), "expected tokens discovery to return None, got: {missing:?}");

        // A start path with no Specify ancestor returns None.
        let bare = tempfile::tempdir().expect("tempdir");
        assert!(
            discover_artifact(bare.path(), ValidateMode::Composition).is_none(),
            "expected None for non-Specify starting paths"
        );
    }

    /// The `.specify/.cache/<schema>/schema.yaml` shape (cached by
    /// the agent) must resolve identically to the local
    /// `<root>/schemas/<schema>/schema.yaml` shape exercised by the
    /// other tests. Pinning both paths so a future cache vs. local
    /// preference flip surfaces here.
    #[test]
    fn read_artifacts_block_finds_cached_schema_when_local_is_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dot_specify = tmp.path().join(".specify");
        std::fs::create_dir_all(&dot_specify).expect("mkdir .specify");
        std::fs::write(dot_specify.join("project.yaml"), "name: demo\nschema: vectis\n")
            .expect("write project.yaml");
        // Place the schema under `.specify/.cache/vectis/schema.yaml`
        // -- the agent-managed cache path -- and NOT under
        // `<root>/schemas/vectis/`.
        let cache_dir = dot_specify.join(".cache").join("vectis");
        std::fs::create_dir_all(&cache_dir).expect("mkdir cache");
        std::fs::write(cache_dir.join("schema.yaml"), V2_SCHEMA_YAML)
            .expect("write cached schema.yaml");

        let artifacts = read_artifacts_block(tmp.path()).expect("artifacts found in cache");
        // Spot-check: every v1 key is present.
        for key in ["layout", "tokens", "assets", "composition"] {
            assert!(artifacts.get(key).is_some(), "expected `{key}` in artifacts: {artifacts}");
        }
    }

    /// `read_artifacts_block` interprets URL-shaped `schema:`
    /// values (e.g. `https://.../vectis@1`) by extracting the last
    /// path segment minus any `@<ref>` suffix. This mirrors the
    /// canonical resolution in `crates/schema/src/schema.rs`.
    #[test]
    fn schema_name_from_value_handles_urls_and_refs() {
        assert_eq!(schema_name_from_value("vectis"), "vectis");
        assert_eq!(schema_name_from_value("https://example.com/schemas/vectis"), "vectis");
        assert_eq!(schema_name_from_value("https://example.com/schemas/vectis@1.2.3"), "vectis");
        assert_eq!(schema_name_from_value("https://example.com/schemas/vectis@1.2.3/"), "vectis");
    }

    // -------------------------------------------------------------
    // Phase 1.10: `validate all` envelope shape + sub-mode dispatch
    // -------------------------------------------------------------

    /// The combined-run envelope MUST carry `mode: "all"`, the
    /// project root in `path`, and a `results` array with exactly
    /// four sub-reports in the canonical order layout → composition
    /// → tokens → assets. Each sub-report has its own per-mode
    /// envelope under `report`.
    #[test]
    fn all_envelope_runs_every_mode_in_canonical_order() {
        let tmp = write_specify_project(Some(V2_SCHEMA_YAML));

        // Materialise every artifact at the project shape so each
        // sub-mode produces a real (non-skipped) report.
        let design = tmp.path().join("design-system");
        std::fs::create_dir_all(&design).expect("mkdir design-system");
        std::fs::write(design.join("layout.yaml"), "version: 1\nscreens: {}\n")
            .expect("write layout.yaml");
        std::fs::write(design.join("tokens.yaml"), "version: 1\n").expect("write tokens.yaml");
        std::fs::write(design.join("assets.yaml"), "version: 1\nassets: {}\n")
            .expect("write assets.yaml");
        let specs = tmp.path().join(".specify/specs");
        std::fs::create_dir_all(&specs).expect("mkdir specs");
        std::fs::write(specs.join("composition.yaml"), "version: 1\nscreens: {}\n")
            .expect("write composition.yaml");

        let envelope = extract_envelope(
            run(&ValidateArgs {
                mode: ValidateMode::All,
                path: Some(tmp.path().to_path_buf()),
            })
            .expect("run all succeeds"),
        );

        assert_eq!(envelope["mode"], "all");
        assert_eq!(
            envelope["path"].as_str().expect("path string"),
            tmp.path().display().to_string()
        );
        let results = envelope["results"].as_array().expect("results array");
        assert_eq!(results.len(), 4, "expected four sub-reports: {envelope}");
        assert_eq!(results[0]["mode"], "layout");
        assert_eq!(results[1]["mode"], "composition");
        assert_eq!(results[2]["mode"], "tokens");
        assert_eq!(results[3]["mode"], "assets");

        // Each sub-report must have a real per-mode envelope (NOT
        // the skipped synthetic shape).
        for entry in results {
            let report = &entry["report"];
            assert!(report.get("skipped").is_none(), "unexpected skipped: {entry}");
            assert_eq!(
                report["errors"].as_array().map(Vec::len),
                Some(0),
                "{}: unexpected errors: {entry}",
                entry["mode"]
            );
        }
    }

    /// Sub-modes whose default-resolved input does not exist on
    /// disk MUST surface as a synthetic `{ skipped: true }`
    /// sub-report rather than a hard `InvalidProject` failure -- so
    /// `validate all` keeps running through the rest of the modes.
    #[test]
    fn all_envelope_skips_missing_inputs_without_failing() {
        let tmp = write_specify_project(Some(V2_SCHEMA_YAML));
        // Provide ONLY tokens.yaml; the other three are absent.
        let design = tmp.path().join("design-system");
        std::fs::create_dir_all(&design).expect("mkdir design-system");
        std::fs::write(design.join("tokens.yaml"), "version: 1\n").expect("write tokens.yaml");

        let envelope = extract_envelope(
            run(&ValidateArgs {
                mode: ValidateMode::All,
                path: Some(tmp.path().to_path_buf()),
            })
            .expect("run all does not fail on missing inputs"),
        );

        let results = envelope["results"].as_array().expect("results array");
        let by_mode: std::collections::BTreeMap<&str, &Value> =
            results.iter().map(|e| (e["mode"].as_str().expect("mode str"), e)).collect();

        for skipped_mode in ["layout", "composition", "assets"] {
            let report = &by_mode[skipped_mode]["report"];
            assert_eq!(
                report["skipped"],
                Value::Bool(true),
                "[{skipped_mode}] expected skipped: {report}",
            );
            assert_eq!(
                report["errors"].as_array().map(Vec::len),
                Some(0),
                "[{skipped_mode}] errors must stay empty: {report}"
            );
        }
        let tokens_report = &by_mode["tokens"]["report"];
        assert!(
            tokens_report.get("skipped").is_none(),
            "tokens.yaml IS on disk; skipped MUST be absent: {tokens_report}",
        );
    }

    /// A sub-mode's findings MUST surface inside `results[*].report`
    /// so the dispatcher's recursion-aware `validate_exit_code`
    /// helper picks them up. This test feeds a deliberately-broken
    /// tokens.yaml and asserts the broken-hex error rides the
    /// nested sub-report.
    #[test]
    fn all_envelope_propagates_sub_mode_errors_into_nested_report() {
        let tmp = write_specify_project(Some(V2_SCHEMA_YAML));
        let design = tmp.path().join("design-system");
        std::fs::create_dir_all(&design).expect("mkdir design-system");
        std::fs::write(
            design.join("tokens.yaml"),
            "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n",
        )
        .expect("write tokens.yaml");

        let envelope = extract_envelope(
            run(&ValidateArgs {
                mode: ValidateMode::All,
                path: Some(tmp.path().to_path_buf()),
            })
            .expect("run all succeeds"),
        );
        let results = envelope["results"].as_array().expect("results array");
        let tokens_entry =
            results.iter().find(|e| e["mode"] == "tokens").expect("tokens sub-report present");
        let tokens_errors =
            tokens_entry["report"]["errors"].as_array().expect("tokens errors array");
        assert!(
            !tokens_errors.is_empty(),
            "broken hex MUST surface in nested tokens report: {envelope}"
        );
    }
}
