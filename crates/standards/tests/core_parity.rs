//! Parity tests: each `CORE-001..009` declarative rule must flag the same
//! cases as its retiring (or notional) imperative predicate.
//!
//! Each `core_00N` submodule stages a synthetic fixture, runs an inline
//! reference implementation of the imperative predicate semantics, runs the
//! declarative pipeline (`lint::index::build` + `lint::eval::evaluate`)
//! against a synthesised rule carrying the hints the `CORE-00N` rule ships on
//! disk, and asserts both passes agree on the flagged set. Per-finding
//! locations are not compared byte-identically — functional parity (which
//! cases were flagged) is the contract. See the per-module comments for the
//! equivalence mapping rationale.
//!
//! Consolidated from the former nine `core_parity_*.rs` integration binaries;
//! the shared `make_rule` / `hint` / `NoToolRunner` scaffolding lives here once
//! and each submodule keeps its own fixture and reference implementation.

use std::path::Path;

use specify_diagnostics::Severity;
use specify_standards::lint::eval::{ToolOutput, ToolRunError, ToolRunner};
use specify_standards::rules::{DeterministicHint, HintKind, Origin, PathRoot, ResolvedRule};

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

/// `CORE-001` ≅ the retiring `adapter.schema-violation` imperative row.
/// Both walk the same `iter_errors` set over the same `serde_saphyr`-parsed
/// `adapter.yaml`, so the `instance_path` pointer set must be byte-identical.
mod core_001 {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::Value as JsonValue;
    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const BAD_MANIFEST: &str = "name: bad-source\nversion: 1\naxis: source\n";
    const GOOD_MANIFEST: &str = concat!(
        "name: good-source\n",
        "version: 1\n",
        "axis: source\n",
        "description: Valid fixture.\n",
        "briefs:\n",
        "  survey: briefs/survey.md\n",
        "  extract: briefs/extract.md\n",
    );

    fn cli_schemas_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("schemas")
    }

    fn stage_project(project_dir: &Path) {
        fs::create_dir_all(project_dir.join("plugins")).expect("plugins");
        fs::create_dir_all(project_dir.join("adapters/sources")).expect("sources");
        fs::create_dir_all(project_dir.join("adapters/targets")).expect("targets");

        let cursor_schemas = project_dir.join(".cursor/schemas");
        fs::create_dir_all(&cursor_schemas).expect("cursor schemas");
        let schema_src = cli_schemas_dir().join("adapter.schema.json");
        fs::copy(&schema_src, cursor_schemas.join("adapter.schema.json")).expect("copy schema");

        let bad_dir = project_dir.join("adapters/sources/bad-source");
        fs::create_dir_all(&bad_dir).expect("bad dir");
        fs::write(bad_dir.join("adapter.yaml"), BAD_MANIFEST).expect("write bad");

        let good_dir = project_dir.join("adapters/sources/good-source");
        fs::create_dir_all(&good_dir).expect("good dir");
        fs::write(good_dir.join("adapter.yaml"), GOOD_MANIFEST).expect("write good");
    }

    /// Reproduces the deleted imperative
    /// `check::adapter::{load_runtime_validator, validate_manifest}` body
    /// inline so the parity claim is anchored to executable code in this
    /// commit.
    fn imperative_pointer_set(project_dir: &Path, manifest_rel: &str) -> BTreeSet<String> {
        let schema_path = cli_schemas_dir().join("adapter.schema.json");
        let schema_body = fs::read_to_string(&schema_path).expect("read schema");
        let schema_json: JsonValue = serde_json::from_str(&schema_body).expect("schema json");
        let validator = jsonschema::validator_for(&schema_json).expect("schema compiles");

        let manifest_body =
            fs::read_to_string(project_dir.join(manifest_rel)).expect("read manifest");
        let instance: JsonValue =
            serde_saphyr::from_str(&manifest_body).expect("manifest yaml parse");
        validator.iter_errors(&instance).map(|err| err.instance_path().to_string()).collect()
    }

    fn declarative_pointer_set(findings: &[Diagnostic], manifest_rel: &str) -> BTreeSet<String> {
        findings
            .iter()
            .filter(|f| f.location.as_ref().is_some_and(|loc| loc.path == manifest_rel))
            .filter_map(|f| match &f.evidence {
                FindingEvidence::Structured { data, .. } => {
                    data.get("json_pointer").and_then(JsonValue::as_str).map(str::to_owned)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn matches_imperative_schema_row() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let bad_rel = "adapters/sources/bad-source/adapter.yaml";
        let good_rel = "adapters/sources/good-source/adapter.yaml";

        let imperative_bad = imperative_pointer_set(project_dir, bad_rel);
        let imperative_good = imperative_pointer_set(project_dir, good_rel);
        assert!(
            !imperative_bad.is_empty(),
            "imperative row must flag the bad manifest (parity fixture invariant)"
        );
        assert!(
            imperative_good.is_empty(),
            "imperative row must not flag the good manifest: {imperative_good:?}"
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-001",
            vec![
                hint(HintKind::PathPattern, "adapters/**/adapter.yaml"),
                hint(HintKind::Schema, "./.cursor/schemas/adapter.schema.json"),
            ],
        );
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
                Some("CORE-001"),
                "declarative findings must carry the documented CORE-001 rule id",
            );
        }

        let declarative_bad = declarative_pointer_set(&outcome.findings, bad_rel);
        let declarative_good = declarative_pointer_set(&outcome.findings, good_rel);

        assert_eq!(
            declarative_bad, imperative_bad,
            "declarative CORE-001 must cite the same instance pointers on the bad manifest as the retired adapter.schema-violation predicate",
        );
        assert!(
            declarative_good.is_empty(),
            "declarative CORE-001 must not flag the good manifest: {declarative_good:?}"
        );
    }
}

