//! `specify vectis validate <mode> [path]` -- schema and cross-artifact
//! validation surface (RFC-11 §H, §I).
//!
//! Phase 1.6 wired the `tokens` mode against the embedded
//! `schemas/vectis/tokens.schema.json` (Appendix A; vendored from the
//! `specify` repo at `crates/vectis/embedded/tokens.schema.json`).
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
//! `crates/vectis/embedded/composition.schema.json` -- the same
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
//!    [`check_structural_identity`]); the engine consumes
//!    `*-when`-key *presence* (forbidden in layout, allowed in
//!    composition) but ignores `*-when` *condition values* per §G's
//!    edge-case enumeration.
//!
//! Cross-artifact reference checks (token / asset id resolution
//! from inside `layout.yaml`) are deliberately deferred to Phase 1.9.
//! The plan note in Phase 1.8 ("Cross-artifact reference checks are
//! also §A behavior; layered exactly the same way as Phase 1.9 does
//! for composition") is an architectural pointer rather than a Phase
//! 1.8 deliverable: §A's verification step describes the inferer
//! invoking those checks, and §H's `layout` mode bullet enumerates
//! "YAML syntax, schema shape, `screens` only, no `delta`, no
//! define-owned wiring keys, and the §G structural-identity rule"
//! without mentioning auto-invoke. Phase 1.9 owns the auto-invoke
//! mechanism; once it lands, layout mode can opt into it via the
//! same shared helper without disturbing Phase 1.8's surface.
//!
//! The remaining two modes (`composition`, `all`) still return
//! [`CommandOutcome::Stub`] and will be filled in by Phases 1.9
//! and 1.10:
//!
//! - **Phase 1.9** -- `composition` mode adds cross-artifact
//!   resolution and auto-invokes `tokens` / `assets` when sibling
//!   files exist. Reuses the structural-identity engine landed
//!   in Phase 1.8.
//! - **Phase 1.10** -- `all` runs the four modes in turn, plus the
//!   `artifacts:`-block default-path resolution every mode shares.
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
//! dispatcher (in `src/commands/vectis.rs::run_vectis`) translates
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

/// Default fallback path for `tokens.yaml` when no `[path]` argument is
/// supplied (RFC-11 §H "Inputs"). Phase 1.10 layers `artifacts:`-block
/// resolution on top of this; until then the canonical fallback is
/// the project-relative path documented in the RFC.
const DEFAULT_TOKENS_PATH: &str = "design-system/tokens.yaml";

/// Default fallback path for `assets.yaml`, mirroring the tokens
/// fallback (RFC-11 §H "Inputs"). Phase 1.10 layers `artifacts:`-block
/// resolution on top of this.
const DEFAULT_ASSETS_PATH: &str = "design-system/assets.yaml";

/// Default fallback path for `layout.yaml`, mirroring the tokens /
/// assets fallbacks (RFC-11 §H "Inputs"). Phase 1.10 layers
/// `artifacts:`-block resolution on top of this; until then the
/// canonical fallback is the project-relative path the RFC names.
const DEFAULT_LAYOUT_PATH: &str = "design-system/layout.yaml";

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
/// Phases 1.6 + 1.7 implement `tokens` and `assets`; the other three
/// modes still return [`CommandOutcome::Stub`] with a `command`
/// string of the form `"validate <mode>"` so the dispatcher in
/// `src/commands/vectis.rs` emits the v2 `not-implemented` envelope
/// unchanged.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved
/// `tokens.yaml` / `assets.yaml` is unreadable (missing file,
/// permission denied) and [`VectisError::Internal`] if an embedded
/// schema fails to compile (a build-time invariant violation -- both
/// schemas ship with the binary). YAML parse failures and schema
/// validation failures are *not* errors at this layer; they are
/// folded into the `errors` array of the per-mode envelope so the
/// operator sees the full report alongside any other findings.
pub fn run(args: &ValidateArgs) -> Result<CommandOutcome, VectisError> {
    match args.mode {
        ValidateMode::Tokens => validate_tokens(args.path.as_deref()),
        ValidateMode::Assets => validate_assets(args.path.as_deref()),
        ValidateMode::Layout => validate_layout(args.path.as_deref()),
        mode => Ok(CommandOutcome::Stub {
            command: stub_command(mode),
        }),
    }
}

/// Stub command identifier for not-yet-implemented modes. The string
/// MUST match the kebab-case spelling in [`ValidateMode::as_str`] so
/// the v2 `not-implemented` envelope's `command` field stays
/// consistent across modes.
const fn stub_command(mode: ValidateMode) -> &'static str {
    match mode {
        ValidateMode::Layout => "validate layout",
        ValidateMode::Composition => "validate composition",
        ValidateMode::Tokens => "validate tokens",
        ValidateMode::Assets => "validate assets",
        ValidateMode::All => "validate all",
    }
}

