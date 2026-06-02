//! CLI acceptance tests for `specrun tool`.

use std::fmt::Write as _;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tempfile::{TempDir, tempdir};

mod common;
use common::{parse_json, repo_root, sha256_hex, specrun};

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
        for name in ["tools-test-project", "tools-test-adp", "tools-test-project-adp"] {
            copy_dir(&fixtures_root().join(name), &root.join(name));
        }
        for path in [
            root.join("tools-test-project/.specify/project.yaml"),
            root.join("tools-test-adp/tools.yaml"),
            root.join("tools-test-project-adp/.specify/project.yaml"),
        ] {
            let text = fs::read_to_string(&path).expect("read fixture manifest");
            fs::write(&path, text.replace("/__SPECIFY_FIXTURE_ROOT__", &root.to_string_lossy()))
                .expect("write materialized manifest");
        }
        copy_dir(
            &root.join("tools-test-adp"),
            &root.join("tools-test-project-adp/adapters/targets/tools-test-adp"),
        );
        Self { _tmp: tmp, root }
    }

    fn project(&self) -> PathBuf {
        self.root.join("tools-test-project")
    }

    fn cap_project(&self) -> PathBuf {
        self.root.join("tools-test-project-adp")
    }

    fn adapter(&self) -> PathBuf {
        self.cap_project().join("adapters").join("targets").join("tools-test-adp")
    }

    fn project_wasm(&self, name: &str) -> PathBuf {
        self.project().join("wasm").join(format!("{name}.wasm"))
    }

    fn cap_wasm(&self, name: &str) -> PathBuf {
        self.adapter().join("wasm").join(format!("{name}.wasm"))
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
    format!("name: tools-test\nworkspace: true\ntools:\n{tools}")
}

fn adapter_project_manifest(tools: Option<&str>) -> String {
    let mut yaml = "name: tools-test-project-adp\nadapter: tools-test-adp\nrules: {}\n".to_string();
    if let Some(tools) = tools {
        yaml.push_str("tools:\n");
        yaml.push_str(tools);
    }
    yaml
}

fn write_project_manifest(project: &Path, yaml: &str) {
    fs::write(project.join(".specify/project.yaml"), yaml).expect("write project.yaml");
}

fn write_adapter_tools(adapter: &Path, tools: &str) {
    fs::write(adapter.join("tools.yaml"), format!("tools:\n{tools}")).expect("write tools.yaml");
}

fn run_json_failure(project: &Path, cache: &Path, args: &[&str], code: i32) -> Value {
    let assert = specrun()
        .current_dir(project)
        .env("SPECIFY_TOOLS_CACHE", cache)
        .args(["--format", "json"])
        .args(args)
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(code));
    parse_json(&assert.get_output().stderr)
}

fn assert_validation_rule(value: &Value, rule_id: &str) {
    // Tool-manifest validation failures surface as a payload-free
    // `Error::Validation` keyed on the first failing rule id; the wire
    // `error` discriminant is that rule id directly.
    assert_eq!(value["error"], rule_id, "{value}");
    assert_eq!(value["exit-code"], 2, "{value}");
}

#[test]
fn help_lists_active_verbs() {
    let assert = specrun().args(["tool", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    for verb in ["run", "fetch", "gc", "schema"] {
        assert!(stdout.contains(verb), "tool --help must list `{verb}`, got:\n{stdout}");
    }
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
        let value = run_json_failure(&project, &cache, &["tool", "run", "echo"], 2);
        assert_validation_rule(&value, rule_id);
    }
}

#[test]
fn adapter_manifest_reports_sidecar_rules() {
    let fixtures = ToolFixtures::new();
    let cache = cache_dir("adapter-validation");
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
        write_adapter_tools(&fixtures.adapter(), &entry);
        let value =
            run_json_failure(&fixtures.cap_project(), &cache, &["tool", "run", "exit-seven"], 2);
        assert_validation_rule(&value, rule_id);
        let cap_yaml =
            fs::read_to_string(fixtures.adapter().join("adapter.yaml")).expect("read adapter.yaml");
        assert!(!cap_yaml.contains("\ntools:"), "sidecar mutation must not touch adapter.yaml");
    }
}