/// `CORE-002` ≅ the retiring `links.unresolved` imperative row. Both share the
/// `[label](target)` grammar, fence-skipping, and the parent-relative
/// path-resolution rule; the `(file, broken_target)` set must agree.
mod core_002 {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use regex::Regex;
    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const VALID_BODY: &str =
        "# Valid page\n\nSee [target](./target.md) and [external](https://example.com).\n";
    const TARGET_BODY: &str = "# Target\n";
    const BROKEN_BODY: &str =
        "# Broken page\n\nSee [missing](./missing.md) and [also-missing](../other/absent.md).\n";

    fn stage_project(project_dir: &Path) {
        let docs = project_dir.join("docs");
        fs::create_dir_all(&docs).expect("docs dir");
        fs::write(docs.join("target.md"), TARGET_BODY).expect("write target");
        fs::write(docs.join("valid.md"), VALID_BODY).expect("write valid");
        fs::write(docs.join("broken.md"), BROKEN_BODY).expect("write broken");
    }

    /// Reproduces the deleted imperative `check::links::check_markdown_links`
    /// body inline; returns the `(from_relative, target)` pairs that would
    /// have been flagged.
    fn imperative_broken_set(project_dir: &Path) -> BTreeSet<(String, String)> {
        let link_re = link_pattern();
        let fence_re = fenced_code_pattern();
        let inline_re = inline_code_pattern();
        let comment_re = html_comment_pattern();

        let mut out: BTreeSet<(String, String)> = BTreeSet::new();
        let mut stack: Vec<PathBuf> = vec![project_dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&path) else { continue };
                let parent = path.parent().unwrap_or(project_dir);
                let stripped = {
                    let no_fence = fence_re.replace_all(&content, "");
                    let no_comments = comment_re.replace_all(&no_fence, "");
                    inline_re.replace_all(&no_comments, "").into_owned()
                };
                for cap in link_re.captures_iter(&stripped) {
                    let target = cap.get(1).map_or("", |m| m.as_str());
                    if target.starts_with("http://")
                        || target.starts_with("https://")
                        || target.starts_with("mailto:")
                        || target.starts_with('#')
                    {
                        continue;
                    }
                    let path_part = target.split('#').next().unwrap_or("");
                    if path_part.is_empty() || path_part.starts_with("src/") {
                        continue;
                    }
                    if !parent.join(path_part).exists() {
                        let rel = path.strip_prefix(project_dir).map_or_else(
                            |_| path.display().to_string(),
                            |p| p.to_string_lossy().replace('\\', "/"),
                        );
                        out.insert((rel, target.to_string()));
                    }
                }
            }
        }
        out
    }

    fn declarative_broken_set(findings: &[Diagnostic]) -> BTreeSet<(String, String)> {
        findings
            .iter()
            .filter_map(|f| {
                let loc = f.location.as_ref()?;
                let target = match &f.evidence {
                    FindingEvidence::Snippet { value } => value.clone(),
                    _ => return None,
                };
                Some((loc.path.clone(), target))
            })
            .collect()
    }

    fn link_pattern() -> &'static Regex {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"\[[^\]]*\]\(([^)]+)\)").expect("valid link pattern"))
    }

    fn fenced_code_pattern() -> &'static Regex {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"```[\s\S]*?```").expect("valid fence pattern"))
    }

    fn inline_code_pattern() -> &'static Regex {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid inline code pattern"))
    }

    fn html_comment_pattern() -> &'static Regex {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").expect("valid comment pattern"))
    }

    #[test]
    fn matches_imperative_markdown_link_row() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let imperative = imperative_broken_set(project_dir);
        assert!(
            !imperative.is_empty(),
            "imperative row must flag the broken page (parity fixture invariant)"
        );

        let expected: BTreeSet<(String, String)> = [
            ("docs/broken.md".to_string(), "./missing.md".to_string()),
            ("docs/broken.md".to_string(), "../other/absent.md".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            imperative, expected,
            "imperative fixture must match the documented broken-link set",
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-002",
            vec![
                hint(HintKind::PathPattern, "docs/**/*.md"),
                hint(HintKind::ReferenceResolves, "markdown-link"),
            ],
        );
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
                Some("CORE-002"),
                "declarative findings must carry the documented CORE-002 rule id",
            );
            let loc = finding.location.as_ref().expect("location set");
            assert!(
                loc.line.is_some_and(|line| line >= 1),
                "declarative finding must record the 1-based link line",
            );
        }

        let declarative = declarative_broken_set(&outcome.findings);
        assert_eq!(
            declarative, imperative,
            "declarative CORE-002 must flag the same (file, target) pairs as the retired links.unresolved predicate",
        );
    }
}

