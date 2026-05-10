//! CLI acceptance tests for `specify tool`.

use std::fmt::Write as _;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};

mod common;
use common::{parse_json, repo_root, specify};

fn fixtures_root() -> PathBuf {
    repo_root().join("tests").join("fixtures")
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst");
    for entry in fs::read_dir(src).expect("read src") {
        let entry = entry.expect("dir entry");
        if entry.file_name() == "target" {
            continue;
        }
        let kind = entry.file_type().expect("file type");
        let target = dst.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target);
        } else if kind.is_file() {
            fs::copy(entry.path(), target).expect("copy file");
        }
    }
}

struct ToolFixtures {
    _tmp: TempDir,
    root: PathBuf,
}

impl ToolFixtures {
    fn new() -> Self {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for name in ["tools-test-project", "tools-test-cap", "tools-test-project-cap"] {
            copy_dir(&fixtures_root().join(name), &root.join(name));
        }
        for path in [
            root.join("tools-test-project/.specify/project.yaml"),
            root.join("tools-test-cap/tools.yaml"),
            root.join("tools-test-project-cap/.specify/project.yaml"),
        ] {
            let text = fs::read_to_string(&path).expect("read fixture manifest");
            fs::write(&path, text.replace("/__SPECIFY_FIXTURE_ROOT__", &root.to_string_lossy()))
                .expect("write materialized manifest");
        }
        copy_dir(
            &root.join("tools-test-cap"),
            &root.join("tools-test-project-cap/schemas/tools-test-cap"),
        );
        Self { _tmp: tmp, root }
    }

    fn project(&self) -> PathBuf {
        self.root.join("tools-test-project")
    }

    fn cap_project(&self) -> PathBuf {
        self.root.join("tools-test-project-cap")
    }

    fn capability(&self) -> PathBuf {
        self.cap_project().join("schemas/tools-test-cap")
    }

    fn project_wasm(&self, name: &str) -> PathBuf {
        self.project().join("wasm").join(format!("{name}.wasm"))
    }

    fn cap_wasm(&self, name: &str) -> PathBuf {
        self.capability().join("wasm").join(format!("{name}.wasm"))
    }
}

fn cache_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir()
        .join(format!("specify-tool-{label}-{}-{nanos}-{n}", std::process::id()));
    fs::create_dir_all(&path).expect("create cache dir");
    path
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn sha256_hex(path: &Path) -> String {
    let bytes = fs::read(path).expect("read bytes for sha256");
    format!("{:x}", Sha256::digest(bytes))
}

fn tool_entry(
    name: &str, version: &str, source: &str, sha256: Option<&str>, permissions: Option<&str>,
) -> String {
    let mut entry = format!("  - name: {name}\n    version: {version}\n    source: \"{source}\"\n");
    if let Some(sha256) = sha256 {
        writeln!(entry, "    sha256: {sha256}").expect("write sha256 YAML line");
    }
    if let Some(permissions) = permissions {
        entry.push_str(permissions);
    }
    entry
}

fn project_manifest(tools: &str) -> String {
    format!("name: tools-test\nhub: true\ntools:\n{tools}")
}

fn capability_project_manifest(tools: Option<&str>) -> String {
    let mut yaml =
        "name: tools-test-project-cap\ncapability: tools-test-cap\nrules: {}\n".to_string();
    if let Some(tools) = tools {
        yaml.push_str("tools:\n");
        yaml.push_str(tools);
    }
    yaml
}

fn write_project_manifest(project: &Path, yaml: &str) {
    fs::write(project.join(".specify/project.yaml"), yaml).expect("write project.yaml");
}

fn write_capability_tools(capability: &Path, tools: &str) {
    fs::write(capability.join("tools.yaml"), format!("tools:\n{tools}")).expect("write tools.yaml");
}

fn json_tool_list(project: &Path, cache: &Path) -> Value {
    let assert = specify()
        .current_dir(project)
        .env("SPECIFY_TOOLS_CACHE", cache)
        .args(["--format", "json", "tool", "list"])
        .assert()
        .success();
    parse_json(&assert.get_output().stdout)
}

fn run_json_failure(project: &Path, cache: &Path, args: &[&str], code: i32) -> Value {
    let assert = specify()
        .current_dir(project)
        .env("SPECIFY_TOOLS_CACHE", cache)
        .args(["--format", "json"])
        .args(args)
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(code));
    parse_json(&assert.get_output().stdout)
}

