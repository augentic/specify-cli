//! End-to-end binary tests for `specify lint project` (`specify
//! review` (Phase 2 CLI)").
//!
//! Exercises the wired clap surface, `--rules-root` resolution per rules-root resolution,
//! the `--dump-model` debug branch, and the lint exit mapping exit-code map for the
//! `rules-root-required` negative scenario. The deterministic happy
//! path uses a single shared `kind: regex` UNI-100 rule that matches a
//! literal `TODO` token in the project — chosen because the regex
//! evaluator is the simplest Phase 2 hint that surfaces an
//! `important` finding without requiring a WASI tool to be built.
//!
//! Beyond `regex` / `schema` / `kind: tool`, the per-kind cases below
//! drive every Road A declarative kind reachable under the
//! `ScanProfile::Project` fact set the consumer surface builds —
//! `path-pattern`, `presence`, `field-grammar`, `set-coverage`,
//! `cardinality`, `reference-resolves`, `fenced-block` — through the
//! binary, mirroring the crate-level `crates/standards/tests/lint_hint/`
//! cases at integration level. The four kinds bound to framework-only
//! fact families (`cross-reference`, `set-eq`, `constant-eq`,
//! `unique`) are unreachable through `lint project`
//! because the project profile never indexes adapter / skill / scenario
//! facts; they are proven through `specify lint framework`
//! in `tests/lint/framework.rs` instead.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use jsonschema::{Registry, Resource, Validator};
use serde_json::Value;
use specify_schema::{
    DIAGNOSTIC_JSON_SCHEMA, DIAGNOSTIC_REPORT_JSON_SCHEMA, WORKSPACE_MODEL_JSON_SCHEMA,
};
use tempfile::TempDir;

const FINDING_SCHEMA_URL: &str =
    "https://github.com/augentic/specify-cli/schemas/diagnostics/diagnostic.schema.json";

/// Compile the diagnostic-report envelope schema with the
/// `diagnostic.schema.json` child resource wired through a
/// `jsonschema::Registry`. Mirrors the
/// `specify_diagnostics::render` with `Format::Json` setup so the
/// e2e test re-validates the same shape the CLI emits.
fn compile_review_result_validator() -> Validator {
    let envelope: Value =
        serde_json::from_str(DIAGNOSTIC_REPORT_JSON_SCHEMA).expect("envelope schema");
    let finding: Value = serde_json::from_str(DIAGNOSTIC_JSON_SCHEMA).expect("finding schema");
    let registry = Registry::new()
        .add(FINDING_SCHEMA_URL, Resource::from_contents(finding))
        .and_then(jsonschema::RegistryBuilder::prepare)
        .expect("registry build");
    jsonschema::options().with_registry(&registry).build(&envelope).expect("validator build")
}

fn compile_workspace_model_validator() -> Validator {
    let schema: Value = serde_json::from_str(WORKSPACE_MODEL_JSON_SCHEMA).expect("parse schema");
    jsonschema::validator_for(&schema).expect("validator build")
}

#[track_caller]
fn assert_validates(validator: &Validator, stdout: &str, schema_label: &str) {
    let instance: Value = serde_json::from_str(stdout)
        .unwrap_or_else(|err| panic!("stdout is not JSON ({err}); raw:\n{stdout}"));
    let errors: Vec<String> = validator.iter_errors(&instance).map(|err| err.to_string()).collect();
    assert!(
        errors.is_empty(),
        "stdout failed schema validation ({schema_label}): {errors:?}; raw:\n{stdout}"
    );
}