/// `CORE-003` ≅ the retiring `skill.duplicate-name` imperative row. Both group
/// `SKILL.md` files by frontmatter `name:` and flag duplicated names; the
/// `(skill_name, sorted_paths)` set must agree.
mod core_003 {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};

    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const DUP_A: &str =
        "---\nname: duplicate-skill\ndescription: Run the duplicate-skill flow.\n---\n# Body A\n";
    const DUP_B: &str = "---\nname: duplicate-skill\ndescription: Run the duplicate-skill flow again.\n---\n# Body B\n";
    const SOLO: &str =
        "---\nname: solo-skill\ndescription: Run the solo-skill flow.\n---\n# Body solo\n";

    fn stage_project(project_dir: &Path) {
        let a = project_dir.join("plugins/alpha/skills/build/SKILL.md");
        let b = project_dir.join("plugins/beta/skills/build/SKILL.md");
        let solo = project_dir.join("plugins/gamma/skills/solo/SKILL.md");
        for parent in [a.parent(), b.parent(), solo.parent()].into_iter().flatten() {
            fs::create_dir_all(parent).expect("plugin skill dir");
        }
        fs::write(&a, DUP_A).expect("write dup A");
        fs::write(&b, DUP_B).expect("write dup B");
        fs::write(&solo, SOLO).expect("write solo");
    }

    /// Reproduces the deleted imperative
    /// `check::skill_frontmatter::check_duplicate_names` body inline; returns
    /// the `(skill_name, sorted_paths)` groups with two or more files.
    fn imperative_duplicate_set(project_dir: &Path) -> BTreeMap<String, Vec<String>> {
        let mut by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let plugins = project_dir.join("plugins");
        let mut stack: Vec<PathBuf> = vec![plugins];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.file_name().and_then(|s| s.to_str()) != Some("SKILL.md") {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&path) else { continue };
                let Some(name) = parse_frontmatter_name(&content) else {
                    continue;
                };
                let Ok(relative) = path.strip_prefix(project_dir) else { continue };
                let rel = relative.to_string_lossy().replace('\\', "/");
                by_name.entry(name).or_default().insert(rel);
            }
        }

        by_name
            .into_iter()
            .filter(|(_, paths)| paths.len() >= 2)
            .map(|(name, paths)| (name, paths.into_iter().collect::<Vec<_>>()))
            .collect()
    }

    /// Minimal `name:` extractor mirroring the retired imperative row.
    fn parse_frontmatter_name(content: &str) -> Option<String> {
        let rest = content.strip_prefix("---\n")?;
        let end = rest.find("\n---")?;
        let block = &rest[..end];
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("name:") {
                let trimmed = value.trim().trim_matches(|c: char| c == '"' || c == '\'');
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    fn declarative_duplicate_set(findings: &[Diagnostic]) -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        for finding in findings {
            let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
            let name = data.get("name").and_then(|v| v.as_str()).map(str::to_string);
            let paths = data.get("paths").and_then(|v| v.as_array()).map(|arr| {
                arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>()
            });
            if let (Some(name), Some(paths)) = (name, paths) {
                out.insert(name, paths);
            }
        }
        out
    }

    #[test]
    fn matches_duplicate_name() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let imperative = imperative_duplicate_set(project_dir);
        assert!(
            !imperative.is_empty(),
            "imperative row must flag the duplicate pair (parity fixture invariant)",
        );

        let mut expected: BTreeMap<String, Vec<String>> = BTreeMap::new();
        expected.insert(
            "duplicate-skill".to_string(),
            vec![
                "plugins/alpha/skills/build/SKILL.md".to_string(),
                "plugins/beta/skills/build/SKILL.md".to_string(),
            ],
        );
        assert_eq!(
            imperative, expected,
            "imperative fixture must match the documented duplicate-name set",
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-003",
            vec![
                hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
                hint(HintKind::Unique, "skill-name"),
            ],
        );
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
                Some("CORE-003"),
                "declarative findings must carry the documented CORE-003 rule id",
            );
            let loc = finding.location.as_ref().expect("location set");
            assert!(
                loc.path.starts_with("plugins/"),
                "declarative location path must point at a `plugins/**/SKILL.md` file: got {}",
                loc.path,
            );
        }

        let declarative = declarative_duplicate_set(&outcome.findings);
        assert_eq!(
            declarative, imperative,
            "declarative CORE-003 must flag the same (name, paths) pairs as the retired skill.duplicate-name predicate",
        );
    }
}

/// `CORE-004` ≅ the `set-coverage` reserved-kind semantics: adapter manifest
/// `briefs.keys()` must cover the axis-appropriate operation enum. No
/// imperative `Check` row is retired; an inline reference stands in.
mod core_004 {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const BAD_SOURCE: &str = concat!(
        "name: bad-source\n",
        "version: 1\n",
        "axis: source\n",
        "description: Source adapter missing `extract:`.\n",
        "briefs:\n",
        "  survey: briefs/survey.md\n",
    );
    const BAD_TARGET: &str = concat!(
        "name: bad-target\n",
        "version: 1\n",
        "axis: target\n",
        "description: Target adapter missing `merge:`.\n",
        "briefs:\n",
        "  shape: briefs/shape.md\n",
        "  build: briefs/build.md\n",
    );
    const GOOD_SOURCE: &str = concat!(
        "name: good-source\n",
        "version: 1\n",
        "axis: source\n",
        "description: Fully covered source adapter (negative control).\n",
        "briefs:\n",
        "  survey: briefs/survey.md\n",
        "  extract: briefs/extract.md\n",
    );

    const SOURCE_OPERATIONS: &[&str] = &["extract", "survey"];
    const TARGET_OPERATIONS: &[&str] = &["build", "merge", "shape"];

    fn stage_project(project_dir: &Path) {
        fs::create_dir_all(project_dir.join("plugins")).expect("plugins");
        fs::create_dir_all(project_dir.join("adapters/sources")).expect("sources");
        fs::create_dir_all(project_dir.join("adapters/targets")).expect("targets");

        for (rel, body) in [
            ("adapters/sources/bad-source/adapter.yaml", BAD_SOURCE),
            ("adapters/targets/bad-target/adapter.yaml", BAD_TARGET),
            ("adapters/sources/good-source/adapter.yaml", GOOD_SOURCE),
        ] {
            let path = project_dir.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("manifest dir");
            fs::write(&path, body).expect("write manifest");
        }
    }

