//! C16 parity test: prove `CORE-008` covers the `content-digest-eq`
//! reserved-kind semantics for framework `agent-teams.md` symlink
//! overlays — every symlink must resolve to content whose SHA-256
//! equals the canonical `docs/reference/review-team-protocol.md`
//! review-team-protocol document.
//!
//! # Equivalence mapping
//!
//! Declarative rule id `CORE-008` ≅ `agent-teams.content-digest`
//! (notional). **No imperative `Check` row is retired by this card.**
//! The C16 plan card named `crates/lints/src/framework/check/agent_teams.rs`
//! (the `agent-teams.non-canonical-overlay` /
//! `agent-teams.missing-canonical` predicate) as a strong candidate,
//! but `content-digest-eq` does not subsume it cleanly, so the
//! predicate stays. The imperative symlink branch enforces *path*
//! equality — the symlink must `canonicalize` to the canonical
//! document's path — whereas `content-digest-eq` enforces
//! *content-digest* equality: a symlink pointing at a byte-identical
//! copy of the canonical document at a different path passes the
//! digest check but fails the imperative path check. The imperative
//! check also owns three branches with no `AgentTeam` fact behind
//! them — the regular-file digest branch (`agent-teams.md` committed
//! as a file, not a symlink), the missing-canonical branch, and the
//! unsupported-entry branch. The C5 framework extractor
//! (`crates/lints/src/lint/index/agent_teams.rs::record`)
//! emits an `AgentTeam` fact only for symlinks, so a fact-iterating
//! evaluator is structurally blind to those cases. Per the §F5
//! migration cadence the kind interpreter plus seed rule land against
//! a synthetic fixture when no `Check` row maps cleanly (the
//! imperative deletion is gated on byte-identical parity, not required
//! for the kind landing) — exactly the C14 / C15 fallback. Because no
//! imperative deletion is in flight, the fingerprint-based
//! deduplication in the migration cadence has nothing to merge.
//!
//! Imperative behaviour (anchored as executable code in this test
//! crate): for every `**/agent-teams.md` symlink, read the resolved
//! target's bytes, compute its SHA-256, take the expected digest to be
//! the digest carried by symlinks resolving to the canonical relative
//! path `docs/reference/review-team-protocol.md`, and return the set
//! of symlink paths whose target digest diverges from the expected
//! canonical digest (a divergent digest, or an unreadable / broken
//! target with no digest at all).
//!
//! Declarative behaviour: the framework-profile indexer extracts one
//! [`specify_lints::lint::AgentTeam`] fact per followed
//! `agent-teams.md` symlink
//! (`crates/lints/src/lint/index/agent_teams.rs::record`,
//! whose `target-sha256` field is the SHA-256 of the resolved target's
//! bytes); the `kind: content-digest-eq` interpreter
//! (`crates/lints/src/lint/eval/content_digest_eq.rs::evaluate`)
//! consumes the fact set, derives the same canonical digest, and emits
//! one [`Diagnostic`] per divergent symlink carrying the
//! `(agent-team, resolved-target, expected-digest, actual-digest)`
//! shape as structured evidence.
//!
//! # Option
//!
//! Option A (functional parity) against a synthetic fixture. The test
//! stages a canonical review-team-protocol document, a second
//! divergent document, and three `agent-teams.md` symlinks:
//!
//! - `aligned-a` — symlinks to the canonical document; matches.
//! - `aligned-b` — symlinks to the canonical document; matches.
//! - `drifted` — symlinks to the divergent document; its content
//!   digest differs from the canonical digest, so it is flagged.
//!
//! Then runs:
//!
//! 1. An inline reference implementation that mirrors the
//!    `kind: content-digest-eq` semantics (walk every
//!    `**/agent-teams.md` symlink, hash the resolved target, derive
//!    the canonical digest from symlinks resolving to
//!    `docs/reference/review-team-protocol.md`, and return the set of
//!    diverging symlink paths). This stands in for the "imperative
//!    row" anchored as executable code in this test crate so the
//!    parity claim is not purely tautological.
//! 2. The declarative pipeline: `lint::index::build` under the
//!    framework scan profile (which populates `model.agent_teams`),
//!    then `lint::eval::evaluate` against a synthesised `CORE-008`
//!    rule carrying the single `content-digest-eq:
//!    agent-teams-match-canonical` hint CORE-008 ships on disk.
//!
//! Both passes MUST agree on the set of diverging `agent-teams.md`
//! symlink paths. Per-finding locations are NOT compared
//! byte-identically because the imperative reference returns abstract
//! paths while the declarative evaluator stamps a project-relative
//! location; functional parity (which symlinks diverged from the
//! canonical content digest) is the contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::{Diagnostic, FindingEvidence, Severity};
use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{DeterministicHint, HintKind, Origin, PathRoot, ResolvedRule};

const CANONICAL_REL: &str = "docs/reference/review-team-protocol.md";
const CANONICAL_BODY: &str = "# Review Team Protocol\n\nCanonical review-team-protocol body.\n";
const DIVERGENT_REL: &str = "docs/reference/legacy-review-team.md";
const DIVERGENT_BODY: &str =
    "# Legacy Review Team\n\nStale copy that has drifted from canonical.\n";

/// Stage a synthetic framework tree: the canonical document, a
/// divergent document, and three `agent-teams.md` symlinks (two
/// aligned with canonical, one drifted to the divergent doc).
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
        // Relative target back up to the project root, then down to the
        // doc: adapters/targets/<adapter>/references/ is four levels deep.
        let link_target = format!("../../../../{target_rel}");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link_target, &link_path).expect("unix symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&link_target, &link_path).expect("windows symlink");
    }
}

/// Inline reference implementation mirroring the
/// `kind: content-digest-eq` semantics so the parity claim is anchored
/// to executable code in this commit. Walks every `**/agent-teams.md`
/// symlink, hashes the resolved target, derives the canonical digest
/// from symlinks resolving to `docs/reference/review-team-protocol.md`,
/// and returns the set of symlink paths whose target digest diverges.
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

fn make_rule(rule_id: &str, hints: Vec<DeterministicHint>) -> ResolvedRule {
    ResolvedRule {
        rule_id: rule_id.to_string(),
        title: format!("{rule_id} parity fixture"),
        severity: Severity::Important,
        trigger: format!("Trigger for {rule_id}"),
        lint_mode: None,
        applicability: None,
        deterministic_hints: if hints.is_empty() { None } else { Some(hints) },
        references: None,
        origin: Origin::Core,
        path_root: PathRoot::RulesRoot,
        path: format!("adapters/shared/rules/core/{rule_id}.md"),
        body: String::new(),
        deprecated: None,
    }
}

fn hint(kind: HintKind, value: &str) -> DeterministicHint {
    DeterministicHint {
        kind,
        value: value.to_string(),
        description: None,
    }
}

struct NoToolRunner;

impl ToolRunner for NoToolRunner {
    fn run(
        &self, _tool_name: &str, _args: &[String], _project_dir: &Path,
    ) -> Result<ToolOutput, ToolRunError> {
        Err(ToolRunError::Runtime("no tool runner wired".to_string()))
    }

    fn is_declared(&self, _tool_name: &str) -> bool {
        false
    }
}

#[test]
fn core_008_matches_content_digest_eq_reference_against_agent_teams() {
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
        rule.deterministic_hints.as_deref().unwrap_or_default(),
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
