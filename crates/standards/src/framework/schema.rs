use std::fs;
use std::path::Path;

use jsonschema::Validator;
use serde_json::Value as JsonValue;
use specify_schema::{
    FRAMEWORK_JSON_SCHEMA, MARKETPLACE_JSON_SCHEMA, RULE_JSON_SCHEMA, SCENARIO_JSON_SCHEMA,
    SKILL_JSON_SCHEMA,
};

use crate::framework::error::ToolingError;
use crate::framework::helpers::skill_frontmatter;

/// Framework authoring JSON Schema identifiers.
///
/// Each variant resolves to an embedded `&'static` constant compiled
/// into the binary via [`specify_schema`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaId {
    Skill,
    Rule,
    Scenario,
    Marketplace,
    Framework,
}

impl SchemaId {
    /// Embedded `&'static` schema source for `schema_id`. The pointer
    /// identity of these constants keys the shared [`specify_schema`]
    /// validator cache.
    const fn source(self) -> &'static str {
        match self {
            Self::Skill => SKILL_JSON_SCHEMA,
            Self::Rule => RULE_JSON_SCHEMA,
            Self::Scenario => SCENARIO_JSON_SCHEMA,
            Self::Marketplace => MARKETPLACE_JSON_SCHEMA,
            Self::Framework => FRAMEWORK_JSON_SCHEMA,
        }
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

/// Lazily compile a framework schema via the shared [`specify_schema`]
/// validator cache (one schema-cache implementation, keyed on the
/// embedded constant's pointer identity).
///
/// # Errors
///
/// [`ToolingError::Infrastructure`] when the embedded schema cannot be
/// compiled or the shared cache lock is poisoned (a corrupt-binary or
/// prior-panic signal).
pub fn validator(schema_id: SchemaId) -> Result<std::sync::Arc<Validator>, ToolingError> {
    specify_schema::cached_validator(schema_id.source())
        .map_err(|err| ToolingError::Infrastructure(err.to_string()))
}

/// Validate a parsed JSON/YAML value against a framework schema.
pub fn validate_value(value: &JsonValue, schema_id: SchemaId) -> Result<(), SchemaError> {
    let compiled = validator(schema_id)?;
    collect_errors(&compiled, value).map_err(SchemaError::Validation)
}

/// Extract YAML frontmatter from a Markdown file and validate it against `schema_id`.
pub fn validate_frontmatter(
    path: impl AsRef<Path>, schema_id: SchemaId,
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
    validate_value(&value, schema_id)
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
mod tests;
