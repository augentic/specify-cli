//! Shared primitives: embedded schemas, lazy-compiled validators, and
//! the JSON-Pointer / YAML helpers every per-mode handler reuses.

use std::path::Path;
use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

use crate::validate::error::VectisError;

/// Embedded `tokens.schema.json`. Vendored from the upstream
/// `capabilities/vectis/tokens.schema.json` in the `specify` repo; the
/// upstream is canonical and any edit there must be mirrored here
/// byte-for-byte.
const TOKENS_SCHEMA_SOURCE: &str = include_str!("../../../embedded/tokens.schema.json");

/// Embedded `assets.schema.json`. Vendored from the upstream
/// `capabilities/vectis/assets.schema.json` in the `specify` repo;
/// same byte-identity discipline as the tokens copy.
const ASSETS_SCHEMA_SOURCE: &str = include_str!("../../../embedded/assets.schema.json");

/// Embedded `composition.schema.json`. Vendored from the upstream
/// `capabilities/vectis/composition.schema.json` in the `specify`
/// repo. Shared between `layout` mode (unwired-subset runtime) and
/// `composition` mode (full lifecycle runtime); same byte-identity
/// discipline as the tokens / assets copies.
const COMPOSITION_SCHEMA_SOURCE: &str = include_str!("../../../embedded/composition.schema.json");

/// Lazily compiled tokens validator. Compiling once per process avoids
/// re-parsing the embedded schema on every invocation; `validate all`
/// fans out across every mode in a single dispatch, so the cache pays
/// off.
static TOKENS_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Lazily compiled assets validator (companion to [`TOKENS_VALIDATOR`]).
static ASSETS_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Lazily compiled composition validator. Shared between `layout` mode
/// and `composition` mode: one schema, two runtime layers on top.
static COMPOSITION_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Compile the embedded tokens schema once and re-use the validator.
///
/// # Errors
///
/// Returns [`VectisError::Internal`] if the embedded JSON is
/// unparseable or the schema fails to compile.
pub fn tokens_validator() -> Result<&'static Validator, VectisError> {
    lazy_validator(&TOKENS_VALIDATOR, TOKENS_SCHEMA_SOURCE, "tokens.schema.json")
}

/// Compile the embedded assets schema once and re-use the validator.
///
/// # Errors
///
/// Returns [`VectisError::Internal`] when the embedded JSON is
/// unparseable or the schema fails to compile.
pub fn assets_validator() -> Result<&'static Validator, VectisError> {
    lazy_validator(&ASSETS_VALIDATOR, ASSETS_SCHEMA_SOURCE, "assets.schema.json")
}

/// Compile the embedded composition schema once and re-use the
/// validator. Shared between `layout` mode and `composition` mode.
///
/// # Errors
///
/// Returns [`VectisError::Internal`] when the embedded JSON is
/// unparseable or the schema fails to compile.
pub fn composition_validator() -> Result<&'static Validator, VectisError> {
    lazy_validator(&COMPOSITION_VALIDATOR, COMPOSITION_SCHEMA_SOURCE, "composition.schema.json")
}

/// Generic helper for the embedded-schema lazy-compile pattern. The
/// cell stores `Result<Validator, String>` so a build-time invariant
/// breach (the embedded JSON is unparseable, or the schema itself is
/// invalid) survives across `OnceLock` initialisation without
/// re-running the failing branch on every call.
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

/// Read `path` and parse it as YAML into a [`serde_json::Value`].
///
/// The `Option<Value>` return shape is intentional: composition mode
/// only calls this after auto-invoking the sibling validator, so any
/// read / parse failure has already surfaced in the folded sub-report.
/// Returning `None` lets the call site stay flat with `if let Some(...)`
/// instead of dragging a synthetic error type through.
pub(super) fn parse_yaml_file(path: &Path) -> Option<Value> {
    let source = std::fs::read_to_string(path).ok()?;
    serde_saphyr::from_str::<Value>(&source).ok()
}

/// Escape a JSON Pointer reference token: `~` becomes `~0` and `/`
/// becomes `~1`. Asset ids are kebab-case so neither substitution
/// fires for the common case, but composition keys (e.g. screen slugs)
/// MAY contain slashes if a future schema relaxation permits, so the
/// escape is safe rather than redundant.
pub(super) fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}
