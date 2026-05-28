use std::fs;
use std::path::{Path, PathBuf};

use jsonschema::Validator;
use serde_json::Value as JsonValue;
use specify_schema::RULE_JSON_SCHEMA;

use crate::context::Context;
use crate::error::ToolingError;
use crate::helpers::skill_frontmatter;

/// Cache key for the embedded codex-rule schema. The schema is compiled from
/// the in-memory [`RULE_JSON_SCHEMA`] constant rather than a filesystem
/// path, so the synthetic key keeps the rest of [`Context::schema_cache`]
/// machinery uniform.
const EMBEDDED_CODEX_RULE_CACHE_KEY: &str = "<embedded>/rule.schema.json";

/// Framework-only JSON Schema identifiers.
///
/// `Rule` resolves to the embedded constant
/// [`specify_schema::RULE_JSON_SCHEMA`]; the rest resolve to files under
/// `crates/authoring/schemas/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaId {
    Skill,
    Rule,
    Scenario,
    Marketplace,
}

impl SchemaId {
    /// Basename of the schema file in `crates/authoring/schemas/`, when one
    /// exists. Returns `None` for schemas served from an embedded constant.
    pub const fn file_name(self) -> Option<&'static str> {
        match self {
            Self::Skill => Some("skill.schema.json"),
            Self::Rule => None,
            Self::Scenario => Some("scenario.schema.json"),
            Self::Marketplace => Some("marketplace.schema.json"),
        }
    }
}

/// Embedded schema source for `schema_id`, when one is compiled in.
fn embedded_schema(schema_id: SchemaId) -> Option<(PathBuf, &'static str)> {
    match schema_id {
        SchemaId::Rule => Some((PathBuf::from(EMBEDDED_CODEX_RULE_CACHE_KEY), RULE_JSON_SCHEMA)),
        SchemaId::Skill | SchemaId::Scenario | SchemaId::Marketplace => None,
    }
}

/// Schema validation failure: infrastructure problem or one or more constraint violations.
#[derive(Debug)]
pub enum SchemaError {
    Infrastructure(ToolingError),
    Validation(Vec<ValidationError>),
}

impl From<ToolingError> for SchemaError {
    fn from(error: ToolingError) -> Self {
        Self::Infrastructure(error)
    }
}

/// One JSON Schema validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub instance_path: String,
    pub message: String,
}

/// Resolve the authoritative schema path for `schema_id`. Returns `None` for
/// schemas served from an embedded constant.
pub fn schema_path(ctx: &Context, schema_id: SchemaId) -> Option<PathBuf> {
    schema_id.file_name().map(|name| ctx.framework_schema_dir().join(name))
}

/// Lazily compile a framework schema via the shared context cache.
///
/// Schemas backed by an in-memory constant (today only
/// `SchemaId::Rule`, sourced from [`RULE_JSON_SCHEMA`]) compile
/// from the embedded bytes and are cached under a synthetic path; the rest
/// read from `crates/authoring/schemas/`.
pub fn validator(
    ctx: &Context, schema_id: SchemaId,
) -> Result<std::sync::Arc<Validator>, ToolingError> {
    if let Some((key, source)) = embedded_schema(schema_id) {
        return ctx.schema_from_source(key, source);
    }
    let path = schema_path(ctx, schema_id)
        .ok_or_else(|| ToolingError::Infrastructure(format!("no schema path for {schema_id:?}")))?;
    ctx.schema(path)
}

/// Validate a parsed JSON/YAML value against a framework schema.
pub fn validate_value(
    ctx: &Context, value: &JsonValue, schema_id: SchemaId,
) -> Result<(), SchemaError> {
    let compiled = validator(ctx, schema_id)?;
    collect_errors(&compiled, value).map_err(SchemaError::Validation)
}

/// Extract YAML frontmatter from a Markdown file and validate it against `schema_id`.
pub fn validate_frontmatter(
    ctx: &Context, path: impl AsRef<Path>, schema_id: SchemaId,
) -> Result<(), SchemaError> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|source| {
        SchemaError::Infrastructure(ToolingError::Infrastructure(format!(
            "read {}: {source}",
            path.display()
        )))
    })?;

    let Some(frontmatter) = skill_frontmatter(&content) else {
        return Err(SchemaError::Validation(vec![ValidationError {
            instance_path: "/".into(),
            message: "missing leading YAML frontmatter delimited by ---".into(),
        }]));
    };

    let value = JsonValue::Object(frontmatter.into_iter().collect());
    validate_value(ctx, &value, schema_id)
}

/// Shared validation error collection for checks and acceptance tests.
pub fn collect_errors(compiled: &Validator, value: &JsonValue) -> Result<(), Vec<ValidationError>> {
    if compiled.is_valid(value) {
        return Ok(());
    }

    let errors = compiled
        .iter_errors(value)
        .map(|error| ValidationError {
            instance_path: error.instance_path().to_string(),
            message: error.to_string(),
        })
        .collect();

    Err(errors)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn validate_frontmatter_rejects_invalid_skill_description() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tempdir.path().join("plugins")).expect("plugins");
        std::fs::create_dir_all(tempdir.path().join("adapters")).expect("adapters");
        let ctx = Context::from_framework_root(tempdir.path()).expect("framework root resolves");
        let mut temp = tempfile::NamedTempFile::new().expect("temp file");
        write!(temp, "---\nname: spec-test-skill\ndescription: Too short.\n---\n")
            .expect("write temp frontmatter");

        let result = validate_frontmatter(&ctx, temp.path(), SchemaId::Skill);
        let SchemaError::Validation(errors) = result.expect_err("invalid frontmatter should fail")
        else {
            panic!("expected validation errors");
        };
        assert!(
            errors.iter().any(|error| {
                error.instance_path.contains("description") || error.message.contains("Use when")
            }),
            "expected description validation error, got {errors:?}"
        );
    }

    #[test]
    fn validate_frontmatter_accepts_minimal_valid_skill() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tempdir.path().join("plugins")).expect("plugins");
        std::fs::create_dir_all(tempdir.path().join("adapters")).expect("adapters");
        let ctx = Context::from_framework_root(tempdir.path()).expect("framework root resolves");
        let mut temp = tempfile::NamedTempFile::new().expect("temp file");
        write!(
            temp,
            "---\nname: spec-test-skill\ndescription: Test specification skill behavior. Use when validating schema tests.\n---\n"
        )
        .expect("write temp frontmatter");
        validate_frontmatter(&ctx, temp.path(), SchemaId::Skill)
            .unwrap_or_else(|error| panic!("valid frontmatter should validate: {error:?}"));
    }
}
