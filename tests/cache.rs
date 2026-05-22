//! Integration tests for the RFC-27 §D8 cache surface.
//!
//! Covers the `specify source cache {lookup, write}` verbs and the
//! `specify source resolve --explain` fingerprint-chain reader, plus
//! the matching `slice.extract.cache-{hit,miss}` journal events.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

mod common;
use common::{Project, parse_stdout, specify};

fn stage_source(project: &Project, name: &str, manifest: &str) {
    let adapter_dir = project.root().join("sources").join(name);
    let briefs_dir = adapter_dir.join("briefs");
    fs::create_dir_all(&briefs_dir).expect("mkdir source fixture");
    fs::write(adapter_dir.join("adapter.yaml"), manifest).expect("write adapter.yaml");
    fs::write(
        briefs_dir.join("enumerate.md"),
        "---\nid: enumerate\ndescription: enumerate brief\n---\n\nbody\n",
    )
    .expect("write enumerate brief");
    fs::write(
        briefs_dir.join("extract.md"),
        "---\nid: extract\ndescription: extract brief\n---\n\nbody\n",
    )
    .expect("write extract brief");
}

fn write_payload(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, body).expect("write payload");
    path
}

fn read_index_lines(project_root: &Path, adapter: &str) -> Vec<String> {
    let path = project_root
        .join(".specify")
        .join(".cache")
        .join("sources")
        .join(adapter)
        .join("index.jsonl");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read index.jsonl at {}: {err}", path.display()));
    raw.lines().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect()
}

fn read_journal_lines(project_root: &Path) -> Vec<Value> {
    let path = project_root.join(".specify/journal.jsonl");
    let raw = fs::read_to_string(&path).expect("read journal.jsonl");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("journal line is JSON"))
        .collect()
}

const V1_MANIFEST: &str = "name: code-typescript\nversion: 1\naxis: source\noperations: [enumerate, extract]\nbriefs:\n  enumerate: briefs/enumerate.md\n  extract: briefs/extract.md\n";

const V2_MANIFEST: &str = "name: code-typescript\nversion: 2\naxis: source\noperations: [enumerate, extract]\nbriefs:\n  enumerate: briefs/enumerate.md\n  extract: briefs/extract.md\n";

const OPT_OUT_MANIFEST: &str = "name: code-typescript\nversion: 1\naxis: source\noperations: [enumerate, extract]\nbriefs:\n  enumerate: briefs/enumerate.md\n  extract: briefs/extract.md\ncache: opt-out\n";

fn run_lookup(project: &Project, source_dir: &Path) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_specify"))
        .current_dir(project.root())
        .args(["--format", "json", "source", "cache", "lookup", "code-typescript"])
        .arg("--project-dir")
        .arg(project.root())
        .args([
            "--slice",
            "identity",
            "--source-key",
            "legacy",
            "--operation",
            "extract",
            "--candidate",
            "user-registration",
        ])
        .arg("--source-path")
        .arg(source_dir)
        .output()
        .expect("spawn specify cache lookup");
    assert!(
        output.status.success(),
        "lookup failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_stdout(&output.stdout, project.root())
}

fn run_write(project: &Project, source_dir: &Path, payload: &Path) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_specify"))
        .current_dir(project.root())
        .args(["--format", "json", "source", "cache", "write", "code-typescript"])
        .arg("--project-dir")
        .arg(project.root())
        .args([
            "--slice",
            "identity",
            "--source-key",
            "legacy",
            "--operation",
            "extract",
            "--candidate",
            "user-registration",
        ])
        .arg("--source-path")
        .arg(source_dir)
        .arg("--payload")
        .arg(payload)
        .output()
        .expect("spawn specify cache write");
    assert!(
        output.status.success(),
        "write failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_stdout(&output.stdout, project.root())
}

#[test]
fn extract_miss_then_hit_with_unchanged_inputs() {
    let project = Project::init();
    stage_source(&project, "code-typescript", V1_MANIFEST);
    let source_dir = project.root().join("vendor/legacy");
    fs::create_dir_all(&source_dir).expect("mkdir source");
    let payload = write_payload(project.root(), "evidence.yaml", "---\nclaims: []\n");

    // First lookup is a cold miss with reason no-prior-entry.
    let cold = run_lookup(&project, &source_dir);
    assert_eq!(cold["status"], "miss");
    assert_eq!(cold["reason"], "no-prior-entry");

    // Run the operation and write the cache entry.
    let written = run_write(&project, &source_dir, &payload);
    assert_eq!(written["opted-out"], false);
    assert_eq!(written["fingerprint"], cold["fingerprint"]);

    // Second lookup with identical inputs is a hit.
    let warm = run_lookup(&project, &source_dir);
    assert_eq!(warm["status"], "hit");
    assert!(warm.get("reason").is_none(), "hit must elide reason, got {warm}");

    // Index has exactly one row per cache write.
    let index = read_index_lines(project.root(), "code-typescript");
    assert_eq!(index.len(), 1, "one row per cache write, got {index:?}");
    let row: Value = serde_json::from_str(&index[0]).expect("index row JSON");
    assert_eq!(row["adapter"], "code-typescript");
    assert_eq!(row["operation"], "extract");
    assert_eq!(row["fingerprint"], cold["fingerprint"]);

    // Journal carries one miss followed by one hit.
    let journal = read_journal_lines(project.root());
    let cache_events: Vec<&Value> = journal
        .iter()
        .filter(|e| {
            e["event"]
                .as_str()
                .is_some_and(|n| n == "slice.extract.cache-hit" || n == "slice.extract.cache-miss")
        })
        .collect();
    assert_eq!(cache_events.len(), 2, "miss + hit expected, got: {cache_events:?}");
    assert_eq!(cache_events[0]["event"], "slice.extract.cache-miss");
    assert_eq!(cache_events[0]["payload"]["reason"], "no-prior-entry");
    assert_eq!(cache_events[1]["event"], "slice.extract.cache-hit");
}