/// Scratch workspace used by the happy-path scenarios.
///
/// Lives in the tempdir until the test returns. Two trees are produced:
///
/// - `project_dir/` — a minimal initialised project (`.specify/project.yaml`
///   declaring the `contract` tool so the `kind: tool` hint family at
///   least passes the `kind: tool` evaluator contract `is_declared` half) plus a `notes.md` file
///   carrying the literal `TODO` token the UNI-100 regex hint matches.
/// - `codex_dir/` — a fresh rules tree with one shared rule under
///   `adapters/shared/rules/universal/uni-100.md`. The rule's
///   `kind: regex` hint pattern is `TODO`.
struct Fixture {
    _root: TempDir,
    project: std::path::PathBuf,
    codex: std::path::PathBuf,
}

fn build_fixture() -> Fixture {
    let root = TempDir::new().expect("create tempdir");
    let project = root.path().join("project");
    let codex = root.path().join("rules");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::create_dir_all(codex.join("adapters/shared/rules/universal")).expect("mkdir codex");

    fs::write(
        project.join(".specify").join("project.yaml"),
        concat!(
            "name: review-e2e\n",
            "tools:\n",
            "  - name: contract\n",
            "    version: 0.1.0\n",
            "    source: https://example.com/contract.wasm\n",
        ),
    )
    .expect("write project.yaml");

    fs::write(project.join("notes.md"), "# Project notes\n\nTODO: drop scaffolding.\n")
        .expect("write notes.md");

    fs::write(
        codex.join("adapters/shared/rules/universal/uni-100.md"),
        concat!(
            "---\n",
            "id: UNI-100\n",
            "title: Forbid scaffolding TODOs\n",
            "severity: important\n",
            "trigger: TODO comments leak development scaffolding into shipped artefacts.\n",
            "lint_mode: deterministic\n",
            "rule_hints:\n",
            "  - kind: regex\n",
            "    value: TODO\n",
            "---\n",
            "## Rule\n",
            "\n",
            "Strip scaffolding TODOs before merge.\n",
        ),
    )
    .expect("write UNI-100");

    Fixture {
        _root: root,
        project,
        codex,
    }
}

/// Write a `kind: regex` UNI rule into `codex` at the given severity.
///
/// Mirrors the inline rule `build_fixture` writes, but parameterised on
/// `id` / `severity` / `pattern` so the blocking-tier tests can stand up
/// `suggestion`-severity rules (which never gate) alongside the default
/// `important` one (which does).
fn write_regex_rule(codex: &Path, id: &str, severity: &str, pattern: &str) {
    let slug = id.to_ascii_lowercase();
    fs::write(
        codex.join(format!("adapters/shared/rules/universal/{slug}.md")),
        format!(
            "---\n\
             id: {id}\n\
             title: Forbid scaffolding {pattern}\n\
             severity: {severity}\n\
             trigger: {pattern} tokens leak development scaffolding into shipped artefacts.\n\
             lint_mode: deterministic\n\
             rule_hints:\n\
             \x20 - kind: regex\n\
             \x20   value: {pattern}\n\
             ---\n\
             ## Rule\n\nStrip scaffolding {pattern} before merge.\n",
        ),
    )
    .unwrap_or_else(|err| panic!("write rule {id}: {err}"));
}

fn run_review(project: &Path, codex: Option<&Path>, extra: &[&str]) -> std::process::Output {
    let mut cmd = Command::cargo_bin("specify").expect("cargo_bin(specify)");
    // The global `--format` toggles the error-envelope shape; the
    // per-subcommand `--output-format` selects the the diagnostics formatter set closed set.
    cmd.arg("--format").arg("json");
    cmd.arg("lint").arg("project");
    cmd.arg("--target").arg("omnia");
    cmd.arg("--project-dir").arg(project);
    cmd.arg("--output-format").arg("json");
    if let Some(codex) = codex {
        cmd.arg("--rules-root").arg(codex);
    }
    cmd.env_remove("RULES_ROOT");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.output().expect("specify invocation")
}