/// Validate `tokens.yaml` against the embedded Appendix A schema.
///
/// Resolution order for the file path:
/// 1. The explicit `[path]` positional, when supplied.
/// 2. The canonical fallback `design-system/tokens.yaml` (relative to
///    the current working directory).
///
/// Phase 1.10 adds an `artifacts:`-block lookup between (1) and (2);
/// until then the canonical fallback is the only default the CLI
/// honours.
fn validate_tokens(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target =
        path.map_or_else(|| PathBuf::from(DEFAULT_TOKENS_PATH), std::path::Path::to_path_buf);

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
/// the explicit `[path]` positional wins, otherwise fall back to the
/// canonical `design-system/assets.yaml`.
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
///    then look at `.specify/changes/<name>/composition.yaml`
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
    let target =
        path.map_or_else(|| PathBuf::from(DEFAULT_ASSETS_PATH), std::path::Path::to_path_buf);

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

        // Cross-artifact composition reference resolution. Phase 1.10
        // replaces this walk with the `artifacts:`-block cascade; for
        // now we use a project-root walk that mirrors the canonical
        // paths from RFC-11 §H "Inputs".
        if let Some(comp) = find_sibling_composition(&target)
            && let Ok(comp_value) = serde_saphyr::from_str::<Value>(&comp.source)
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
                            comp.path.display(),
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
/// otherwise fall back to the canonical `design-system/layout.yaml`
/// (Phase 1.10 layers the `artifacts:`-block cascade on top).
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
    let target =
        path.map_or_else(|| PathBuf::from(DEFAULT_LAYOUT_PATH), std::path::Path::to_path_buf);

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

/// Minimal record of a discovered sibling composition. Carries the
/// raw YAML so the caller can decide whether to parse, and the
/// resolved path so the resulting error messages can name it.
struct SiblingComposition {
    path: PathBuf,
    source: String,
}

/// Walk up from the directory containing `assets_path` until a
/// project root marked by `.specify/` is found, then look at the
/// canonical composition locations from RFC-11 §H "Inputs":
///
/// 1. The first `.specify/changes/<name>/composition.yaml` (sorted
///    alphabetically by `<name>`).
/// 2. `.specify/specs/composition.yaml`.
///
/// The first existing path wins. Returns `None` when no project
/// root is found, no composition file is present at either location,
/// or the file cannot be read. Phase 1.10 replaces this walk with
/// the full `artifacts:`-block cascade (`paths.change_local` then
/// `paths.project` then `paths.baseline`).
fn find_sibling_composition(assets_path: &Path) -> Option<SiblingComposition> {
    let mut cursor = assets_path.parent()?.to_path_buf();
    loop {
        let root = cursor.join(".specify");
        if root.is_dir() {
            // Change-local first: the active change overrides the
            // baseline in every other Specify lifecycle phase, so
            // mirror that priority here. We sort the entries so the
            // discovery is deterministic across filesystems that do
            // not iterate alphabetically.
            let changes_dir = root.join("changes");
            if let Ok(entries) = std::fs::read_dir(&changes_dir) {
                let mut names: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                names.sort();
                for change_dir in names {
                    let candidate = change_dir.join("composition.yaml");
                    if let Ok(source) = std::fs::read_to_string(&candidate) {
                        return Some(SiblingComposition {
                            path: candidate,
                            source,
                        });
                    }
                }
            }
            let baseline = root.join("specs").join("composition.yaml");
            if let Ok(source) = std::fs::read_to_string(&baseline) {
                return Some(SiblingComposition {
                    path: baseline,
                    source,
                });
            }
            // `.specify/` exists but no composition was found at
            // either canonical location: stop the walk so we don't
            // accidentally pick up a parent project's `.specify/`.
            return None;
        }
        if !cursor.pop() {
            return None;
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

    /// Stub modes (every mode except `tokens`, `assets`, and
    /// `layout` after Phases 1.6 / 1.7 / 1.8) MUST continue to
    /// return [`CommandOutcome::Stub`] until the corresponding phase
    /// lands. This pins the regression so accidentally flipping a
    /// mode to `Success` shows up in CI.
    #[test]
    fn stub_modes_still_return_stub() {
        for (mode, expected) in [
            (ValidateMode::Composition, "validate composition"),
            (ValidateMode::All, "validate all"),
        ] {
            let args = ValidateArgs { mode, path: None };
            let outcome = run(&args).expect("stub never errors");
            match outcome {
                CommandOutcome::Stub { command } => assert_eq!(command, expected),
                CommandOutcome::Success(value) => {
                    panic!("expected Stub for {mode:?}, got Success({value})")
                }
            }
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
}
