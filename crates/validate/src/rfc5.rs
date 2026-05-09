//! RFC-5 framework-lint rule ids that RFC-15 needs before the repo-level
//! linter is fully ported to Rust.
//!
//! `specify-validate` currently validates slice artifacts, not the entire
//! Specify framework repository. The tool declaration checks below therefore
//! live in `specify-tool` for now. The plugin repository's Deno checks carry
//! the interim skill/brief scanner until the eventual RFC-5 `specify check`
//! port can own the same rule.

/// Warns when a declared tool asks for write access to the whole project.
///
/// The explicit `$PROJECT_DIR` form is enforced by `specify-tool` today.
/// TODO(rfc-5): once the framework linter has project context, also flag
/// absolute write paths whose canonical target is the project root. Fixture
/// seed: a `tools.yaml` with `permissions.write: ["$PROJECT_DIR"]`.
pub const TOOL_WRITE_PERMISSION_TOO_BROAD: &str = "tool.write-permission-too-broad";

/// Rejects tool write permissions that target Specify lifecycle state.
///
/// Enforced by `specify-tool` today for manifests loaded by `specify tool`.
pub const TOOL_LIFECYCLE_STATE_WRITE_DENIED: &str = "tool.lifecycle-state-write-denied";

/// Warns when a skill or brief shells out to a host binary after an equivalent
/// declared tool exists.
///
/// The plugin repository currently scans active capability briefs and plugin
/// skills in `scripts/checks.ts`. TODO(rfc-5): port that scanner into the
/// eventual framework-level `specify check` surface without changing this rule
/// id.
pub const SKILL_INVOKES_HOST_BINARY_WITH_DECLARED_TOOL_EQUIVALENT: &str =
    "skill.invokes-host-binary-with-declared-tool-equivalent";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc15_rule_ids_stay_stable() {
        assert_eq!(TOOL_WRITE_PERMISSION_TOO_BROAD, "tool.write-permission-too-broad");
        assert_eq!(TOOL_LIFECYCLE_STATE_WRITE_DENIED, "tool.lifecycle-state-write-denied");
        assert_eq!(
            SKILL_INVOKES_HOST_BINARY_WITH_DECLARED_TOOL_EQUIVALENT,
            "skill.invokes-host-binary-with-declared-tool-equivalent"
        );
    }
}