/// Happy path: a single `important` finding from the UNI-100 regex
/// hint lands stdout on a schema-valid review envelope and exits 2 per
/// lint exit mapping.
#[test]
fn review_emits_important_exits_2() {
    let fx = build_fixture();
    let output = run_review(&fx.project, Some(&fx.codex), &[]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    let validator = compile_review_result_validator();
    assert_validates(&validator, stdout, "review-result");
    let envelope: Value = serde_json::from_str(stdout).expect("parse envelope");
    let important = envelope
        .pointer("/summary/important")
        .and_then(Value::as_u64)
        .expect("summary.important present");
    assert!(
        important >= 1,
        "expected ≥1 important finding, got {important}; envelope:\n{envelope:#}"
    );

    let rule_id = envelope
        .pointer("/findings/0/rule-id")
        .and_then(Value::as_str)
        .expect("findings[0].rule-id");
    assert_eq!(rule_id, "UNI-100", "envelope:\n{envelope:#}");
}

/// Bare `specify lint` without a subcommand must fail at clap parse time.
#[test]
fn bare_lint_requires_subcommand() {
    let mut cmd = Command::cargo_bin("specify").expect("cargo_bin(specify)");
    cmd.arg("lint");
    let output = cmd.output().expect("specify invocation");

    assert!(
        !output.status.success(),
        "bare `specify lint` must fail; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        combined.contains("project") || combined.contains("subcommand"),
        "failure must hint at required subcommand; got:\n{combined}"
    );
}

/// `DiagnosticReport` envelope byte-stability: two back-to-back runs against the same fixture
/// must emit byte-identical stdout. Pins the deterministic ordering
/// contract through the CLI boundary.
#[test]
fn review_run_byte_stable() {
    let fx = build_fixture();
    let first = run_review(&fx.project, Some(&fx.codex), &[]);
    let second = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        first.stdout, second.stdout,
        "consecutive specify lint runs must emit byte-identical stdout"
    );
}

