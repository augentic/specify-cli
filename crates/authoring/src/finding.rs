use std::path::PathBuf;

use specify_error::{ValidationStatus, ValidationSummary};

/// A single validation finding from a check predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule_id: &'static str,
    pub message: String,
    pub location: Option<Location>,
}

/// File location for a finding (1-based line, optional column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: usize,
    pub column: Option<usize>,
}

impl Finding {
    /// Project this authoring finding into the runtime validation summary shape.
    #[must_use]
    pub fn to_summary(&self) -> ValidationSummary {
        ValidationSummary {
            status: ValidationStatus::Fail,
            rule_id: self.rule_id.to_string(),
            rule: self.rule_id.to_string(),
            detail: Some(match &self.location {
                Some(location) => format!("{}: {}", location.path.display(), self.message),
                None => self.message.clone(),
            }),
        }
    }
}

/// A check predicate that scans the framework repo and returns findings.
pub trait Check {
    fn run(&self, ctx: &crate::context::Context) -> Vec<Finding>;
}
