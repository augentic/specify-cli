//! SKILL.md frontmatter predicates.
//!
//! The five checks share one SKILL.md walk (`entries`, memoised per
//! [`crate::framework::context::Context`]) and split per concern:
//! `schema` (`skill.missing-frontmatter` / `skill.schema-violation`),
//! `name_dir` (`skill.name-directory-mismatch`), `unknown_tool`
//! (`skill.unknown-tool`), `description` (`skill.description-grammar`),
//! and `argument_hint` (`skill.argument-hint-grammar`). This root owns
//! the shared rule ids and the `finding` constructor, and re-exports
//! the public surface.

mod argument_hint;
mod description;
mod entries;
mod name_dir;
mod schema;
mod unknown_tool;

#[cfg(test)]
mod tests;

use std::path::Path;

use specify_diagnostics::Diagnostic;

pub use self::argument_hint::{ArgumentHintGrammar, argument_hint_grammar_error};
pub use self::description::DescriptionGrammar;
pub use self::name_dir::NameDirMismatch;
pub use self::schema::{
    FrontmatterSchema, findings_missing_frontmatter, findings_schema_violation,
};
pub use self::unknown_tool::UnknownTool;
use crate::framework::builder::{framework_finding, loc};

pub const RULE_SCHEMA_VIOLATION: &str = "skill.schema-violation";
pub const RULE_MISSING_FRONTMATTER: &str = "skill.missing-frontmatter";
pub const RULE_NAME_DIRECTORY_MISMATCH: &str = "skill.name-directory-mismatch";
pub const RULE_UNKNOWN_TOOL: &str = "skill.unknown-tool";
pub const RULE_DESCRIPTION_GRAMMAR: &str = "skill.description-grammar";
pub const RULE_ARGUMENT_HINT_GRAMMAR: &str = "skill.argument-hint-grammar";

/// Kept in sync with `schemas/authoring/skill.schema.json` (`description.maxLength`).
pub const MAX_DESCRIPTION_CHARS: usize = 512;

/// Build a SKILL.md frontmatter [`Diagnostic`] anchored at line 1 of
/// `path`. Shared by the five frontmatter predicates.
fn finding(rule_id: &'static str, message: impl Into<String>, path: &Path) -> Diagnostic {
    framework_finding(rule_id, message.into(), Some(loc(path, 1, None)))
}