#[test]
fn cache_miss_hit_and_override_observable() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("cache-flow");

    let first = specrun()
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

    specrun()
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

    let pinned = tool_entry(
        "echo",
        "0.1.0",
        &file_uri(&fixtures.project_wasm("echo")),
        Some(&sha256_hex(&fixtures.project_wasm("echo"))),
        None,
    );
    write_project_manifest(&project, &project_manifest(&pinned));

    specrun()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "fetch", "echo"])
        .assert()
        .success();
    let refetched = fs::read_to_string(&sidecar).expect("read refetched sidecar");
    assert!(refetched.contains("sha256:"), "{refetched}");
}

#[test]
fn digest_mismatch_fails_before_install() {
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

    let assert = specrun()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "echo", "--", "local"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("echo: local"), "{stdout}");
}

#[test]
fn https_network_failure_is_typed() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("network-failure");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let url = format!("https://{}/echo.wasm", listener.local_addr().expect("addr"));
    drop(listener);
    let entry = tool_entry("echo", "0.1.0", &url, None, None);
    write_project_manifest(&project, &project_manifest(&entry));

    let value = run_json_failure(&project, &cache, &["tool", "run", "echo"], 1);
    let code = value["error"].as_str().expect("error code");
    assert!(
        matches!(code, "tool-network-other" | "tool-network-timeout" | "tool-network-malformed"),
        "{value}"
    );
    assert!(value["message"].as_str().expect("message").contains(&url), "{value}");
}

#[test]
fn invalid_wasm_runtime_failure_is_typed() {
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
fn allowed_fs_reads_and_writes_declared() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.project();
    let cache = cache_dir("allowed-fs");

    let read = specrun()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "read-only"])
        .assert()
        .success();
    let stdout = String::from_utf8(read.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("read-only: fixture-probe"), "{stdout}");

    let result = project.join("outputs/result.txt");
    drop(fs::remove_file(&result));
    let write = specrun()
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
fn denied_fs_and_lifecycle_fail_before_guest() {
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
    assert_validation_rule(&value, "tool-permission-denied");

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
fn adapter_non_zero_exit_caches_by_scope() {
    let fixtures = ToolFixtures::new();
    let cache = cache_dir("exit-seven");
    let assert = specrun()
        .current_dir(fixtures.cap_project())
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "exit-seven"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(7));
    assert!(cache.join("adapter--target--tools-test-adp/exit-seven/0.1.0/module.wasm").is_file());
}

#[test]
fn name_collision_project_scope_wins() {
    let fixtures = ToolFixtures::new();
    let project = fixtures.cap_project();
    let cache = cache_dir("collision");
    let project_echo =
        tool_entry("echo", "0.1.0", &file_uri(&fixtures.project_wasm("echo")), None, None);
    let cap_echo =
        tool_entry("echo", "0.1.0", &file_uri(&fixtures.cap_wasm("exit-seven")), None, None);
    let cap_exit =
        tool_entry("exit-seven", "0.1.0", &file_uri(&fixtures.cap_wasm("exit-seven")), None, None);
    write_project_manifest(&project, &adapter_project_manifest(Some(&project_echo)));
    write_adapter_tools(&fixtures.adapter(), &format!("{cap_echo}{cap_exit}"));

    let run = specrun()
        .current_dir(&project)
        .env("SPECIFY_TOOLS_CACHE", &cache)
        .args(["tool", "run", "echo", "--", "project-wins"])
        .assert()
        .success();
    let stdout = String::from_utf8(run.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("echo: project-wins"), "{stdout}");
    let stderr = String::from_utf8(run.get_output().stderr.clone()).expect("utf8 stderr");
    assert!(stderr.contains("tool-name-collision"), "{stderr}");
}
