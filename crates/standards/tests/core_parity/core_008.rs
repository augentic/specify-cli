//! `CORE-008` ≅ the `content-digest-eq` reserved-kind semantics: every
//! `agent-teams.md` symlink must resolve to content whose SHA-256 equals the
//! canonical review-team-protocol document. No imperative `Check` row is retired.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::{Diagnostic, FindingEvidence};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

const CANONICAL_REL: &str = "docs/reference/review-team-protocol.md";
const CANONICAL_BODY: &str = "# Review Team Protocol\n\nCanonical review-team-protocol body.\n";
const DIVERGENT_REL: &str = "docs/reference/legacy-review-team.md";
const DIVERGENT_BODY: &str =
    "# Legacy Review Team\n\nStale copy that has drifted from canonical.\n";

/// Stage a synthetic framework tree: the canonical document, a divergent
/// document, and three `agent-teams.md` symlinks (two aligned, one drifted).
fn stage_project(project_dir: &Path) {
    let docs_ref = project_dir.join("docs/reference");
    fs::create_dir_all(&docs_ref).expect("docs/reference");
    fs::write(project_dir.join(CANONICAL_REL), CANONICAL_BODY).expect("canonical doc");
    fs::write(project_dir.join(DIVERGENT_REL), DIVERGENT_BODY).expect("divergent doc");

    for (adapter, target_rel) in
        [("aligned-a", CANONICAL_REL), ("aligned-b", CANONICAL_REL), ("drifted", DIVERGENT_REL)]
    {
        let link_dir = project_dir.join("adapters/targets").join(adapter).join("references");
        fs::create_dir_all(&link_dir).expect("link parent");
        let link_path = link_dir.join("agent-teams.md");
        // adapters/targets/<adapter>/references/ is four levels deep.
        let link_target = format!("../../../../{target_rel}");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link_target, &link_path).expect("unix symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&link_target, &link_path).expect("windows symlink");
    }
}

/// Inline reference mirroring `kind: content-digest-eq`; returns the set of
/// symlink paths whose resolved-target digest diverges from canonical.
fn imperative_divergence_set(project_dir: &Path) -> BTreeSet<String> {
    let mut teams: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    collect_agent_teams(project_dir, project_dir, &mut teams);

    let expected = teams
        .iter()
        .find(|(_, resolved, _)| resolved.as_deref() == Some(CANONICAL_REL))
        .and_then(|(_, _, digest)| digest.clone());
    let Some(expected) = expected else {
        return BTreeSet::new();
    };

    teams
        .into_iter()
        .filter(|(_, _, digest)| digest.as_deref() != Some(expected.as_str()))
        .map(|(path, _, _)| path)
        .collect()
}

/// Recursively find `agent-teams.md` symlinks, returning
/// `(project-relative symlink path, resolved-target rel, target sha256)`.
fn collect_agent_teams(
    project_dir: &Path, dir: &Path, out: &mut Vec<(String, Option<String>, Option<String>)>,
) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() {
            if path.file_name().and_then(|n| n.to_str()) != Some("agent-teams.md") {
                continue;
            }
            let rel = render_rel(project_dir, &path);
            let resolved =
                fs::canonicalize(&path).ok().and_then(|c| canonical_project_rel(project_dir, &c));
            let digest = fs::canonicalize(&path)
                .ok()
                .and_then(|c| fs::read(c).ok())
                .map(|bytes| sha256(&bytes));
            out.push((rel, resolved, digest));
        } else if meta.is_dir() {
            collect_agent_teams(project_dir, &path, out);
        }
    }
}

fn canonical_project_rel(project_dir: &Path, resolved: &Path) -> Option<String> {
    let root = fs::canonicalize(project_dir).ok()?;
    let rel = resolved.strip_prefix(&root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

fn render_rel(project_dir: &Path, path: &Path) -> String {
    path.strip_prefix(project_dir)
        .map_or_else(|_| path.display().to_string(), |rel| rel.to_string_lossy().replace('\\', "/"))
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

fn declarative_divergence_set(findings: &[Diagnostic]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for finding in findings {
        let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
        if let Some(team) = data.get("agent-team").and_then(|v| v.as_str()) {
            out.insert(team.to_string());
        }
    }
    out
}

#[test]
fn matches_content_digest_eq() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_divergence_set(project_dir);
    let expected: BTreeSet<String> =
        std::iter::once("adapters/targets/drifted/references/agent-teams.md".to_string()).collect();
    assert_eq!(
        imperative, expected,
        "imperative reference must flag exactly the drifted agent-teams.md symlink",
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule =
        make_rule("CORE-008", vec![hint(HintKind::ContentDigestEq, "agent-teams-match-canonical")]);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        project_dir,
        runner,
        1,
    )
    .expect("declarative evaluate");

    for finding in &outcome.findings {
        assert_eq!(
            finding.rule_id.as_deref(),
            Some("CORE-008"),
            "declarative findings must carry the documented CORE-008 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            loc.path.ends_with("agent-teams.md"),
            "declarative location must point at an agent-teams.md symlink: got {}",
            loc.path,
        );
    }

    let declarative = declarative_divergence_set(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative CORE-008 must flag the same agent-teams.md symlinks as the inline content-digest-eq reference",
    );
}
