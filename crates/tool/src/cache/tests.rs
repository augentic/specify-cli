use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use super::*;
use crate::manifest::{ToolPermissions, ToolScope};
use crate::test_support::{EnvGuard, env_lock, scratch_dir};

fn project_scope() -> ToolScope {
    ToolScope::Project {
        project_name: "demo".to_string(),
    }
}

fn capability_scope() -> ToolScope {
    ToolScope::Capability {
        capability_slug: "contracts".to_string(),
        capability_dir: PathBuf::from("/capabilities/contracts"),
    }
}

fn fixed_sidecar(scope: &ToolScope, name: &str, version: &str, source: &str) -> Sidecar {
    Sidecar::new(
        scope,
        name,
        version,
        source,
        ToolPermissions {
            read: vec!["$PROJECT_DIR/contracts".to_string()],
            write: Vec::new(),
        },
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()),
        None,
        "2026-05-07T00:00:00Z".parse().expect("fixed test stamp"),
    )
    .expect("sidecar")
}

fn write_cached_version(scope: &ToolScope, name: &str, version: &str, source: &str) -> PathBuf {
    let dir = tool_dir(scope, name, version).expect("tool dir");
    fs::create_dir_all(&dir).expect("create version dir");
    fs::write(dir.join(MODULE_FILENAME), b"wasm").expect("write module");
    write_sidecar(&dir.join(SIDECAR_FILENAME), &fixed_sidecar(scope, name, version, source))
        .expect("write sidecar");
    dir
}

#[test]
fn cache_root_honours_override_precedence() {
    let override_dir = scratch_dir("override");
    let xdg_dir = scratch_dir("xdg");
    let home_dir = scratch_dir("home");
    let _g = env_lock();
    let _cache = EnvGuard::set("SPECIFY_TOOLS_CACHE", &override_dir);
    let _xdg = EnvGuard::set("XDG_CACHE_HOME", &xdg_dir);
    let _home = EnvGuard::set("HOME", &home_dir);
    assert_eq!(root().expect("cache root"), override_dir);
}

#[test]
fn cache_root_uses_xdg_before_home_fallback() {
    let xdg_dir = scratch_dir("xdg-only");
    let home_dir = scratch_dir("home-only");
    let _g = env_lock();
    let _cache = EnvGuard::unset("SPECIFY_TOOLS_CACHE");
    let _xdg = EnvGuard::set("XDG_CACHE_HOME", &xdg_dir);
    let _home = EnvGuard::set("HOME", &home_dir);
    assert_eq!(root().expect("cache root"), xdg_dir.join("specify").join("tools"));
}

#[test]
fn cache_root_uses_home_when_no_explicit_env() {
    let home_dir = scratch_dir("home-fallback");
    let _g = env_lock();
    let _cache = EnvGuard::unset("SPECIFY_TOOLS_CACHE");
    let _xdg = EnvGuard::unset("XDG_CACHE_HOME");
    let _home = EnvGuard::set("HOME", &home_dir);
    assert_eq!(root().expect("cache root"), home_dir.join(".cache").join("specify").join("tools"));
}

#[test]
fn scope_segment_formats_and_rejects_empty_names() {
    assert_eq!(scope_segment(&project_scope()).expect("project segment"), "project--demo");
    assert_eq!(
        scope_segment(&capability_scope()).expect("capability segment"),
        "capability--contracts"
    );
    let empty = ToolScope::Project {
        project_name: String::new(),
    };
    assert!(matches!(scope_segment(&empty), Err(ToolError::InvalidCacheSegment { .. })));
}

#[test]
fn sidecar_round_trips_and_schema_rejects_invalid_shape() {
    let root = scratch_dir("sidecar");
    let path = root.join(SIDECAR_FILENAME);
    let sidecar =
        fixed_sidecar(&project_scope(), "contract", "1.0.0", "https://example.test/contract.wasm");

    write_sidecar(&path, &sidecar).expect("write sidecar");
    assert_eq!(read_sidecar(&path).expect("read sidecar"), Some(sidecar));

    fs::write(
        &path,
        "schema-version: 2\nscope: project--demo\ntool-name: contract\ntool-version: 1.0.0\nsource: https://example.test/contract.wasm\nfetched-at: 2026-05-07T00:00:00Z\npermissions-snapshot:\n  read: []\n  write: []\n",
    )
    .expect("write invalid sidecar");
    assert!(matches!(
        read_sidecar(&path),
        Err(ToolError::Sidecar {
            kind: crate::error::SidecarKind::Schema(_),
            ..
        })
    ));

    let schema: serde_json::Value =
        serde_json::from_str(TOOL_SIDECAR_JSON_SCHEMA).expect("sidecar schema parses");
    jsonschema::validator_for(&schema).expect("sidecar schema compiles");
}

