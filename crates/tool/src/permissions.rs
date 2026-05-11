//! Permission substitution, canonicalisation, and escape checks for tool runs.

use std::path::{Component, Path, PathBuf};

use crate::error::ToolError;

const PROJECT_DIR_VAR: &str = "PROJECT_DIR";
const CAPABILITY_DIR_VAR: &str = "CAPABILITY_DIR";
const LIFECYCLE_RULE_ID: &str = "tool.lifecycle-state-write-denied";

/// Substitute the permission variables (`$PROJECT_DIR`, `$CAPABILITY_DIR`) in one manifest permission entry.
///
/// # Errors
///
/// Returns an error when the template references an unsupported variable,
/// references `$CAPABILITY_DIR` outside capability scope, contains a parent
/// segment, expands to a relative path, or uses a non-UTF-8 root path.
pub fn substitute(
    template: &str, project_dir: &Path, capability_dir: Option<&Path>,
) -> Result<String, ToolError> {
    if has_parent_segment(template) {
        return Err(ToolError::invalid_permission(
            template,
            "permission paths must not contain `..` segments",
        ));
    }

    for variable in variables(template)? {
        match variable.as_str() {
            PROJECT_DIR_VAR => {}
            CAPABILITY_DIR_VAR if capability_dir.is_some() => {}
            CAPABILITY_DIR_VAR => {
                return Err(ToolError::invalid_permission(
                    template,
                    "$CAPABILITY_DIR is only available to capability-scope tools",
                ));
            }
            _ => {
                return Err(ToolError::invalid_permission(
                    template,
                    format!("unsupported variable `${variable}`"),
                ));
            }
        }
    }

    let project = utf8_path(project_dir, "$PROJECT_DIR", template)?;
    let mut expanded = template.replace("$PROJECT_DIR", project);
    if let Some(capability_dir) = capability_dir {
        let capability = utf8_path(capability_dir, "$CAPABILITY_DIR", template)?;
        expanded = expanded.replace("$CAPABILITY_DIR", capability);
    }

    if has_parent_segment(&expanded) {
        return Err(ToolError::invalid_permission(
            template,
            "expanded permission path must not contain `..` segments",
        ));
    }
    if !path_is_absolute(&expanded) {
        return Err(ToolError::invalid_permission(
            template,
            format!("expanded permission path must be absolute: {expanded}"),
        ));
    }

    Ok(expanded)
}

/// Canonicalise a target path and require it to remain inside an allowed root.
///
/// The target and roots must already exist. Canonicalisation follows symlinks,
/// so a symlink that textually lives under an allowed root but points elsewhere
/// is rejected.
///
/// # Errors
///
/// Returns an error when the target or roots cannot be canonicalised, or when
/// the canonical target is outside every allowed root.
pub fn canonicalise_under(target: &Path, allowed_roots: &[&Path]) -> Result<PathBuf, ToolError> {
    if allowed_roots.is_empty() {
        return Err(ToolError::permission_denied(
            target,
            "no allowed roots were supplied for permission canonicalisation",
        ));
    }

    let canonical_target = target.canonicalize().map_err(|err| {
        ToolError::permission_denied(
            target,
            format!("permission path must already exist and be canonicalisable: {err}"),
        )
    })?;

    for root in allowed_roots {
        let canonical_root = root.canonicalize().map_err(|err| {
            ToolError::permission_denied(
                root,
                format!("allowed root must be canonicalisable: {err}"),
            )
        })?;
        if canonical_target == canonical_root || canonical_target.starts_with(&canonical_root) {
            return Ok(canonical_target);
        }
    }

    Err(ToolError::permission_denied(
        canonical_target,
        "canonical permission path escapes PROJECT_DIR/CAPABILITY_DIR",
    ))
}

