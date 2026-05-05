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
//! The remaining three modes (`layout`, `composition`, `all`) still
//! return [`CommandOutcome::Stub`] and will be filled in by Phases
//! 1.8-1.10:
//!
//! - **Phase 1.8** -- `layout` mode validates as the unwired subset
//!   of `composition.schema.json`, including the §G structural-
//!   identity rule for any `component:` directives present.
//! - **Phase 1.9** -- `composition` mode adds cross-artifact
//!   resolution and auto-invokes `tokens` / `assets` when sibling
//!   files exist.
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

/// Default fallback path for `tokens.yaml` when no `[path]` argument is
/// supplied (RFC-11 §H "Inputs"). Phase 1.10 layers `artifacts:`-block
/// resolution on top of this; until then the canonical fallback is
/// the project-relative path documented in the RFC.
const DEFAULT_TOKENS_PATH: &str = "design-system/tokens.yaml";

/// Default fallback path for `assets.yaml`, mirroring the tokens
/// fallback (RFC-11 §H "Inputs"). Phase 1.10 layers `artifacts:`-block
/// resolution on top of this.
const DEFAULT_ASSETS_PATH: &str = "design-system/assets.yaml";

/// Lazily compiled tokens validator. Compiling once per process avoids
/// re-parsing the embedded schema on every invocation; in practice the
/// CLI runs one mode per process today, but Phase 1.10's `validate
/// all` will fan out and exercise every mode in a single dispatch.
static TOKENS_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Lazily compiled assets validator (companion to [`TOKENS_VALIDATOR`]).
static ASSETS_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

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

    /// Stub modes (every mode except `tokens` + `assets` after Phases
    /// 1.6 / 1.7) MUST continue to return [`CommandOutcome::Stub`]
    /// until the corresponding phase lands. This pins the regression
    /// so accidentally flipping a mode to `Success` shows up in CI.
    #[test]
    fn stub_modes_still_return_stub() {
        for (mode, expected) in [
            (ValidateMode::Layout, "validate layout"),
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
}
