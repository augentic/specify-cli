//! Default-path resolver and cross-artifact discovery helper.
//!
//! Two related questions answered in one place:
//!
//! * "What file should `validate <mode>` read when no `[path]`
//!   positional is supplied?" → [`resolve_default_path`].
//! * "What sibling artifact should cross-artifact resolution chase
//!   from this calling artifact's location?" → [`discover_artifact`].
//!
//! Both walk up from a starting path looking for `.specify/`, parse
//! the project's `schema.yaml` (if any) for an on-disk `artifacts:`
//! block, and otherwise use the embedded defaults at
//! [`EMBEDDED_ARTIFACT_PATHS`].

use std::path::{Path, PathBuf};

use crate::ValidateMode;

/// Embedded default paths for the four `vectis-validate` modes. The
/// canonical Vectis cascade: slice-local files first, then
/// project-level inputs or the merged composition baseline.
///
/// The order of the inner array is the resolution order; the first
/// existing file wins. The role label (first tuple element) is
/// retained for parity with the schema YAML even though only the
/// template strings are consumed. The `<name>` placeholder is
/// expanded against `.specify/slices/<dir>/` (alphabetical first
/// match) at resolution time.
const EMBEDDED_ARTIFACT_PATHS: &[(&str, &[(&str, &str)])] = &[
    (
        "layout",
        &[
            ("change_local", ".specify/slices/<name>/layout.yaml"),
            ("project", "design-system/layout.yaml"),
        ],
    ),
    (
        "tokens",
        &[
            ("change_local", ".specify/slices/<name>/tokens.yaml"),
            ("project", "design-system/tokens.yaml"),
        ],
    ),
    (
        "assets",
        &[
            ("change_local", ".specify/slices/<name>/assets.yaml"),
            ("project", "design-system/assets.yaml"),
        ],
    ),
    (
        "composition",
        &[
            ("change_local", ".specify/slices/<name>/composition.yaml"),
            ("baseline", ".specify/specs/composition.yaml"),
        ],
    ),
];

/// Resolve a per-mode default path for `validate <mode>` when no
/// `[path]` positional was supplied.
///
/// Walks up from CWD looking for a project root (the directory
/// containing `.specify/`); falls through to a fixed canonical path
/// when the resolver yields nothing, so the caller's
/// `<file>.yaml not readable at <path>` error names the most
/// operator-friendly path.
pub(super) fn resolve_default_path(mode: ValidateMode) -> PathBuf {
    resolve_default_path_with_root(mode, &default_project_root())
}

/// Return the default project root for omitted `[path]` positionals.
///
/// WASI tool invocations receive `PROJECT_DIR` from the host. Native
/// development walks up from CWD to a `.specify/` root when present,
/// otherwise uses CWD.
pub(super) fn default_project_root() -> PathBuf {
    if let Some(project_dir) = std::env::var_os("PROJECT_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(project_dir);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_project_root(&cwd).unwrap_or(cwd)
}

/// Resolve a per-mode default path against an explicit project root.
///
/// Used both by [`resolve_default_path`] (where the root is derived
/// from CWD) and by `validate all` (where the root is the operator's
/// `[path]` positional, defaulting to CWD). When no candidate exists
/// the function returns the *last* candidate it considered; if the
/// candidate list itself is empty (an unknown mode key in a
/// hand-edited `artifacts:` block), it falls back to the embedded
/// canonical name under `<root>/`.
#[must_use]
pub fn resolve_default_path_with_root(mode: ValidateMode, project_root: &Path) -> PathBuf {
    let key = artifact_key_for_mode(mode).unwrap_or("composition");
    let templates = paths_for_key(key);

    let mut last_candidate: Option<PathBuf> = None;
    for template in &templates {
        for resolved in expand_path_template(template, project_root) {
            if resolved.is_file() {
                return resolved;
            }
            last_candidate = Some(resolved);
        }
    }
    last_candidate.unwrap_or_else(|| project_root.join(canonical_default_template(key)))
}

/// Locate a sibling artifact (in the [`ValidateMode`] sense) for a
/// caller anchored at `start`. Returns `Some(path)` only when an
/// existing file is found; `None` otherwise.
///
/// Resolution order:
///
/// 1. **Same directory as `start`** — catches the change-local case
///    where every artifact sits next to its caller, plus standalone
///    "files in the same folder" usage that does not rely on a
///    Specify project layout.
/// 2. **Embedded canonical cascade against the project root** —
///    walks up from `start` to find `.specify/`, then tries every
///    `paths.<role>` template in canonical order.
#[must_use]
pub fn discover_artifact(start: &Path, mode: ValidateMode) -> Option<PathBuf> {
    let key = artifact_key_for_mode(mode)?;

    let filename = canonical_filename_for_key(key);
    if let Some(parent) = start.parent() {
        let local = parent.join(filename);
        if local.is_file() {
            return Some(local);
        }
    }

    let project_root = find_project_root(start)?;
    let templates = paths_for_key(key);

    for template in &templates {
        for resolved in expand_path_template(template, &project_root) {
            if resolved.is_file() {
                return Some(resolved);
            }
        }
    }
    None
}

/// Filename half of the canonical-default template for a given
/// artifact key. Stays in lock-step with [`canonical_default_template`].
fn canonical_filename_for_key(key: &str) -> &'static str {
    match key {
        "layout" => "layout.yaml",
        "tokens" => "tokens.yaml",
        "assets" => "assets.yaml",
        _ => "composition.yaml",
    }
}