#[test]
fn adapter_version_bump_misses_with_changed_reason() {
    let project = Project::init();
    stage_source(&project, "code-typescript", V1_MANIFEST);
    let source_dir = project.root().join("vendor/legacy");
    fs::create_dir_all(&source_dir).expect("mkdir source");
    let payload = write_payload(project.root(), "evidence.yaml", "---\nclaims: []\n");

    let cold = run_lookup(&project, &source_dir);
    assert_eq!(cold["status"], "miss");
    let _written = run_write(&project, &source_dir, &payload);
    let warm = run_lookup(&project, &source_dir);
    assert_eq!(warm["status"], "hit");

    // Bump the adapter version → fingerprint changes.
    stage_source(&project, "code-typescript", V2_MANIFEST);
    let post_bump = run_lookup(&project, &source_dir);
    assert_eq!(post_bump["status"], "miss");
    assert_eq!(post_bump["reason"], "adapter-version-changed");
    assert_ne!(post_bump["fingerprint"], cold["fingerprint"]);

    // Index still has one row per write (no new writes since the bump).
    let index = read_index_lines(project.root(), "code-typescript");
    assert_eq!(index.len(), 1);
}

#[test]
fn opt_out_misses_with_adapter_opt_out_reason() {
    let project = Project::init();
    stage_source(&project, "code-typescript", OPT_OUT_MANIFEST);
    let source_dir = project.root().join("vendor/legacy");
    fs::create_dir_all(&source_dir).expect("mkdir source");
    let payload = write_payload(project.root(), "evidence.yaml", "---\nclaims: []\n");

    let first = run_lookup(&project, &source_dir);
    assert_eq!(first["status"], "miss");
    assert_eq!(first["reason"], "adapter-opt-out");

    let written = run_write(&project, &source_dir, &payload);
    assert_eq!(written["opted-out"], true);

    // No cache body was written, but the index row was appended.
    let fp = written["fingerprint"].as_str().expect("fingerprint string");
    let bare_digest = fp.strip_prefix("sha256:").expect("sha256 prefix");
    let cache_dir = project
        .root()
        .join(".specify")
        .join(".cache")
        .join("sources")
        .join("code-typescript")
        .join(bare_digest);
    assert!(
        !cache_dir.exists(),
        "opt-out must not write cache directory at {}",
        cache_dir.display()
    );
    let index = read_index_lines(project.root(), "code-typescript");
    assert_eq!(index.len(), 1, "opt-out still appends one index row per write");

    // Repeated lookup still misses with the same reason.
    let second = run_lookup(&project, &source_dir);
    assert_eq!(second["status"], "miss");
    assert_eq!(second["reason"], "adapter-opt-out");
}

#[test]
fn source_resolve_explain_prints_fingerprint_chain() {
    let project = Project::init();
    stage_source(&project, "code-typescript", V1_MANIFEST);
    let source_dir = project.root().join("vendor/legacy");
    fs::create_dir_all(&source_dir).expect("mkdir source");
    let payload = write_payload(project.root(), "evidence.yaml", "---\nclaims: []\n");

    // Cold extract → cache miss + cache write.
    let _miss = run_lookup(&project, &source_dir);
    let _written = run_write(&project, &source_dir, &payload);

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "source", "resolve", "code-typescript", "--explain"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();
    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["adapter"], "code-typescript");
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["operation"], "extract");
    assert_eq!(entries[0]["slice"], "identity");
    assert_eq!(entries[0]["source-key"], "legacy");
    assert!(
        entries[0]["fingerprint"].as_str().expect("fp str").starts_with("sha256:"),
        "fingerprint must be sha256:-prefixed: {entries:?}"
    );

    // Text mode renders the same chain for human consumption.
    let assert_text = specify()
        .current_dir(project.root())
        .args(["source", "resolve", "code-typescript", "--explain"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert_text.get_output().stdout);
    assert!(stdout.contains("adapter: code-typescript"), "text body:\n{stdout}");
    assert!(stdout.contains("index:"), "text body:\n{stdout}");
    assert!(stdout.contains("extract identity/legacy"), "text body:\n{stdout}");
}
