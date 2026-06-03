//! Unit tests for the `vectis verify` engine.

use serde_json::Value;
use tempfile::tempdir;

use super::*;

fn write_project_yaml(root: &Path, platforms: &[&str]) {
    let yaml_platforms: Vec<String> = platforms.iter().map(|p| format!("  - {p}")).collect();
    let content = format!(
        "name: test-app\nadapter: vectis\nspecify_version: '2.0'\nplatforms:\n{}",
        yaml_platforms.join("\n"),
    );
    std::fs::write(root.join("project.yaml"), content).expect("write project.yaml");
}

fn scaffold_core(root: &Path) {
    let dir = root.join("shared/src");
    std::fs::create_dir_all(&dir).expect("mkdir shared/src");
    std::fs::write(dir.join("app.rs"), "pub struct App;").expect("write app.rs");
}

fn scaffold_ios(root: &Path) {
    let dir = root.join("iOS/TestApp");
    std::fs::create_dir_all(&dir).expect("mkdir iOS/TestApp");
    std::fs::write(dir.join("ContentView.swift"), "struct ContentView {}").expect("write swift");
}

fn scaffold_android(root: &Path) {
    let dir = root.join("Android/app/src/main/kotlin/com/test");
    std::fs::create_dir_all(&dir).expect("mkdir Android");
    std::fs::write(dir.join("MainActivity.kt"), "class MainActivity").expect("write kt");
}

// ── detect mode ────────────────────────────────────────────────────

#[test]
fn detect_all_present_returns_empty_missing() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
    scaffold_core(tmp.path());
    scaffold_ios(tmp.path());
    scaffold_android(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("detect should succeed");
    let missing = result["missing"].as_array().expect("missing array");

    assert!(missing.is_empty(), "expected empty missing set: {result}");
}

#[test]
fn detect_missing_ios_returns_ios_in_missing() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
    scaffold_core(tmp.path());
    scaffold_android(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("detect should succeed");
    let missing = result["missing"].as_array().expect("missing array");

    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0], "ios");
}

#[test]
fn detect_greenfield_returns_all_supported_missing() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("detect should succeed");
    let missing = result["missing"].as_array().expect("missing array");

    assert_eq!(missing.len(), 3);
    let names: Vec<&str> = missing.iter().filter_map(Value::as_str).collect();
    assert!(names.contains(&"core"));
    assert!(names.contains(&"ios"));
    assert!(names.contains(&"android"));
}

#[test]
fn detect_web_desktop_skipped_not_in_missing() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "web", "desktop"]);
    scaffold_core(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("detect should succeed");
    let missing = result["missing"].as_array().expect("missing array");

    assert!(missing.is_empty(), "web/desktop should not appear in missing: {result}");

    let info = result["info"].as_array().expect("info array");
    assert_eq!(info.len(), 2, "expected info findings for web and desktop: {result}");
    assert!(info.iter().all(|f| f["id"] == "platform-not-yet-supported"));
}

#[test]
fn detect_mode_exit_code_always_zero() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios"]);

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("detect should succeed");
    assert_eq!(verify_exit_code(&result), 0);
}

// ── verify mode ────────────────────────────────────────────────────

#[test]
fn verify_all_present_exits_clean() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios", "android"]);
    scaffold_core(tmp.path());
    scaffold_ios(tmp.path());
    scaffold_android(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Verify,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("verify should succeed");
    let findings = result["findings"].as_array().expect("findings array");

    let errors: Vec<&Value> =
        findings.iter().filter(|f| f["severity"] == "error").collect();
    assert!(errors.is_empty(), "expected no error findings: {result}");
    assert_eq!(verify_exit_code(&result), 0);
}