fn assert_validation_rule(value: &Value, rule_id: &str) {
    assert_eq!(value["error"], "validation", "{value}");
    let results = value["results"].as_array().expect("validation results array");
    assert!(
        results.iter().any(|result| result["rule-id"] == rule_id),
        "expected rule-id `{rule_id}` in {value}"
    );
}

#[test]
fn tool_help_lists_chunk_five_verbs() {
    let assert = specify().args(["tool", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    for verb in ["run", "list", "fetch", "show", "gc"] {
        assert!(stdout.contains(verb), "tool --help must list `{verb}`, got:\n{stdout}");
    }
}

#[test]
fn tool_list_outside_project_preserves_not_initialized_error() {
    let tmp = tempdir().expect("tempdir");
    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "tool", "list"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["schema-version"], 4);
    assert_eq!(value["error"], "not-initialized");
}

#[test]
fn tool_list_discovers_project_root_from_nested_subdirectory() {
    let fixtures = ToolFixtures::new();
    let nested = fixtures.project().join("nested/deeper");
    fs::create_dir_all(&nested).expect("create nested project dir");

    let value = json_tool_list(&nested, &cache_dir("nested-list"));
    let tools = value["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["echo", "read-only", "read-write"]);
    assert!(tools.iter().all(|tool| tool["scope-detail"] == "tools-test"));
}

#[test]
fn project_scope_fixture_lists_three_hub_project_tools() {
    let fixtures = ToolFixtures::new();
    let value = json_tool_list(&fixtures.project(), &cache_dir("project-list"));
    let tools = value["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["echo", "read-only", "read-write"]);
    assert!(tools.iter().all(|tool| tool["scope"] == "project"));
    assert!(tools.iter().all(|tool| tool["scope-detail"] == "tools-test"));
    assert!(tools.iter().all(|tool| tool["cache-status"] == "miss-not-found"));
}

#[test]
fn capability_scope_fixture_lists_sidecar_tool_and_keeps_capability_closed() {
    let fixtures = ToolFixtures::new();
    let cap_yaml = fs::read_to_string(fixtures.capability().join("capability.yaml"))
        .expect("read capability.yaml");
    assert!(!cap_yaml.contains("\ntools:"), "capability.yaml must stay closed");

    specify()
        .args(["--format", "json", "capability", "check"])
        .arg(fixtures.capability())
        .assert()
        .success();

    let value = json_tool_list(&fixtures.cap_project(), &cache_dir("capability-list"));
    let tools = value["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "{value}");
    assert_eq!(tools[0]["name"], "exit-seven");
    assert_eq!(tools[0]["scope"], "capability");
    assert_eq!(tools[0]["scope-detail"], "tools-test-cap");
}

#[test]
fn project_manifest_validation_reports_rule_ids() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("project-validation");
    let source = file_uri(&fixtures.project_wasm("echo"));
    let cases = [
        (tool_entry("BadName", "0.1.0", &source, None, None), "tool.name-format"),
        (tool_entry("echo", "not-semver", &source, None, None), "tool.version-is-semver"),
        (tool_entry("echo", "0.1.0", "https://", None, None), "tool.source-is-supported-uri"),
        (tool_entry("echo", "0.1.0", &source, Some("ABC"), None), "tool.sha256-format"),
        (
            tool_entry(
                "echo",
                "0.1.0",
                &source,
                None,
                Some("    permissions:\n      read:\n        - \"$PROJECT_DIR/../inputs\"\n"),
            ),
            "tool.permission-path-form",
        ),
        (
            tool_entry(
                "echo",
                "0.1.0",
                &source,
                None,
                Some("    permissions:\n      write:\n        - \"$PROJECT_DIR/.specify\"\n"),
            ),
            "tool.lifecycle-state-write-denied",
        ),
        (
            tool_entry(
                "echo",
                "0.1.0",
                &source,
                None,
                Some("    permissions:\n      read:\n        - \"$CAPABILITY_DIR/templates\"\n"),
            ),
            "tool.capability-dir-out-of-scope",
        ),
    ];

    for (entry, rule_id) in cases {
        write_project_manifest(&project, &project_manifest(&entry));
        let value = run_json_failure(&project, &cache, &["tool", "list"], 2);
        assert_validation_rule(&value, rule_id);
    }
}

#[test]
fn capability_manifest_validation_reports_sidecar_rule_ids() {
    let fixtures = ToolFixtures::new();
    let cache = cache_dir("capability-validation");
    let cases = [
        (tool_entry("exit-seven", "0.1.0", "https://", None, None), "tool.source-is-supported-uri"),
        (
            tool_entry(
                "exit-seven",
                "0.1.0",
                &file_uri(&fixtures.cap_wasm("exit-seven")),
                None,
                Some("    permissions:\n      read:\n        - \"$PROJECT_DIR/../inputs\"\n"),
            ),
            "tool.permission-path-form",
        ),
    ];

    for (entry, rule_id) in cases {
        write_capability_tools(&fixtures.capability(), &entry);
        let value = run_json_failure(&fixtures.cap_project(), &cache, &["tool", "list"], 2);
        assert_validation_rule(&value, rule_id);
        let cap_yaml = fs::read_to_string(fixtures.capability().join("capability.yaml"))
            .expect("read capability.yaml");
        assert!(!cap_yaml.contains("\ntools:"), "sidecar mutation must not touch capability.yaml");
    }
}

#[test]
fn cache_miss_hit_change_and_override_are_observable() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("cache-flow");

    let cold = json_tool_list(&project, &cache);
    assert_eq!(cold["tools"][0]["cache-status"], "miss-not-found");

    let first = specify()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "echo", "--", "hello", "world"])
        .assert()
        .success();
    let stdout = String::from_utf8(first.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("echo: hello world"), "{stdout}");
    assert!(stdout.contains("CAPABILITY_DIR=<unset>"), "{stdout}");
    assert!(stdout.contains("PATH=<unset>"), "{stdout}");

    let sidecar = cache.join("project--tools-test/echo/0.1.0/meta.yaml");
    assert!(sidecar.is_file(), "SPECIFY_TOOLS_CACHE override should receive cache writes");
    let fetched_at = fs::read_to_string(&sidecar).expect("read sidecar");

    specify()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "echo", "--", "hello", "world"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(&sidecar).expect("read sidecar after hit"),
        fetched_at,
        "cache hit must not rewrite fetched-at"
    );

    let hit = json_tool_list(&project, &cache);
    assert_eq!(hit["tools"][0]["cache-status"], "hit");

    let pinned = tool_entry(
        "echo",
        "0.1.0",
        &file_uri(&fixtures.project_wasm("echo")),
        Some(&sha256_hex(&fixtures.project_wasm("echo"))),
        None,
    );
    write_project_manifest(&project, &project_manifest(&pinned));
    let changed = json_tool_list(&project, &cache);
    assert_eq!(changed["tools"][0]["cache-status"], "miss-changed");

    specify()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "fetch", "echo"])
        .assert()
        .success();
    let refetched = fs::read_to_string(&sidecar).expect("read refetched sidecar");
    assert!(refetched.contains("sha256:"), "{refetched}");
}

