//! Pure per-target `agent-teams.md` overlay checks for the
//! `agent-teams` framework-authoring tool, lifted from the host CLI's
//! retiring `framework::check::agent_teams` imperative predicate
//! (Road B framework tool).
//!
//! The tool covers CORE-012 (`agent-teams.non-canonical-overlay`).
//! CORE-012 is deliberately stricter than the already-native CORE-008
//! (`content-digest-eq`): it preserves the symlink path-equality,
//! regular-file content-drift, and unsupported-entry-type branches that
//! the symlink-only `AgentTeam` fact behind CORE-008 cannot express. All
//! branches are lifted verbatim from `check_overlays`. (CORE-011 —
//! missing canonical document — moved to the native Road A
//! `kind: presence` `file` selector; when the canonical document is
//! absent this tool emits nothing, leaving the absence to that rule.)
//!
//! Policy is `specify`-owned, never baked here: the canonical document
//! path arrives as a parameter the entrypoint reads from the rule's
//! `config:` (forwarded by the `kind: tool` evaluator). The only literals
//! in this crate are mechanism — the `adapters/targets/*/references/`
//! overlay layout the checks structurally scan.
//!
//! Carve-out posture: this crate owns its logic and depends only on
//! `serde` / `serde_json` / `sha2`, never the host diagnostics crate
//! (`main.rs` renders the wire envelope).

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Codex id this check stamps onto its findings (closed `CORE-NNN`).
pub const RULE_NON_CANONICAL: &str = "CORE-012";

/// One agent-teams overlay violation: its codex `rule_id`, the offending
/// file's project-relative path (when one applies), and a human-readable
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTeamsFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending file.
    pub path: Option<String>,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// Walk every per-target `agent-teams.md` overlay under
/// `<project_dir>/adapters/targets/*/references/` and return the
/// CORE-012 (overlay drift) findings. The `canonical_rel` path — the
/// rule's policy — names the canonical document every overlay must
/// mirror.
///
/// When the canonical document itself is missing the scan returns no
/// findings: the native `kind: presence` `file` rule (CORE-011) owns
/// that absence, and the overlays cannot be validated against a missing
/// baseline.
#[must_use]
pub fn run(project_dir: &Path, canonical_rel: &str) -> Vec<AgentTeamsFinding> {
    let canonical_path = project_dir.join(canonical_rel);
    let Ok(canonical_bytes) = fs::read(&canonical_path) else {
        return Vec::new();
    };
    let canonical_hash = sha256(&canonical_bytes);
    let Ok(canonical_canon) = fs::canonicalize(&canonical_path) else {
        return Vec::new();
    };

    let targets_dir = project_dir.join("adapters").join("targets");
    let Ok(targets) = fs::read_dir(&targets_dir) else {
        return Vec::new();
    };

    let mut entries: Vec<PathBuf> = targets
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|ft| ft.is_dir()))
        .map(|entry| entry.path())
        .collect();
    entries.sort();

    let mut findings = Vec::new();
    for target in entries {
        match check_overlay(project_dir, &canonical_canon, canonical_rel, &canonical_hash, &target) {
            Overlay::Clean => {}
            Overlay::Drifted(finding) => findings.push(finding),
        }
    }

    findings
}

/// Outcome of checking one target adapter's overlay.
enum Overlay {
    Clean,
    Drifted(AgentTeamsFinding),
}