#[test]
fn verify_missing_shell_exits_one() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios"]);
    scaffold_core(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Verify,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("verify should succeed");
    let findings = result["findings"].as_array().expect("findings array");

    let errors: Vec<&Value> =
        findings.iter().filter(|f| f["severity"] == "error").collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["id"], "platform-shell-missing");
    assert!(errors[0]["message"].as_str().unwrap().contains("ios"));
    assert_eq!(verify_exit_code(&result), 1);
}

#[test]
fn verify_web_desktop_emit_info_not_error() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "web", "desktop"]);
    scaffold_core(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Verify,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("verify should succeed");
    let findings = result["findings"].as_array().expect("findings array");

    let errors: Vec<&Value> =
        findings.iter().filter(|f| f["severity"] == "error").collect();
    assert!(errors.is_empty(), "web/desktop should not produce errors: {result}");

    let infos: Vec<&Value> =
        findings.iter().filter(|f| f["severity"] == "info").collect();
    assert_eq!(infos.len(), 2);
    assert!(infos.iter().all(|f| f["id"] == "platform-not-yet-supported"));
    assert_eq!(verify_exit_code(&result), 0);
}

// ── error paths ────────────────────────────────────────────────────

#[test]
fn missing_project_yaml_returns_error() {
    let tmp = tempdir().unwrap();

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let err = run(&args).unwrap_err();
    assert!(matches!(err, VectisError::InvalidProject { .. }));
}

#[test]
fn project_yaml_without_platforms_returns_error() {
    let tmp = tempdir().unwrap();
    std::fs::write(
        tmp.path().join("project.yaml"),
        "name: test-app\nadapter: vectis\n",
    )
    .expect("write project.yaml");

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let err = run(&args).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("platforms"), "error should mention platforms: {msg}");
}

// ── render_json integration ────────────────────────────────────────

#[test]
fn render_json_detect_success_exits_zero() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core"]);
    scaffold_core(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let (json, code) = super::render_json(run(&args));
    assert_eq!(code, 0);
    let value: Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(value["mode"], "detect");
}

#[test]
fn render_json_verify_miss_exits_one() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios"]);
    scaffold_core(tmp.path());

    let args = VerifyArgs {
        mode: VerifyMode::Verify,
        path: Some(tmp.path().to_path_buf()),
    };
    let (json, code) = super::render_json(run(&args));
    assert_eq!(code, 1);
    let value: Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(value["mode"], "verify");
}

#[test]
fn render_json_error_exits_two() {
    let tmp = tempdir().unwrap();

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let (json, code) = super::render_json(run(&args));
    assert_eq!(code, 2);
    let value: Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 2);
}

// ── shell detection edge cases ─────────────────────────────────────

#[test]
fn ios_dir_without_swift_files_is_not_present() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "ios"]);
    scaffold_core(tmp.path());
    let ios_dir = tmp.path().join("iOS");
    std::fs::create_dir_all(&ios_dir).expect("mkdir iOS");
    std::fs::write(ios_dir.join("README.md"), "placeholder").expect("write readme");

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("detect should succeed");
    let missing = result["missing"].as_array().expect("missing array");

    assert!(
        missing.iter().any(|v| v.as_str() == Some("ios")),
        "iOS dir with no .swift files should be missing: {result}"
    );
}

#[test]
fn android_dir_without_kt_files_is_not_present() {
    let tmp = tempdir().unwrap();
    write_project_yaml(tmp.path(), &["core", "android"]);
    scaffold_core(tmp.path());
    let android_dir = tmp.path().join("Android");
    std::fs::create_dir_all(&android_dir).expect("mkdir Android");
    std::fs::write(android_dir.join("build.gradle"), "").expect("write gradle");

    let args = VerifyArgs {
        mode: VerifyMode::Detect,
        path: Some(tmp.path().to_path_buf()),
    };
    let result = run(&args).expect("detect should succeed");
    let missing = result["missing"].as_array().expect("missing array");

    assert!(
        missing.iter().any(|v| v.as_str() == Some("android")),
        "Android dir with no .kt files should be missing: {result}"
    );
}