    /// Inline reference mirroring `kind: set-coverage`; returns the
    /// `(adapter, axis, missing-operation)` triple set.
    fn imperative_missing_set(project_dir: &Path) -> BTreeSet<(String, String, String)> {
        let mut out = BTreeSet::new();
        for axis in ["sources", "targets"] {
            let axis_dir = project_dir.join("adapters").join(axis);
            let Ok(entries) = fs::read_dir(&axis_dir) else { continue };
            let expected: BTreeSet<&'static str> = match axis {
                "sources" => SOURCE_OPERATIONS.iter().copied().collect(),
                "targets" => TARGET_OPERATIONS.iter().copied().collect(),
                _ => unreachable!(),
            };
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let manifest_path = dir.join("adapter.yaml");
                let Ok(body) = fs::read_to_string(&manifest_path) else { continue };
                let (name, keys) = parse_manifest(&body);
                let Some(name) = name else { continue };
                for op in &expected {
                    if !keys.contains(*op) {
                        out.insert((name.clone(), axis.to_string(), (*op).to_string()));
                    }
                }
            }
        }
        out
    }

    /// Minimal manifest parser: `name:` scalar plus the keys of the top-level
    /// `briefs:` map.
    fn parse_manifest(body: &str) -> (Option<String>, BTreeSet<String>) {
        let mut name: Option<String> = None;
        let mut keys: BTreeSet<String> = BTreeSet::new();
        let mut in_briefs = false;
        for raw in body.lines() {
            if let Some(stripped) = raw.strip_prefix("name:") {
                let trimmed = stripped.trim().trim_matches(|c: char| c == '"' || c == '\'');
                if !trimmed.is_empty() {
                    name = Some(trimmed.to_string());
                }
                in_briefs = false;
                continue;
            }
            if raw == "briefs:" || raw.starts_with("briefs:") {
                in_briefs = true;
                continue;
            }
            if in_briefs {
                if raw.starts_with(' ') || raw.starts_with('\t') {
                    let line = raw.trim_start();
                    if let Some((key, _rest)) = line.split_once(':') {
                        let key = key.trim();
                        if !key.is_empty() {
                            keys.insert(key.to_string());
                        }
                    }
                } else if !raw.trim().is_empty() {
                    in_briefs = false;
                }
            }
        }
        (name, keys)
    }

    fn declarative_missing_set(findings: &[Diagnostic]) -> BTreeSet<(String, String, String)> {
        let mut out = BTreeSet::new();
        for finding in findings {
            let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
            let adapter = data.get("adapter").and_then(|v| v.as_str()).map(str::to_string);
            let axis = data.get("axis").and_then(|v| v.as_str()).map(str::to_string);
            let missing = data.get("missing").and_then(|v| v.as_str()).map(str::to_string);
            if let (Some(a), Some(x), Some(m)) = (adapter, axis, missing) {
                out.insert((a, x, m));
            }
        }
        out
    }

    #[test]
    fn matches_set_coverage() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let imperative = imperative_missing_set(project_dir);
        let expected: BTreeSet<(String, String, String)> = [
            ("bad-source".to_string(), "sources".to_string(), "extract".to_string()),
            ("bad-target".to_string(), "targets".to_string(), "merge".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            imperative, expected,
            "imperative reference must flag exactly the documented (adapter, axis, missing) triples",
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-004",
            vec![
                hint(HintKind::PathPattern, "adapters/sources/*/adapter.yaml"),
                hint(HintKind::PathPattern, "adapters/targets/*/adapter.yaml"),
                hint(HintKind::SetCoverage, "adapter-briefs-cover-operations"),
            ],
        );
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
                Some("CORE-004"),
                "declarative findings must carry the documented CORE-004 rule id",
            );
            let loc = finding.location.as_ref().expect("location set");
            assert!(
                loc.path.starts_with("adapters/"),
                "declarative location must point at an adapter manifest: got {}",
                loc.path,
            );
        }

        let declarative = declarative_missing_set(&outcome.findings);
        assert_eq!(
            declarative, imperative,
            "declarative CORE-004 must flag the same (adapter, axis, missing) triples as the inline set-coverage reference",
        );
    }
}

/// `CORE-005` ≅ the retiring `skill.body-line-count` imperative row via the
/// `cardinality` kind: SKILL.md bodies over 200 lines are flagged. The
/// `{ path -> body_line_count }` set must agree.
mod core_005 {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const SKILL_BODY_LINE_MAX: u32 = 200;

    fn write_skill(project_dir: &Path, plugin: &str, skill: &str, name: &str, body_lines: u32) {
        let body =
            (1..=body_lines).map(|i| format!("body line {i}")).collect::<Vec<_>>().join("\n");
        let content = format!(
            "---\nname: {name}\ndescription: Fixture skill for the CORE-005 parity test.\nargument-hint: <arg>\n---\n{body}\n",
        );
        let path = project_dir.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
        fs::create_dir_all(path.parent().expect("parent")).expect("plugin skill dir");
        fs::write(&path, content).expect("write skill");
    }

    fn stage_project(project_dir: &Path) {
        write_skill(project_dir, "alpha", "long", "long-skill", 300);
        write_skill(project_dir, "beta", "medium", "medium-skill", 50);
        write_skill(project_dir, "gamma", "short", "short-skill", 3);
    }