fn check_overlay(
    project_dir: &Path, canonical_canon: &Path, canonical_rel: &str, canonical_hash: &str,
    target: &Path,
) -> Overlay {
    let ref_path = target.join("references").join("agent-teams.md");
    let ref_rel = path_relative(project_dir, &ref_path);

    let Ok(metadata) = fs::symlink_metadata(&ref_path) else {
        return Overlay::Clean;
    };

    if metadata.file_type().is_symlink() {
        return check_symlink(project_dir, canonical_canon, canonical_rel, &ref_path, &ref_rel);
    }

    if metadata.is_file() {
        return match fs::read(&ref_path) {
            Ok(local_bytes) if sha256(&local_bytes) != canonical_hash => {
                Overlay::Drifted(non_canonical(&ref_rel, format!(
                    "Agent-teams overlay: {ref_rel} — content drifted from canonical '{canonical_rel}' (replace with a symlink or re-sync the file)"
                )))
            }
            Ok(_) => Overlay::Clean,
            Err(source) => Overlay::Drifted(non_canonical(
                &ref_rel,
                format!("Agent-teams overlay: {ref_rel} — cannot read file: {source}"),
            )),
        };
    }

    Overlay::Drifted(non_canonical(&ref_rel, format!(
        "Agent-teams overlay: {ref_rel} — must be a regular file or symlink, found unsupported entry type"
    )))
}

fn check_symlink(
    project_dir: &Path, canonical_canon: &Path, canonical_rel: &str, ref_path: &Path, ref_rel: &str,
) -> Overlay {
    let Ok(resolved) = fs::canonicalize(ref_path) else {
        return Overlay::Drifted(non_canonical(
            ref_rel,
            format!("Agent-teams overlay: {ref_rel} — symlink does not resolve"),
        ));
    };
    if resolved == *canonical_canon {
        Overlay::Clean
    } else {
        Overlay::Drifted(non_canonical(ref_rel, format!(
            "Agent-teams overlay: {ref_rel} — symlink resolves to '{}', expected '{canonical_rel}'",
            path_relative(project_dir, &resolved)
        )))
    }
}

fn non_canonical(ref_rel: &str, message: String) -> AgentTeamsFinding {
    AgentTeamsFinding {
        rule_id: RULE_NON_CANONICAL,
        path: Some(ref_rel.to_string()),
        message,
    }
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn path_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANONICAL_REL: &str = "docs/reference/review-team-protocol.md";

    fn write_canonical(root: &Path, body: &str) {
        let path = root.join(CANONICAL_REL);
        fs::create_dir_all(path.parent().expect("canonical parent")).expect("canonical dir");
        fs::write(path, body).expect("write canonical");
    }

    fn target_ref(root: &Path, target: &str) -> PathBuf {
        let dir = root.join("adapters/targets").join(target).join("references");
        fs::create_dir_all(&dir).expect("references dir");
        dir.join("agent-teams.md")
    }

    #[test]
    fn missing_canonical_is_silent() {
        let dir = tempfile::tempdir().expect("tempdir");
        target_ref(dir.path(), "omnia");
        // The missing canonical is owned by the native CORE-011 presence
        // rule now; this tool emits nothing without a baseline to diff.
        assert!(run(dir.path(), CANONICAL_REL).is_empty());
    }

    #[test]
    fn symlink_to_canonical_is_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_canonical(dir.path(), "# Protocol\n");
        let ref_path = target_ref(dir.path(), "omnia");
        std::os::unix::fs::symlink(dir.path().join(CANONICAL_REL), &ref_path).expect("symlink");
        assert!(run(dir.path(), CANONICAL_REL).is_empty());
    }

    #[test]
    fn drifted_regular_file_flags_non_canonical() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_canonical(dir.path(), "# Protocol\n");
        let ref_path = target_ref(dir.path(), "omnia");
        fs::write(&ref_path, "# Drifted copy\n").expect("write overlay");
        let findings = run(dir.path(), CANONICAL_REL);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_NON_CANONICAL);
        assert!(findings[0].message.contains("content drifted"));
    }

    #[test]
    fn symlink_to_wrong_target_flags_non_canonical() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_canonical(dir.path(), "# Protocol\n");
        let other = dir.path().join("docs/reference/other.md");
        fs::write(&other, "# Other\n").expect("write other");
        let ref_path = target_ref(dir.path(), "omnia");
        std::os::unix::fs::symlink(&other, &ref_path).expect("symlink");
        let findings = run(dir.path(), CANONICAL_REL);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_NON_CANONICAL);
        assert!(findings[0].message.contains("symlink resolves to"));
    }
}