/// Reject write access to Specify lifecycle state under the project root.
///
/// # Errors
///
/// Returns [`ToolError::PermissionDenied`] when `target` is `.specify` or a
/// descendant of `.specify` inside `project_dir`.
pub fn deny_lifecycle_write(target: &Path, project_dir: &Path) -> Result<(), ToolError> {
    let canonical_project = project_dir.canonicalize().map_err(|err| {
        ToolError::permission_denied(
            project_dir,
            format!("project root must be canonicalisable: {err}"),
        )
    })?;
    let lifecycle_root = canonical_project.join(".specify");
    if target == lifecycle_root || target.starts_with(&lifecycle_root) {
        return Err(ToolError::permission_denied(target, LIFECYCLE_RULE_ID));
    }
    Ok(())
}

fn variables(template: &str) -> Result<Vec<String>, ToolError> {
    let mut variables = Vec::new();
    let mut chars = template.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch != '$' {
            continue;
        }
        let mut name = String::new();
        while let Some((_, next)) = chars.peek().copied() {
            if next == '_' || next.is_ascii_alphanumeric() {
                name.push(next);
                let _ = chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            return Err(ToolError::invalid_permission(
                template,
                "permission variables must be named, for example $PROJECT_DIR",
            ));
        }
        variables.push(name);
    }
    Ok(variables)
}

fn utf8_path<'a>(path: &'a Path, variable: &str, template: &str) -> Result<&'a str, ToolError> {
    path.to_str().ok_or_else(|| {
        ToolError::invalid_permission(
            template,
            format!("{variable} contains non-UTF-8 bytes and cannot be exposed to WASI"),
        )
    })
}

fn has_parent_segment(value: &str) -> bool {
    value.split(['/', '\\']).any(|segment| segment == "..")
        || Path::new(value).components().any(|component| matches!(component, Component::ParentDir))
}

fn path_is_absolute(value: &str) -> bool {
    Path::new(value).is_absolute() || looks_like_windows_absolute_path(value)
}

fn looks_like_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn substitute_expands_project_and_capability_dirs() {
        let project = Path::new("/tmp/project");
        let capability = Path::new("/tmp/capability");

        assert_eq!(
            substitute("$PROJECT_DIR/contracts", project, Some(capability)).expect("project"),
            "/tmp/project/contracts"
        );
        assert_eq!(
            substitute("$CAPABILITY_DIR/templates", project, Some(capability)).expect("capability"),
            "/tmp/capability/templates"
        );
    }

    #[test]
    fn substitute_rejects_capability_dir_outside_capability_scope() {
        let err = substitute("$CAPABILITY_DIR/templates", Path::new("/tmp/project"), None)
            .expect_err("project-scope capability dir must fail");
        assert!(matches!(err, ToolError::InvalidPermission { .. }), "{err}");
        assert!(err.to_string().contains("$CAPABILITY_DIR"), "{err}");
    }

    #[test]
    fn substitute_rejects_unknown_variables_parent_segments_and_relative_paths() {
        for template in ["$HOME/contracts", "$PROJECT_DIR/../contracts", "contracts"] {
            let err = substitute(template, Path::new("/tmp/project"), None)
                .expect_err("invalid template must fail");
            assert!(matches!(err, ToolError::InvalidPermission { .. }), "{err}");
        }
    }

    #[test]
    fn canonicalise_under_rejects_symlink_escape() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&outside).expect("outside");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, project.join("escape")).expect("symlink");
            let err = canonicalise_under(&project.join("escape"), &[&project])
                .expect_err("symlink escape must fail");
            assert!(matches!(err, ToolError::PermissionDenied { .. }), "{err}");
        }
    }

    #[test]
    fn canonicalise_under_accepts_existing_descendant() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let contracts = project.join("contracts");
        fs::create_dir_all(&contracts).expect("contracts");

        let canonical = canonicalise_under(&contracts, &[&project]).expect("canonical");
        assert_eq!(canonical, contracts.canonicalize().expect("canonical contracts"));
    }

    #[test]
    fn lifecycle_write_denial_rejects_specify_state() {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let specify = project.join(".specify");
        fs::create_dir_all(&specify).expect("specify");

        let err = deny_lifecycle_write(&specify.canonicalize().expect("canonical"), &project)
            .expect_err("lifecycle write must fail");
        assert!(matches!(err, ToolError::PermissionDenied { .. }), "{err}");
        assert!(err.to_string().contains(LIFECYCLE_RULE_ID), "{err}");
    }
}