    /// Reproduces the deleted imperative
    /// `check::skill_body::check_body_line_count` body inline; returns the
    /// `{ path -> body_line_count }` map for skills over the 200-line cap.
    fn imperative_over_cap_set(project_dir: &Path) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let plugins = project_dir.join("plugins");
        let mut stack: Vec<PathBuf> = vec![plugins];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.file_name().and_then(|s| s.to_str()) != Some("SKILL.md") {
                    continue;
                }
                let Ok(content) = fs::read_to_string(&path) else { continue };
                let Some(lines) = imperative_body_lines(&content) else { continue };
                if lines.len() > SKILL_BODY_LINE_MAX as usize {
                    let Ok(relative) = path.strip_prefix(project_dir) else { continue };
                    let rel = relative.to_string_lossy().replace('\\', "/");
                    out.insert(rel, lines.len());
                }
            }
        }
        out
    }

    /// Mirror of the retired `helpers::skill_body_lines` convention.
    fn imperative_body_lines(content: &str) -> Option<Vec<String>> {
        let rest = content.strip_prefix("---\n")?;
        let end = rest.find("\n---")?;
        let block = &rest[..end];
        let start = content.find(block)? + block.len();
        let mut lines: Vec<String> = content[start..].split('\n').map(str::to_string).collect();
        if lines.first().is_some_and(String::is_empty) {
            lines.remove(0);
        }
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        Some(lines)
    }

    fn declarative_over_cap_set(findings: &[Diagnostic]) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        for finding in findings {
            let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
            let path = data.get("path").and_then(|v| v.as_str()).map(str::to_string);
            let actual = data
                .get("actual")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok());
            if let (Some(path), Some(actual)) = (path, actual) {
                out.insert(path, actual);
            }
        }
        out
    }

    #[test]
    fn matches_body_line_count() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let imperative = imperative_over_cap_set(project_dir);
        assert_eq!(
            imperative.len(),
            1,
            "imperative row must flag exactly the long-skill fixture (parity fixture invariant)",
        );
        let long_path = "plugins/alpha/skills/long/SKILL.md".to_string();
        assert!(imperative.contains_key(&long_path), "imperative must flag {long_path}");
        assert!(imperative[&long_path] > SKILL_BODY_LINE_MAX as usize);

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-005",
            vec![
                hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
                hint(HintKind::Cardinality, "skill-body-line-count-max-200"),
            ],
        );
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
                Some("CORE-005"),
                "declarative findings must carry the documented CORE-005 rule id",
            );
            let loc = finding.location.as_ref().expect("location set");
            assert!(
                loc.path.starts_with("plugins/"),
                "declarative location path must point at a `plugins/**/SKILL.md` file: got {}",
                loc.path,
            );
        }

        let declarative = declarative_over_cap_set(&outcome.findings);
        assert_eq!(
            declarative.len(),
            imperative.len(),
            "declarative CORE-005 must flag the same number of skills as the retired skill.body-line-count predicate",
        );
        let declarative_paths: Vec<&String> = declarative.keys().collect();
        let imperative_paths: Vec<&String> = imperative.keys().collect();
        assert_eq!(
            declarative_paths, imperative_paths,
            "declarative CORE-005 must flag the same paths as the retired skill.body-line-count predicate",
        );
        for path in declarative.keys() {
            let declarative_count = declarative[path];
            let imperative_count = imperative[path];
            assert!(
                declarative_count > SKILL_BODY_LINE_MAX,
                "declarative count for {path} must exceed the cap ({declarative_count})",
            );
            assert!(
                imperative_count > SKILL_BODY_LINE_MAX as usize,
                "imperative count for {path} must exceed the cap ({imperative_count})",
            );
        }
    }
}

/// `CORE-006` ≅ the `constant-eq` reserved-kind semantics: adapter manifest
/// `version:` must equal `"1"`. No imperative `Check` row is retired; an
/// inline reference stands in.
mod core_006 {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const BAD_SOURCE: &str = concat!(
        "name: bad-source\n",
        "version: 2\n",
        "axis: source\n",
        "description: Source adapter declaring the wrong manifest version.\n",
        "briefs:\n",
        "  survey: briefs/survey.md\n",
        "  extract: briefs/extract.md\n",
    );
    const BAD_TARGET: &str = concat!(
        "name: bad-target\n",
        "version: \"0.9\"\n",
        "axis: target\n",
        "description: Target adapter declaring a pre-v1 manifest version.\n",
        "briefs:\n",
        "  shape: briefs/shape.md\n",
        "  build: briefs/build.md\n",
        "  merge: briefs/merge.md\n",
    );
    const GOOD_SOURCE: &str = concat!(
        "name: good-source\n",
        "version: 1\n",
        "axis: source\n",
        "description: Conforming source adapter (negative control).\n",
        "briefs:\n",
        "  survey: briefs/survey.md\n",
        "  extract: briefs/extract.md\n",
    );

    const EXPECTED_VERSION: &str = "1";

    fn stage_project(project_dir: &Path) {
        fs::create_dir_all(project_dir.join("plugins")).expect("plugins");
        fs::create_dir_all(project_dir.join("adapters/sources")).expect("sources");
        fs::create_dir_all(project_dir.join("adapters/targets")).expect("targets");

        for (rel, body) in [
            ("adapters/sources/bad-source/adapter.yaml", BAD_SOURCE),
            ("adapters/targets/bad-target/adapter.yaml", BAD_TARGET),
            ("adapters/sources/good-source/adapter.yaml", GOOD_SOURCE),
        ] {
            let path = project_dir.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("manifest dir");
            fs::write(&path, body).expect("write manifest");
        }
    }

