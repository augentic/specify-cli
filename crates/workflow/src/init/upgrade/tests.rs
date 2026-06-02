use std::fs;
use std::path::Path;

use tempfile::tempdir;

use crate::config::ProjectConfig;
use crate::init::{InitOptions, fixed_now, init};

/// Build `--upgrade` options anchored at `project_dir`. Every
/// scaffold-shaped field is inert in upgrade mode.
fn upgrade_opts(project_dir: &Path) -> InitOptions<'_> {
    InitOptions {
        project_dir,
        adapter: None,
        name: None,
        description: None,
        workspace: false,
        include_framework: false,
        upgrade: true,
    }
}

/// Seed `.specify/project.yaml` with `contents` under `project_dir`.
fn seed_project_yaml(project_dir: &Path, contents: &str) {
    let specify = project_dir.join(".specify");
    fs::create_dir_all(&specify).expect("mkdir .specify");
    fs::write(specify.join("project.yaml"), contents).expect("write project.yaml");
}

#[test]
fn upgrade_bumps_older_same_major_pin_preserving_fields() {
    let tmp = tempdir().unwrap();
    seed_project_yaml(
        tmp.path(),
        "name: demo\ndescription: a project\nadapter: omnia\nspecify_version: 0.2.0\nrules:\n  specs: specs.md\n",
    );

    let result = init(upgrade_opts(tmp.path()), fixed_now()).expect("upgrade ok");
    assert!(result.specify_version_changed, "an older pin must be bumped");
    assert_eq!(result.specify_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(result.adapter_name, "omnia");
    assert!(result.directories_created.is_empty(), "upgrade scaffolds no directories");
    assert!(!result.wasm_pkg_config_written, "upgrade never scaffolds wasm-pkg config");

    let cfg = ProjectConfig::load(tmp.path()).expect("reload");
    assert_eq!(cfg.specify_version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(cfg.name, "demo");
    assert_eq!(cfg.description.as_deref(), Some("a project"));
    assert_eq!(cfg.adapter.as_deref(), Some("omnia"));
    assert_eq!(cfg.rules.get("specs").map(String::as_str), Some("specs.md"));
    assert!(!cfg.workspace);
}

#[test]
fn upgrade_is_byte_stable_noop_when_already_current() {
    let tmp = tempdir().unwrap();
    seed_project_yaml(
        tmp.path(),
        &format!("name: demo\nadapter: omnia\nspecify_version: {}\n", env!("CARGO_PKG_VERSION")),
    );
    let config_path = tmp.path().join(".specify/project.yaml");
    let before = fs::read(&config_path).expect("read before");

    let result = init(upgrade_opts(tmp.path()), fixed_now()).expect("upgrade ok");
    assert!(!result.specify_version_changed, "an already-current pin must be a no-op");

    let after = fs::read(&config_path).expect("read after");
    assert_eq!(before, after, "upgrade must not rewrite an already-current project.yaml");
}

#[test]
fn upgrade_preserves_workspace_discriminator() {
    let tmp = tempdir().unwrap();
    seed_project_yaml(tmp.path(), "name: platform-workspace\nspecify_version: 0.2.0\nhub: true\n");

    let result = init(upgrade_opts(tmp.path()), fixed_now()).expect("upgrade ok");
    assert!(result.specify_version_changed);
    assert_eq!(result.adapter_name, "workspace");

    let cfg = ProjectConfig::load(tmp.path()).expect("reload");
    assert!(cfg.workspace, "workspace discriminator must survive an upgrade");
    assert!(cfg.adapter.is_none(), "workspace upgrade must not synthesise an adapter");
    assert_eq!(cfg.specify_version.as_deref(), Some(env!("CARGO_PKG_VERSION")));

    let on_disk = fs::read_to_string(tmp.path().join(".specify/project.yaml")).expect("read");
    assert!(
        on_disk.contains("workspace: true"),
        "upgrade must canonicalise legacy hub: to workspace:, got:\n{on_disk}"
    );
    assert!(!on_disk.contains("hub: true"), "upgrade must drop legacy hub: key, got:\n{on_disk}");
}

#[test]
fn upgrade_refuses_when_uninitialised() {
    let tmp = tempdir().unwrap();
    let err = init(upgrade_opts(tmp.path()), fixed_now())
        .expect_err("upgrade over a bare directory must error");
    assert!(matches!(err, specify_error::Error::NotInitialized), "got: {err:?}");
}