/// Walk up from `start` until a `.specify/` directory is found.
///
/// `start` is treated as a directory if it is one, otherwise its
/// parent is used. Returns the project root — the directory
/// containing `.specify/`, *not* `.specify/` itself.
#[must_use]
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor =
        if start.is_dir() { start.to_path_buf() } else { start.parent()?.to_path_buf() };
    loop {
        if cursor.join(".specify").is_dir() {
            return Some(cursor);
        }
        if !cursor.pop() {
            return None;
        }
    }
}

/// Map a [`ValidateMode`] to the `artifacts:` map key it resolves
/// against. `ValidateMode::All` has no per-mode key (the convenience
/// verb dispatches each per-mode handler in turn) and returns `None`.
const fn artifact_key_for_mode(mode: ValidateMode) -> Option<&'static str> {
    match mode {
        ValidateMode::Layout => Some("layout"),
        ValidateMode::Composition => Some("composition"),
        ValidateMode::Tokens => Some("tokens"),
        ValidateMode::Assets => Some("assets"),
        ValidateMode::All => None,
    }
}

/// Return the ordered list of `paths.<role>` templates for the given
/// artifact `key`. The resolution order comes from the embedded
/// canonical mapping in [`EMBEDDED_ARTIFACT_PATHS`].
#[must_use]
pub fn paths_for_key(key: &str) -> Vec<String> {
    EMBEDDED_ARTIFACT_PATHS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, paths)| paths.iter().map(|(_, t)| (*t).to_string()).collect())
        .unwrap_or_default()
}

/// Return the operator-friendly fallback template (the project /
/// baseline location, *not* the change-local one) for a given
/// artifact key. Used as the very last resort when neither the
/// on-disk `artifacts:` block nor the embedded defaults yield any
/// candidate.
fn canonical_default_template(key: &str) -> &'static str {
    match key {
        "layout" => "design-system/layout.yaml",
        "tokens" => "design-system/tokens.yaml",
        "assets" => "design-system/assets.yaml",
        _ => ".specify/specs/composition.yaml",
    }
}

/// Expand a `paths.<role>` template against `project_root`.
///
/// `<name>` is substituted with each directory under
/// `.specify/slices/` (sorted alphabetically). Templates without
/// `<name>` resolve to a single absolute path.
pub fn expand_path_template(template: &str, project_root: &Path) -> Vec<PathBuf> {
    if !template.contains("<name>") {
        return vec![project_root.join(template)];
    }
    let slices_dir = project_root.join(".specify/slices");
    let Ok(entries) = std::fs::read_dir(&slices_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names.into_iter().map(|name| project_root.join(template.replace("<name>", &name))).collect()
}