    /// Inline reference mirroring `kind: constant-eq`; returns the
    /// `(adapter, axis, actual_version)` triple set for manifests whose
    /// `version:` is not `EXPECTED_VERSION`.
    fn imperative_mismatch_set(project_dir: &Path) -> BTreeSet<(String, String, String)> {
        let mut out = BTreeSet::new();
        for axis in ["sources", "targets"] {
            let axis_dir = project_dir.join("adapters").join(axis);
            let Ok(entries) = fs::read_dir(&axis_dir) else { continue };
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let manifest_path = dir.join("adapter.yaml");
                let Ok(body) = fs::read_to_string(&manifest_path) else { continue };
                let (name, version) = parse_manifest(&body);
                let Some(name) = name else { continue };
                let actual = version.unwrap_or_else(|| "(absent)".to_string());
                if actual != EXPECTED_VERSION {
                    out.insert((name, axis.to_string(), actual));
                }
            }
        }
        out
    }

    /// Minimal manifest parser: the `name:` and `version:` top-level scalars.
    fn parse_manifest(body: &str) -> (Option<String>, Option<String>) {
        let mut name: Option<String> = None;
        let mut version: Option<String> = None;
        for raw in body.lines() {
            if let Some(stripped) = raw.strip_prefix("name:") {
                let trimmed = stripped.trim().trim_matches(|c: char| c == '"' || c == '\'');
                if !trimmed.is_empty() {
                    name = Some(trimmed.to_string());
                }
                continue;
            }
            if let Some(stripped) = raw.strip_prefix("version:") {
                let trimmed = stripped.trim().trim_matches(|c: char| c == '"' || c == '\'');
                if !trimmed.is_empty() {
                    version = Some(trimmed.to_string());
                }
            }
        }
        (name, version)
    }

    fn declarative_mismatch_set(findings: &[Diagnostic]) -> BTreeSet<(String, String, String)> {
        let mut out = BTreeSet::new();
        for finding in findings {
            let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
            let adapter = data.get("adapter").and_then(|v| v.as_str()).map(str::to_string);
            let actual = data.get("actual").and_then(|v| v.as_str()).map(str::to_string);
            let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let axis = axis_from_path(path).map(str::to_string);
            if let (Some(a), Some(x), Some(v)) = (adapter, axis, actual) {
                out.insert((a, x, v));
            }
        }
        out
    }

    /// Recover the `sources` / `targets` axis from a manifest's project-relative path.
    fn axis_from_path(path: &str) -> Option<&'static str> {
        let rest = path.strip_prefix("adapters/")?;
        let (axis, _rest) = rest.split_once('/')?;
        match axis {
            "sources" => Some("sources"),
            "targets" => Some("targets"),
            _ => None,
        }
    }

    #[test]
    fn matches_constant_eq() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let imperative = imperative_mismatch_set(project_dir);
        let expected: BTreeSet<(String, String, String)> = [
            ("bad-source".to_string(), "sources".to_string(), "2".to_string()),
            ("bad-target".to_string(), "targets".to_string(), "0.9".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            imperative, expected,
            "imperative reference must flag exactly the documented (adapter, axis, actual) triples",
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-006",
            vec![
                hint(HintKind::PathPattern, "adapters/sources/*/adapter.yaml"),
                hint(HintKind::PathPattern, "adapters/targets/*/adapter.yaml"),
                hint(HintKind::ConstantEq, "adapter-manifest-version-equals-v1"),
            ],
        );
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
                Some("CORE-006"),
                "declarative findings must carry the documented CORE-006 rule id",
            );
            let loc = finding.location.as_ref().expect("location set");
            assert!(
                loc.path.starts_with("adapters/"),
                "declarative location must point at an adapter manifest: got {}",
                loc.path,
            );
        }

        let declarative = declarative_mismatch_set(&outcome.findings);
        assert_eq!(
            declarative, imperative,
            "declarative CORE-006 must flag the same (adapter, axis, actual) triples as the inline constant-eq reference",
        );
    }
}

/// `CORE-007` ≅ the `set-eq` reserved-kind semantics: adapter manifest
/// `briefs.keys()` must exactly equal the axis-appropriate operation enum
/// (both missing and unexpected keys). No imperative `Check` row is retired.
mod core_007 {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const BAD_SOURCE: &str = concat!(
        "name: bad-source\n",
        "version: 1\n",
        "axis: source\n",
        "description: Source adapter missing `extract:`.\n",
        "briefs:\n",
        "  survey: briefs/survey.md\n",
    );
    const BAD_TARGET: &str = concat!(
        "name: bad-target\n",
        "version: 1\n",
        "axis: target\n",
        "description: Target adapter missing `merge:` and carrying an unexpected key.\n",
        "briefs:\n",
        "  shape: briefs/shape.md\n",
        "  build: briefs/build.md\n",
        "  extra: briefs/extra.md\n",
    );
    const EXTRA_SOURCE: &str = concat!(
        "name: extra-source\n",
        "version: 1\n",
        "axis: source\n",
        "description: Source adapter with a complete set plus a stray key.\n",
        "briefs:\n",
        "  survey: briefs/survey.md\n",
        "  extract: briefs/extract.md\n",
        "  legacy: briefs/legacy.md\n",
    );
    const GOOD_TARGET: &str = concat!(
        "name: good-target\n",
        "version: 1\n",
        "axis: target\n",
        "description: Exactly the target operation set (negative control).\n",
        "briefs:\n",
        "  shape: briefs/shape.md\n",
        "  build: briefs/build.md\n",
        "  merge: briefs/merge.md\n",
    );

    const SOURCE_OPERATIONS: &[&str] = &["extract", "survey"];
    const TARGET_OPERATIONS: &[&str] = &["build", "merge", "shape"];

    const DIVERGENCE_MISSING: &str = "missing";
    const DIVERGENCE_UNEXPECTED: &str = "unexpected";

    fn stage_project(project_dir: &Path) {
        fs::create_dir_all(project_dir.join("plugins")).expect("plugins");
        fs::create_dir_all(project_dir.join("adapters/sources")).expect("sources");
        fs::create_dir_all(project_dir.join("adapters/targets")).expect("targets");

        for (rel, body) in [
            ("adapters/sources/bad-source/adapter.yaml", BAD_SOURCE),
            ("adapters/targets/bad-target/adapter.yaml", BAD_TARGET),
            ("adapters/sources/extra-source/adapter.yaml", EXTRA_SOURCE),
            ("adapters/targets/good-target/adapter.yaml", GOOD_TARGET),
        ] {
            let path = project_dir.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("manifest dir");
            fs::write(&path, body).expect("write manifest");
        }
    }

