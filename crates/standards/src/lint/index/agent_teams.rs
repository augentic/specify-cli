//! `**/agent-teams.md` extractor per the standards-layer contract
//! §"Module additions" and §F1's framework symlink policy.
//!
//! Adapter review briefs symlink `agent-teams.md` into the canonical
//! `docs/reference/review-team-protocol.md` document. The framework
//! scan profile follows those links to record the endpoint pair plus
//! a SHA-256 of the resolved target's content so review-team drift
//! across documentation boundaries is detectable. Broken links and
//! off-tree targets still produce an [`AgentTeam`] fact carrying the
//! raw `read_link` target so downstream rules can flag the
//! divergence rather than silently dropping the entry.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::lint::AgentTeam;

const AGENT_TEAMS_FILE: &str = "agent-teams.md";

/// Build an [`AgentTeam`] fact for an on-disk `agent-teams.md` symlink.
///
/// `link_path` is the absolute filesystem path of the symlink
/// itself; `project_dir` anchors the project-relative rendering.
/// Returns `None` when the entry is not a symlink, when `read_link`
/// fails, or when either path falls outside the project tree.
#[must_use]
pub fn record(link_path: &Path, project_dir: &Path) -> Option<AgentTeam> {
    let file_name = link_path.file_name()?.to_str()?;
    if file_name != AGENT_TEAMS_FILE {
        return None;
    }
    let relative = link_path.strip_prefix(project_dir).ok()?;
    let path = super::path_util::render(relative)?;
    let target = std::fs::read_link(link_path).ok()?;
    let target_raw = super::path_util::render(&target)?;
    let (resolved_target, target_sha256) = resolve(link_path, project_dir);
    Some(AgentTeam {
        path,
        target_raw,
        resolved_target,
        target_sha256,
    })
}

/// Canonicalise the link and read the resolved target's bytes,
/// returning the project-relative endpoint (when on-tree) and the
/// content digest (when readable). Broken links and unreadable
/// targets collapse to `(None, None)` — the raw target on the
/// returned fact carries the diagnostic signal in that case.
fn resolve(link_path: &Path, project_dir: &Path) -> (Option<String>, Option<String>) {
    let Ok(canon_link) = std::fs::canonicalize(link_path) else {
        return (None, None);
    };
    let resolved_target = std::fs::canonicalize(project_dir)
        .ok()
        .and_then(|root| canon_link.strip_prefix(&root).ok().map(Path::to_path_buf))
        .and_then(|relative| super::path_util::render(&relative));
    let target_sha256 = std::fs::read(&canon_link).ok().map(|bytes| {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = hasher.finalize();
        let mut out = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut out, "{byte:02x}").expect("write to string");
        }
        out
    });
    (resolved_target, target_sha256)
}