#[test]
fn digest_mismatch_fails_before_installing_cache_entry() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("digest-mismatch");
    let wrong_sha = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let entry = tool_entry(
        "echo",
        "0.1.0",
        &file_uri(&fixtures.project_wasm("echo")),
        Some(wrong_sha),
        None,
    );
    write_project_manifest(&project, &project_manifest(&entry));

    let value = run_json_failure(&project, &cache, &["tool", "run", "echo"], 1);
    assert_eq!(value["error"], "tool-resolver", "{value}");
    assert!(
        !cache.join("project--tools-test/echo/0.1.0/module.wasm").exists(),
        "digest mismatch must not install staged bytes"
    );
}

#[test]
fn local_path_source_runs_without_file_uri() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("local-path");
    let source = fixtures.project_wasm("echo").display().to_string();
    let entry = tool_entry("echo", "0.1.0", &source, None, None);
    write_project_manifest(&project, &project_manifest(&entry));

    let assert = specify()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "echo", "--", "local"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("echo: local"), "{stdout}");
}

#[test]
fn https_network_failure_is_typed_and_exits_one() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("network-failure");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let url = format!("https://{}/echo.wasm", listener.local_addr().expect("addr"));
    drop(listener);
    let entry = tool_entry("echo", "0.1.0", &url, None, None);
    write_project_manifest(&project, &project_manifest(&entry));

    let value = run_json_failure(&project, &cache, &["tool", "run", "echo"], 1);
    assert_eq!(value["error"], "tool-resolver", "{value}");
    assert!(value["message"].as_str().expect("message").contains(&url), "{value}");
}