#[test]
fn cache_status_distinguishes_hit_not_found_and_changed_digest() {
    let cache_dir = scratch_dir("status-cache");
    let _g = env_lock();
    let _cache = EnvGuard::set("SPECIFY_TOOLS_CACHE", &cache_dir);
    let _xdg = EnvGuard::unset("XDG_CACHE_HOME");
    let _home = EnvGuard::unset("HOME");
    assert_eq!(
        status(
            &project_scope(),
            "contract",
            "1.0.0",
            "https://example.test/contract.wasm",
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        )
        .expect("cold status"),
        Status::MissNotFound
    );
    write_cached_version(
        &project_scope(),
        "contract",
        "1.0.0",
        "https://example.test/contract.wasm",
    );
    assert_eq!(
        status(
            &project_scope(),
            "contract",
            "1.0.0",
            "https://example.test/contract.wasm",
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        )
        .expect("hit status"),
        Status::Hit
    );
    assert_eq!(
        status(
            &project_scope(),
            "contract",
            "1.0.0",
            "https://example.test/contract.wasm",
            Some("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
        )
        .expect("changed status"),
        Status::MissChanged
    );
}

#[test]
fn stage_and_install_installs_complete_tree_and_replaces_existing_version() {
    let root = scratch_dir("stage");
    let staged = root.join("staged");
    let dest = root.join("cache").join("project--demo").join("contract").join("1.0.0");
    fs::create_dir_all(staged.join("nested")).expect("create staged");
    fs::write(staged.join(MODULE_FILENAME), b"new").expect("write module");
    fs::write(staged.join("nested").join("probe.txt"), b"probe").expect("write nested");

    let manual_partial = dest.with_extension("manual-tmp");
    fs::create_dir_all(&manual_partial).expect("create manual temp");
    fs::write(manual_partial.join(MODULE_FILENAME), b"partial").expect("write partial");
    assert!(!dest.exists(), "manual sibling staging must not expose dest");
    fs::remove_dir_all(&manual_partial).expect("remove manual temp");

    stage_and_install(&staged, &dest).expect("install staged");
    assert_eq!(fs::read(dest.join(MODULE_FILENAME)).expect("read module"), b"new");
    assert_eq!(fs::read(dest.join("nested").join("probe.txt")).expect("read nested"), b"probe");

    let staged_replacement = root.join("staged-replacement");
    fs::create_dir_all(&staged_replacement).expect("create replacement");
    fs::write(staged_replacement.join(MODULE_FILENAME), b"replacement").expect("write replacement");
    stage_and_install(&staged_replacement, &dest).expect("replace staged");
    assert_eq!(fs::read(dest.join(MODULE_FILENAME)).expect("read replacement"), b"replacement");
    assert!(!dest.join("nested").exists(), "replacement removes old tree");
}

#[test]
fn scan_for_gc_isolates_scope_and_uses_name_version_source_keep_set() {
    let cache_dir = scratch_dir("gc-cache");
    let _g = env_lock();
    let _cache = EnvGuard::set("SPECIFY_TOOLS_CACHE", &cache_dir);
    let _xdg = EnvGuard::unset("XDG_CACHE_HOME");
    let _home = EnvGuard::unset("HOME");

    let kept_project = write_cached_version(
        &project_scope(),
        "contract",
        "1.0.0",
        "https://example.test/contract.wasm",
    );
    let stale_project = write_cached_version(
        &project_scope(),
        "contract",
        "1.1.0",
        "https://example.test/contract-new.wasm",
    );
    let stale_capability = write_cached_version(
        &capability_scope(),
        "contract",
        "1.0.0",
        "https://example.test/contract.wasm",
    );

    let kept = HashSet::from([(
        "contract".to_string(),
        "1.0.0".to_string(),
        "https://example.test/contract.wasm".to_string(),
    )]);

    let project_gc = scan_for_gc(&project_scope(), &kept).expect("project gc");
    assert_eq!(project_gc, vec![stale_project]);
    assert!(kept_project.exists());

    let capability_gc = scan_for_gc(&capability_scope(), &HashSet::new()).expect("capability gc");
    assert_eq!(capability_gc, vec![stale_capability]);
}
