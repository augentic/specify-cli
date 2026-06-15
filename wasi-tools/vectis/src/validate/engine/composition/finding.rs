//! Typed finding shared by the composition sub-checks. Replaces the
//! ad-hoc `json!({ "path": …, "message": … })` literals so every check
//! produces the same wire fragment by construction.

use serde_json::{Value, json};

/// One composition-mode finding: a JSON-Pointer-shaped `path` into the
/// offending document and an operator-facing `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    /// JSON Pointer into the validated document (`""` for whole-file).
    pub(crate) path: String,
    /// Operator-facing description of the violation.
    pub(crate) message: String,
}

impl Finding {
    /// Build a finding from any displayable path / message pair.
    pub(crate) fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl From<Finding> for Value {
    fn from(finding: Finding) -> Self {
        json!({
            "path": finding.path,
            "message": finding.message,
        })
    }
}

/// Project a typed finding list into the envelope's JSON array items.
pub(crate) fn to_values(findings: Vec<Finding>) -> Vec<Value> {
    findings.into_iter().map(Value::from).collect()
}