#[test]
fn invalid_wasm_runtime_failure_is_typed_and_exits_one() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("runtime-failure");
    let invalid_wasm = project.join("not-a-component.wasm");
    fs::write(&invalid_wasm, b"not a wasm component").expect("write invalid wasm");
    let entry = tool_entry("echo", "0.1.0", &file_uri(&invalid_wasm), None, None);
    write_project_manifest(&project, &project_manifest(&entry));

    let value = run_json_failure(&project, &cache, &["tool", "run", "echo"], 1);
    assert_eq!(value["error"], "tool-runtime", "{value}");
}

#[test]
fn allowed_filesystem_access_reads_and_writes_declared_dirs() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("allowed-fs");

    let read = specify()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "read-only"])
        .assert()
        .success();
    let stdout = String::from_utf8(read.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("read-only: fixture-probe"), "{stdout}");

    let result = project.join("outputs/result.txt");
    let _ = fs::remove_file(&result);
    let write = specify()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "read-write"])
        .assert()
        .success();
    let stdout = String::from_utf8(write.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("read-write: derived: fixture-probe"), "{stdout}");
    assert_eq!(fs::read_to_string(result).expect("read output"), "derived: fixture-probe");
}

#[test]
fn denied_filesystem_and_lifecycle_access_fail_before_guest_work() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("denied-fs");
    let denied = tool_entry(
        "read-only",
        "0.1.0",
        &file_uri(&fixtures.project_wasm("read-only")),
        None,
        Some("    permissions:\n      read:\n        - \"/etc\"\n"),
    );
    write_project_manifest(&project, &project_manifest(&denied));

    let value = run_json_failure(&project, &cache, &["tool", "run", "read-only"], 2);
    assert_eq!(value["error"], "tool-permission-denied", "{value}");

    let lifecycle = tool_entry(
        "read-write",
        "0.1.0",
        &file_uri(&fixtures.project_wasm("read-write")),
        None,
        Some("    permissions:\n      write:\n        - \"$PROJECT_DIR/.specify\"\n"),
    );
    write_project_manifest(&project, &project_manifest(&lifecycle));
    let value = run_json_failure(&project, &cache, &["tool", "run", "read-write"], 2);
    assert_validation_rule(&value, "tool.lifecycle-state-write-denied");
}

#[test]
fn capability_tool_non_zero_exit_propagates_and_caches_by_capability_scope() {
    let fixtures = ToolFixtures::new();
    let cache = cache_dir("exit-seven");
    let assert = specify()
        .current_dir(fixtures.cap_project())
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "exit-seven"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(7));
    assert!(cache.join("capability--tools-test-cap/exit-seven/0.1.0/module.wasm").is_file());
}

#[test]
fn tool_name_collision_warns_and_project_scope_wins() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.cap_project();
    let cache = cache_dir("collision");
    let project_echo =
        tool_entry("echo", "0.1.0", &file_uri(&fixtures.project_wasm("echo")), None, None);
    let cap_echo =
        tool_entry("echo", "0.1.0", &file_uri(&fixtures.cap_wasm("exit-seven")), None, None);
    let cap_exit =
        tool_entry("exit-seven", "0.1.0", &file_uri(&fixtures.cap_wasm("exit-seven")), None, None);
    write_project_manifest(&project, &capability_project_manifest(Some(&project_echo)));
    write_capability_tools(&fixtures.capability(), &(cap_echo + &cap_exit));

    let value = json_tool_list(&project, &cache);
    let warnings = value["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 1, "{value}");
    assert_eq!(warnings[0]["code"], "tool-name-collision");
    let echo = value["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "echo")
        .expect("echo row");
    assert_eq!(echo["scope"], "project");

    let run = specify()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "echo", "--", "project-wins"])
        .assert()
        .success();
    let stdout = String::from_utf8(run.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("echo: project-wins"), "{stdout}");
}