    /// Inline reference mirroring `kind: set-eq`; returns the
    /// `(adapter, axis, divergence, operation)` quadruple set for both halves
    /// of the symmetric difference against the axis-appropriate enum.
    fn imperative_divergence_set(project_dir: &Path) -> BTreeSet<(String, String, String, String)> {
        let mut out = BTreeSet::new();
        for axis in ["sources", "targets"] {
            let axis_dir = project_dir.join("adapters").join(axis);
            let Ok(entries) = fs::read_dir(&axis_dir) else { continue };
            let expected: BTreeSet<&'static str> = match axis {
                "sources" => SOURCE_OPERATIONS.iter().copied().collect(),
                "targets" => TARGET_OPERATIONS.iter().copied().collect(),
                _ => unreachable!(),
            };
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let manifest_path = dir.join("adapter.yaml");
                let Ok(body) = fs::read_to_string(&manifest_path) else { continue };
                let (name, keys) = parse_manifest(&body);
                let Some(name) = name else { continue };
                for op in &expected {
                    if !keys.contains(*op) {
                        out.insert((
                            name.clone(),
                            axis.to_string(),
                            DIVERGENCE_MISSING.to_string(),
                            (*op).to_string(),
                        ));
                    }
                }
                for key in &keys {
                    if !expected.contains(key.as_str()) {
                        out.insert((
                            name.clone(),
                            axis.to_string(),
                            DIVERGENCE_UNEXPECTED.to_string(),
                            key.clone(),
                        ));
                    }
                }
            }
        }
        out
    }

    /// Minimal manifest parser: `name:` scalar plus the keys of the top-level
    /// `briefs:` map.
    fn parse_manifest(body: &str) -> (Option<String>, BTreeSet<String>) {
        let mut name: Option<String> = None;
        let mut keys: BTreeSet<String> = BTreeSet::new();
        let mut in_briefs = false;
        for raw in body.lines() {
            if let Some(stripped) = raw.strip_prefix("name:") {
                let trimmed = stripped.trim().trim_matches(|c: char| c == '"' || c == '\'');
                if !trimmed.is_empty() {
                    name = Some(trimmed.to_string());
                }
                in_briefs = false;
                continue;
            }
            if raw == "briefs:" || raw.starts_with("briefs:") {
                in_briefs = true;
                continue;
            }
            if in_briefs {
                if raw.starts_with(' ') || raw.starts_with('\t') {
                    let line = raw.trim_start();
                    if let Some((key, _rest)) = line.split_once(':') {
                        let key = key.trim();
                        if !key.is_empty() {
                            keys.insert(key.to_string());
                        }
                    }
                } else if !raw.trim().is_empty() {
                    in_briefs = false;
                }
            }
        }
        (name, keys)
    }

    fn declarative_divergence_set(
        findings: &[Diagnostic],
    ) -> BTreeSet<(String, String, String, String)> {
        let mut out = BTreeSet::new();
        for finding in findings {
            let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
            let adapter = data.get("adapter").and_then(|v| v.as_str()).map(str::to_string);
            let axis = data.get("axis").and_then(|v| v.as_str()).map(str::to_string);
            let divergence = data.get("divergence").and_then(|v| v.as_str()).map(str::to_string);
            let operation = data.get("operation").and_then(|v| v.as_str()).map(str::to_string);
            if let (Some(a), Some(x), Some(d), Some(o)) = (adapter, axis, divergence, operation) {
                out.insert((a, x, d, o));
            }
        }
        out
    }

    #[test]
    fn matches_set_eq() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let imperative = imperative_divergence_set(project_dir);
        let expected: BTreeSet<(String, String, String, String)> = [
            ("bad-source", "sources", DIVERGENCE_MISSING, "extract"),
            ("bad-target", "targets", DIVERGENCE_MISSING, "merge"),
            ("bad-target", "targets", DIVERGENCE_UNEXPECTED, "extra"),
            ("extra-source", "sources", DIVERGENCE_UNEXPECTED, "legacy"),
        ]
        .into_iter()
        .map(|(a, x, d, o)| (a.to_string(), x.to_string(), d.to_string(), o.to_string()))
        .collect();
        assert_eq!(
            imperative, expected,
            "imperative reference must flag exactly the documented (adapter, axis, divergence, operation) quadruples",
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-007",
            vec![
                hint(HintKind::PathPattern, "adapters/sources/*/adapter.yaml"),
                hint(HintKind::PathPattern, "adapters/targets/*/adapter.yaml"),
                hint(HintKind::SetEq, "adapter-briefs-equal-operations"),
            ],
        );
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
                Some("CORE-007"),
                "declarative findings must carry the documented CORE-007 rule id",
            );
            let loc = finding.location.as_ref().expect("location set");
            assert!(
                loc.path.starts_with("adapters/"),
                "declarative location must point at an adapter manifest: got {}",
                loc.path,
            );
        }

        let declarative = declarative_divergence_set(&outcome.findings);
        assert_eq!(
            declarative, imperative,
            "declarative CORE-007 must flag the same (adapter, axis, divergence, operation) quadruples as the inline set-eq reference",
        );
    }
}

/// `CORE-008` ≅ the `content-digest-eq` reserved-kind semantics: every
/// `agent-teams.md` symlink must resolve to content whose SHA-256 equals the
/// canonical review-team-protocol document. No imperative `Check` row is retired.
mod core_008 {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

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
                let resolved = fs::canonicalize(&path)
                    .ok()
                    .and_then(|c| canonical_project_rel(project_dir, &c));
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
        path.strip_prefix(project_dir).map_or_else(
            |_| path.display().to_string(),
            |rel| rel.to_string_lossy().replace('\\', "/"),
        )
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
            std::iter::once("adapters/targets/drifted/references/agent-teams.md".to_string())
                .collect();
        assert_eq!(
            imperative, expected,
            "imperative reference must flag exactly the drifted agent-teams.md symlink",
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-008",
            vec![hint(HintKind::ContentDigestEq, "agent-teams-match-canonical")],
        );
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
}