/// `--dump-model` skips evaluation, emits a `WorkspaceModel` envelope
/// that validates against the workspace-model schema, and exits 0.
#[test]
fn review_dump_model_exits_0() {
    let fx = build_fixture();
    let output = run_review(&fx.project, Some(&fx.codex), &["--dump-model"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "expected exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    let validator = compile_workspace_model_validator();
    assert_validates(&validator, stdout, "workspace-model");
}

/// Journal event: every completed scan appends one
/// `lint-completed` line to `.specify/journal.jsonl` with the closed
/// `snake_case` payload shape. The fixture wires a Markdown directive
/// that demotes the UNI-100 TODO finding so the asserted counts
/// straddle both buckets (`ignored: 1`, `open: 0`) and the scan exits
/// clean (`exit_code: 0`) — proving the journal `exit_code` mirrors
/// the status-aware exit decision the exit and presentation
/// semantics define.
#[test]
fn emits_lint_completed_event() {
    use std::path::PathBuf;

    let root = TempDir::new().expect("create tempdir");
    let project: PathBuf = root.path().join("project");
    let codex: PathBuf = root.path().join("rules");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::create_dir_all(codex.join("adapters/shared/rules/universal")).expect("mkdir codex");

    fs::write(project.join(".specify").join("project.yaml"), "name: review-journal-e2e\n")
        .expect("write project.yaml");

    // `<!-- specify-ignore: UNI-100 — … -->` lands on line 2 so the
    // directive's `target_line` resolves to the next non-blank,
    // non-comment line: the TODO on line 3.
    fs::write(
        project.join("notes.md"),
        concat!(
            "# Project notes\n",
            "<!-- specify-ignore: UNI-100 — accepted tech-debt sentinel for the demo -->\n",
            "TODO: drop scaffolding.\n",
        ),
    )
    .expect("write notes.md");

    fs::write(
        codex.join("adapters/shared/rules/universal/uni-100.md"),
        concat!(
            "---\n",
            "id: UNI-100\n",
            "title: Forbid scaffolding TODOs\n",
            "severity: important\n",
            "trigger: TODO comments leak development scaffolding into shipped artefacts.\n",
            "lint_mode: deterministic\n",
            "rule_hints:\n",
            "  - kind: regex\n",
            "    value: TODO\n",
            "---\n",
            "## Rule\n\nStrip scaffolding TODOs before merge.\n",
        ),
    )
    .expect("write UNI-100");

    let output = run_review(&project, Some(&codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "directive demotes the only finding to `ignored`; scan must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let raw = fs::read_to_string(project.join(".specify").join("journal.jsonl"))
        .expect("read journal.jsonl");
    let last_line =
        raw.lines().rfind(|l| !l.is_empty()).expect("journal must contain at least one line");
    let event: Value = serde_json::from_str(last_line)
        .unwrap_or_else(|err| panic!("last journal line is not JSON ({err}): {last_line}"));

    assert_eq!(
        event.pointer("/event").and_then(Value::as_str),
        Some("lint-completed"),
        "last journal line must be the lint-completed event; got:\n{event:#}",
    );

    let payload = event.get("payload").expect("payload object present");
    assert_eq!(payload.pointer("/scope/target").and_then(Value::as_str), Some("omnia"));
    assert!(
        payload.pointer("/scope/slice").is_some_and(Value::is_null),
        "slice must serialise to JSON null when --slice is absent; payload:\n{payload:#}",
    );
    assert!(
        payload.pointer("/scope/artifact").is_some_and(Value::is_null),
        "artifact must serialise to JSON null on full scans; payload:\n{payload:#}",
    );
    assert_eq!(
        payload.pointer("/counts/open").and_then(Value::as_u64),
        Some(0),
        "the only finding is directive-demoted; open bucket must be empty: {payload:#}",
    );
    assert_eq!(
        payload.pointer("/counts/ignored").and_then(Value::as_u64),
        Some(1),
        "the directive demotes UNI-100; ignored bucket must be 1: {payload:#}",
    );
    assert_eq!(payload.pointer("/counts/false_positive").and_then(Value::as_u64), Some(0));
    assert_eq!(payload.pointer("/baseline_present").and_then(Value::as_bool), Some(false));
    assert_eq!(payload.pointer("/exit_code").and_then(Value::as_i64), Some(0));
    assert!(
        payload.pointer("/duration_ms").and_then(Value::as_u64).is_some(),
        "duration_ms must be present and serialise as a JSON number: {payload:#}",
    );

    for forbidden in ["duration-ms", "baseline-present", "false-positive", "exit-code"] {
        assert!(
            !last_line.contains(&format!("\"{forbidden}\"")),
            "lint-completed payload must use snake_case field names; raw:\n{last_line}",
        );
    }
}

/// rules-root resolution / lint exit mapping negative: with no `--rules-root`, no project-local
/// `adapters/shared/rules/universal/` rung, and no distributed
/// `.specify/cache/codex/` cache, the resolver returns
/// `rules-root-required`. The CLI surfaces it on stderr and exits 2.
#[test]
fn review_missing_rules_root_exits_2() {
    let project_root = TempDir::new().expect("project tempdir");
    let project = project_root.path().join("project");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::write(project.join(".specify").join("project.yaml"), "name: review-e2e-missing\n")
        .expect("write project.yaml");

    // Pass `--format json` so the failure envelope renders as JSON on
    // stderr (with the kebab-case `rule-id` field). The text branch
    // collapses to `error: validation failed: N errors` and would
    // hide the closed `rules-root-required` discriminant.
    let mut cmd = Command::cargo_bin("specify").expect("cargo_bin(specify)");
    cmd.arg("--format")
        .arg("json")
        .arg("lint")
        .arg("project")
        .arg("--target")
        .arg("omnia")
        .arg("--project-dir")
        .arg(&project)
        .arg("--output-format")
        .arg("json")
        .env_remove("RULES_ROOT");
    let output = cmd.output().expect("specify invocation");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = std::str::from_utf8(&output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("rules-root-required"),
        "stderr must mention rules-root-required; got:\n{stderr}"
    );
}

/// Blocking-tier exit decision (non-blocking half): a `suggestion`-severity
/// rule that matches still surfaces a finding on the envelope, but the scan
/// exits `0` because only `critical | important` violations gate. Pins the
/// `blocking` predicate (`crates/diagnostics/src/diagnostic.rs`) through the
/// CLI boundary — today's tests only cover `important` -> exit 2 and the
/// directive-demoted / `--dump-model` exit-0 paths, never a present-but-
/// non-blocking finding.
#[test]
fn suggestion_finding_present_exits_0() {
    let fx = build_fixture();
    // Overwrite the default `important` UNI-100 with a `suggestion`-tier
    // rule matching the same `TODO` token in `notes.md`.
    write_regex_rule(&fx.codex, "UNI-100", "suggestion", "TODO");
    let output = run_review(&fx.project, Some(&fx.codex), &[]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a suggestion-tier finding is non-blocking; scan must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    let envelope: Value = serde_json::from_str(stdout).expect("parse envelope");
    let suggestion = envelope
        .pointer("/summary/suggestion")
        .and_then(Value::as_u64)
        .expect("summary.suggestion present");
    assert!(
        suggestion >= 1,
        "the finding must still surface in the envelope, just non-blocking; envelope:\n{envelope:#}"
    );
    let important =
        envelope.pointer("/summary/important").and_then(Value::as_u64).unwrap_or_default();
    let critical =
        envelope.pointer("/summary/critical").and_then(Value::as_u64).unwrap_or_default();
    assert_eq!(
        important + critical,
        0,
        "no blocking-tier finding should exist; envelope:\n{envelope:#}"
    );
}

/// One throwaway project + codex pair carrying exactly the facts one
/// hint-kind case needs. Mirrors [`Fixture`] but parameterises the
/// project tree so each per-kind scenario supplies only the files its
/// rule consumes, instead of the fixed `notes.md` / UNI-100 pair.
struct HintFixture {
    _root: TempDir,
    project: std::path::PathBuf,
    codex: std::path::PathBuf,
}

/// Scaffold a [`HintFixture`]: an initialised project carrying each
/// `(relative-path, body)` in `files`, plus an empty universal-rules
/// tree the caller fills with [`write_hint_rule`].
fn scaffold_hint_fixture(files: &[(&str, &str)]) -> HintFixture {
    let root = TempDir::new().expect("create tempdir");
    let project = root.path().join("project");
    let codex = root.path().join("rules");
    fs::create_dir_all(project.join(".specify")).expect("mkdir project/.specify");
    fs::create_dir_all(codex.join("adapters/shared/rules/universal")).expect("mkdir codex");
    fs::write(project.join(".specify").join("project.yaml"), "name: hint-kind-e2e\n")
        .expect("write project.yaml");
    for (rel, body) in files {
        let path = project.join(rel);
        fs::create_dir_all(path.parent().expect("file parent")).expect("mkdir file parent");
        fs::write(&path, body).unwrap_or_else(|err| panic!("write {rel}: {err}"));
    }
    HintFixture {
        _root: root,
        project,
        codex,
    }
}

/// Write one `lint_mode: deterministic` UNI rule whose `rule_hints:`
/// body is `hints` verbatim (each line already indented two spaces to
/// sit under the `rule_hints:` key, terminated by a newline). Severity
/// is fixed `important` so a single finding gates the scan to exit 2
/// and a clean pass exits 0 — the same exit contract the regex happy
/// path relies on.
fn write_hint_rule(codex: &Path, id: &str, hints: &str) {
    let slug = id.to_ascii_lowercase();
    fs::write(
        codex.join(format!("adapters/shared/rules/universal/{slug}.md")),
        format!(
            "---\n\
             id: {id}\n\
             title: Synthetic {id}\n\
             severity: important\n\
             trigger: Synthetic hint-kind coverage rule for {id}.\n\
             lint_mode: deterministic\n\
             rule_hints:\n\
             {hints}\
             ---\n\
             ## Rule\n\nSynthetic rule body for {id}.\n",
        ),
    )
    .unwrap_or_else(|err| panic!("write rule {id}: {err}"));
}

/// Parse a `run_review` invocation's stdout into the `DiagnosticReport`
/// envelope, panicking with stderr context on a non-JSON body.
#[track_caller]
fn parse_envelope(output: &std::process::Output) -> Value {
    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    serde_json::from_str(stdout).unwrap_or_else(|err| {
        panic!("stdout is not JSON ({err}); stderr:\n{}", String::from_utf8_lossy(&output.stderr))
    })
}

/// Findings on `envelope` whose `rule-id` equals `rule_id`, in wire
/// order.
fn findings_for<'a>(envelope: &'a Value, rule_id: &str) -> Vec<&'a Value> {
    envelope
        .get("findings")
        .and_then(Value::as_array)
        .map(|findings| {
            findings
                .iter()
                .filter(|f| f.get("rule-id").and_then(Value::as_str) == Some(rule_id))
                .collect()
        })
        .unwrap_or_default()
}

/// The `location.path` of a finding, or `""` when absent.
fn finding_path(finding: &Value) -> &str {
    finding.pointer("/location/path").and_then(Value::as_str).unwrap_or_default()
}

/// `kind: path-pattern` through the binary: a rule pairing
/// `path-pattern: *.rs` with `regex: \bfn\b` must let the regex see
/// only the `*.rs` candidate, so the `fn` token in a sibling `.md`
/// file is never flagged. Mirrors the crate-level
/// `path_pattern_narrows_candidates`.
#[test]
fn path_pattern_scopes_regex() {
    let fx = scaffold_hint_fixture(&[
        ("lib.rs", "fn main() {}\n"),
        ("notes.md", "Prose that mentions fn outside any Rust file.\n"),
    ]);
    write_hint_rule(
        &fx.codex,
        "UNI-200",
        "  - kind: path-pattern\n    value: \"*.rs\"\n  - kind: regex\n    value: '\\bfn\\b'\n",
    );
    let output = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "the .rs match must gate the scan; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let envelope = parse_envelope(&output);
    let findings = findings_for(&envelope, "UNI-200");
    assert!(
        !findings.is_empty(),
        "the path-pattern survivor must be flagged; envelope:\n{envelope:#}"
    );
    for finding in &findings {
        assert_eq!(
            finding_path(finding),
            "lib.rs",
            "regex must only see path-pattern survivors, never the .md sibling; envelope:\n{envelope:#}",
        );
    }
}

/// `kind: presence` (the `file` selector) through the binary: a rule
/// requiring an absent `config: { path }` flags the missing file, and
/// the same rule rewritten to require a present path produces no
/// finding. Mirrors the crate-level `file_flags_missing_required_path`.
#[test]
fn presence_file_flags_missing() {
    let fx = scaffold_hint_fixture(&[("docs/reference/present.md", "# Present\n")]);

    write_hint_rule(
        &fx.codex,
        "UNI-201",
        "  - kind: presence\n    value: file\n    config:\n      path: docs/reference/absent.md\n",
    );
    let missing = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        missing.status.code(),
        Some(2),
        "the absent required file must gate the scan; stderr:\n{}",
        String::from_utf8_lossy(&missing.stderr),
    );
    let missing_env = parse_envelope(&missing);
    assert_eq!(
        findings_for(&missing_env, "UNI-201").len(),
        1,
        "exactly one presence finding for the absent path; envelope:\n{missing_env:#}",
    );

    write_hint_rule(
        &fx.codex,
        "UNI-201",
        "  - kind: presence\n    value: file\n    config:\n      path: docs/reference/present.md\n",
    );
    let present = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        present.status.code(),
        Some(0),
        "a satisfied presence requirement must not gate; stderr:\n{}",
        String::from_utf8_lossy(&present.stderr),
    );
    assert!(
        findings_for(&parse_envelope(&present), "UNI-201").is_empty(),
        "a present required path produces no finding",
    );
}

/// `kind: field-grammar` (the `field-first-word` mode) through the
/// binary: a rule requiring each `.md`'s `description` frontmatter to
/// begin with an allow-listed verb flags only the prose-leading
/// description. Mirrors the crate-level `first_word_flags_bad_and_passes_good`.
#[test]
fn field_grammar_flags_first_word() {
    let fx = scaffold_hint_fixture(&[
        ("good.md", "---\ndescription: Build the fixtures.\n---\n\nBody.\n"),
        ("bad.md", "---\ndescription: The thing that runs.\n---\n\nBody.\n"),
    ]);
    write_hint_rule(
        &fx.codex,
        "UNI-202",
        "  - kind: path-pattern\n    value: \"*.md\"\n  - kind: field-grammar\n    value: field-first-word\n    config:\n      field: description\n      allowed:\n        - build\n        - run\n",
    );
    let output = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "the non-verb description must gate; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let envelope = parse_envelope(&output);
    let paths: Vec<&str> =
        findings_for(&envelope, "UNI-202").iter().map(|f| finding_path(f)).collect();
    assert_eq!(
        paths,
        vec!["bad.md"],
        "only the prose-leading description is flagged; the allowed-verb one passes; envelope:\n{envelope:#}",
    );
}

/// `kind: set-coverage` (the `skill-allowed-tools` source) through the
/// binary: an `allowed-tools` frontmatter entry outside the rule's
/// `config: { allowed }` set is flagged. The source reads the
/// frontmatter fact family, so it fires under the project profile
/// without needing the framework-only skill facts. Mirrors the
/// crate-level `flags_only_uncovered_tools`.
#[test]
fn set_coverage_flags_uncovered_tool() {
    let fx = scaffold_hint_fixture(&[(
        "skillish.md",
        "---\nallowed-tools: Read NotATool\n---\n\nBody.\n",
    )]);
    write_hint_rule(
        &fx.codex,
        "UNI-203",
        "  - kind: path-pattern\n    value: \"*.md\"\n  - kind: set-coverage\n    value: skill-allowed-tools\n    config:\n      allowed:\n        - Read\n",
    );
    let output = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "the uncovered tool must gate; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let envelope = parse_envelope(&output);
    let findings = findings_for(&envelope, "UNI-203");
    assert_eq!(findings.len(), 1, "only the unrecognised tool is flagged; envelope:\n{envelope:#}");
    assert_eq!(
        findings[0].pointer("/evidence/data/tool").and_then(Value::as_str),
        Some("NotATool"),
        "the finding must name the uncovered tool; envelope:\n{envelope:#}",
    );
}

/// `kind: cardinality` (the `markdown-h2-section-body-line-count`
/// metric) through the binary: an over-cap level-2 section is flagged
/// while a section under the cap passes. The metric reads the
/// markdown-section fact family, available under the project profile.
/// Mirrors the crate-level `flags_h2_sections_over_cap`.
#[test]
fn cardinality_flags_long_section() {
    let fx = scaffold_hint_fixture(&[("doc.md", "## Big\nl1\nl2\nl3\nl4\n\n## Small\nl1\n")]);
    write_hint_rule(
        &fx.codex,
        "UNI-204",
        "  - kind: path-pattern\n    value: \"*.md\"\n  - kind: cardinality\n    value: markdown-h2-section-body-line-count\n    config:\n      max: 3\n",
    );
    let output = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "the over-cap section must gate; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let envelope = parse_envelope(&output);
    let titles: Vec<&str> = findings_for(&envelope, "UNI-204")
        .iter()
        .filter_map(|f| f.pointer("/evidence/data/title").and_then(Value::as_str))
        .collect();
    assert_eq!(
        titles,
        vec!["Big"],
        "only the over-cap H2 section is flagged; envelope:\n{envelope:#}",
    );
}

/// `kind: reference-resolves` (the `markdown-link` source) through the
/// binary: a relative link to a missing file is flagged while a link to
/// a real sibling resolves. Mirrors the crate-level
/// `flags_only_unresolved_markdown_links`.
#[test]
fn reference_resolves_flags_broken_link() {
    let fx = scaffold_hint_fixture(&[
        ("docs/there.md", "# there\n"),
        ("docs/a.md", "[ok](./there.md) and [bad](./missing.md)\n"),
    ]);
    write_hint_rule(
        &fx.codex,
        "UNI-205",
        "  - kind: path-pattern\n    value: \"docs/**/*.md\"\n  - kind: reference-resolves\n    value: markdown-link\n",
    );
    let output = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "the broken link must gate; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let envelope = parse_envelope(&output);
    let targets: Vec<&str> = findings_for(&envelope, "UNI-205")
        .iter()
        .filter_map(|f| f.pointer("/evidence/value").and_then(Value::as_str))
        .collect();
    assert_eq!(
        targets,
        vec!["./missing.md"],
        "only the unresolved target is flagged; the real sibling resolves; envelope:\n{envelope:#}",
    );
}

/// `kind: fenced-block` (the `inline-json-too-long` source) through the
/// binary: a json fence whose body exceeds `config: { max-lines }` is
/// flagged while a short json fence passes. The fenced-block fact
/// family is built under the project profile. Mirrors the crate-level
/// `flags_long_json_fences_only`.
#[test]
fn fenced_block_flags_long_json() {
    let fx = scaffold_hint_fixture(&[(
        "doc.md",
        "# Doc\n\n```json\n1\n2\n3\n4\n5\n```\n\n```json\nx\n```\n",
    )]);
    write_hint_rule(
        &fx.codex,
        "UNI-206",
        "  - kind: path-pattern\n    value: \"*.md\"\n  - kind: fenced-block\n    value: inline-json-too-long\n    config:\n      langs:\n        - json\n      max-lines: 3\n",
    );
    let output = run_review(&fx.project, Some(&fx.codex), &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "the over-long json fence must gate; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let envelope = parse_envelope(&output);
    assert_eq!(
        findings_for(&envelope, "UNI-206").len(),
        1,
        "only the 5-line json fence is flagged, not the short one; envelope:\n{envelope:#}",
    );
}

/// Blocking-tier exit decision (mixed half): with one `important` rule and
/// one `suggestion` rule both matching, the scan exits `2` driven by the
/// blocking tier — not by raw finding count. Proves the exit is severity-
/// gated, complementing `suggestion_finding_present_exits_0`.
#[test]
fn blocking_tier_drives_exit() {
    let fx = build_fixture();
    // `build_fixture` already wrote the `important` UNI-100 (matches
    // `TODO`). Add a `suggestion` rule matching `scaffolding`, also
    // present in `notes.md` ("TODO: drop scaffolding.").
    write_regex_rule(&fx.codex, "UNI-101", "suggestion", "scaffolding");
    let output = run_review(&fx.project, Some(&fx.codex), &[]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "the important finding must drive exit 2 despite a co-present suggestion; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    let envelope: Value = serde_json::from_str(stdout).expect("parse envelope");
    let important = envelope
        .pointer("/summary/important")
        .and_then(Value::as_u64)
        .expect("summary.important present");
    let suggestion = envelope
        .pointer("/summary/suggestion")
        .and_then(Value::as_u64)
        .expect("summary.suggestion present");
    assert!(
        important >= 1 && suggestion >= 1,
        "both tiers must surface (exit driven by the blocking tier, not count); envelope:\n{envelope:#}"
    );
}