/// `CORE-009` ≅ the `namespace-owner` reserved-kind semantics: each rule's
/// id-namespace prefix must be authored only under the rules directory that
/// owns that namespace. No imperative `Check` row is retired.
mod core_009 {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::Path;

    use specify_diagnostics::{Diagnostic, FindingEvidence};
    use specify_standards::lint::ScanProfile;
    use specify_standards::lint::eval::{ToolRunner, evaluate};
    use specify_standards::lint::index::build;
    use specify_standards::rules::HintKind;

    use super::{NoToolRunner, hint, make_rule};

    const RULE_GLOB: &str = "adapters/**/rules/**/*.md";

    /// `(relative path, rule id)` for every staged rule file.
    const RULES: &[(&str, &str)] = &[
        ("adapters/shared/rules/core/CORE-001-aligned.md", "CORE-001"),
        ("adapters/shared/rules/core/UNI-misplaced.md", "UNI-001"),
        ("adapters/targets/omnia/rules/OMNIA-001-aligned.md", "OMNIA-001"),
        ("adapters/targets/omnia/rules/VECTIS-misplaced.md", "VECTIS-001"),
        ("adapters/sources/documentation/rules/SRC-001-aligned.md", "SRC-001"),
    ];

    /// Stage the synthetic framework tree of rule files.
    fn stage_project(project_dir: &Path) {
        for (rel, id) in RULES {
            let path = project_dir.join(rel);
            fs::create_dir_all(path.parent().expect("rule parent")).expect("create parent");
            let body = format!(
                "---\nid: {id}\ntitle: Parity Fixture\nseverity: optional\ntrigger: Namespace ownership parity fixture covering rule placement.\n---\n\n## Rule\n\nBody.\n"
            );
            fs::write(path, body).expect("write rule");
        }
    }

    /// Inline reference mirroring `kind: namespace-owner`; returns the set of
    /// rule paths whose id-prefix is not owned by the containing rules directory.
    fn imperative_misplaced_set(project_dir: &Path) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for (rel, id) in RULES {
            drop(fs::read_to_string(project_dir.join(rel)).expect("rule readable"));
            let Some(allowed) = owned_namespaces(rel) else { continue };
            let Some(prefix) = namespace_prefix(id) else { continue };
            if !allowed.contains(prefix) {
                out.insert((*rel).to_string());
            }
        }
        out
    }

    fn owned_namespaces(path: &str) -> Option<BTreeSet<&'static str>> {
        if path.starts_with("adapters/shared/rules/universal/") {
            return Some(BTreeSet::from(["UNI"]));
        }
        if path.starts_with("adapters/shared/rules/core/") {
            return Some(BTreeSet::from(["CORE"]));
        }
        let targets: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::from([
            ("omnia", BTreeSet::from(["OMNIA", "RUST", "SEC"])),
            ("contracts", BTreeSet::from(["IFACE"])),
            ("vectis", BTreeSet::from(["VECTIS"])),
        ]);
        if let Some(rest) = path.strip_prefix("adapters/targets/")
            && let Some((name, tail)) = rest.split_once('/')
            && tail.starts_with("rules/")
        {
            return targets.get(name).cloned();
        }
        if let Some(rest) = path.strip_prefix("adapters/sources/")
            && let Some((_, tail)) = rest.split_once('/')
            && tail.starts_with("rules/")
        {
            return Some(BTreeSet::from(["SRC"]));
        }
        None
    }

    fn namespace_prefix(id: &str) -> Option<&str> {
        let (prefix, suffix) = id.split_once('-')?;
        let well_formed = !prefix.is_empty()
            && prefix.bytes().all(|b| b.is_ascii_uppercase())
            && suffix.len() == 3
            && suffix.bytes().all(|b| b.is_ascii_digit());
        well_formed.then_some(prefix)
    }

    fn declarative_misplaced_set(findings: &[Diagnostic]) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for finding in findings {
            let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
            if let Some(rule) = data.get("rule").and_then(|v| v.as_str()) {
                out.insert(rule.to_string());
            }
        }
        out
    }

    #[test]
    fn matches_namespace_owner() {
        let project = tempfile::tempdir().expect("tempdir");
        let project_dir = project.path();
        stage_project(project_dir);

        let imperative = imperative_misplaced_set(project_dir);
        let expected: BTreeSet<String> = [
            "adapters/shared/rules/core/UNI-misplaced.md".to_string(),
            "adapters/targets/omnia/rules/VECTIS-misplaced.md".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            imperative, expected,
            "imperative reference must flag exactly the two misplaced rule files",
        );

        let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule = make_rule(
            "CORE-009",
            vec![
                hint(HintKind::PathPattern, RULE_GLOB),
                hint(HintKind::NamespaceOwner, "rule-namespace-matches-owner"),
            ],
        );
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
                Some("CORE-009"),
                "declarative findings must carry the documented CORE-009 rule id",
            );
            let loc = finding.location.as_ref().expect("location set");
            assert!(
                Path::new(&loc.path).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
                "declarative location must point at a rule markdown file: got {}",
                loc.path,
            );
        }

        let declarative = declarative_misplaced_set(&outcome.findings);
        assert_eq!(
            declarative, imperative,
            "declarative CORE-009 must flag the same rule files as the inline namespace-owner reference",
        );
    }
}
