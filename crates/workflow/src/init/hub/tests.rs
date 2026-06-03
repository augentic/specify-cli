use std::fs;
use std::path::Path;

use tempfile::tempdir;

use crate::config::ProjectConfig;
use crate::init::{InitOptions, fixed_now, init};
use crate::registry::Registry;

fn hub_opts<'a>(project_dir: &'a Path, name: &'a str) -> InitOptions<'a> {
    InitOptions {
        project_dir,
        adapter: None,
        name: Some(name),
        description: None,
        hub: true,
        include_framework: false,
        platforms: None,
        upgrade: false,
    }
}

#[test]
fn hub_init_writes_canonical_on_disk_shape() {
    let tmp = tempdir().unwrap();
    let result = init(hub_opts(tmp.path(), "platform-hub"), fixed_now()).expect("hub init ok");

    let project_yaml = tmp.path().join(".specify/project.yaml");
    let registry_yaml = tmp.path().join("registry.yaml");
    assert!(project_yaml.is_file(), "project.yaml missing");
    assert!(registry_yaml.is_file(), "registry.yaml missing at repo root");

    // Hub init scaffolds `registry.yaml` (intrinsic to the hub's
    // purpose) but no other platform-component artefact.
    // `change.md` and `plan.yaml` stay operator-managed even on a hub.
    for absent in ["plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "hub init must not pre-touch `{absent}` at the repo root"
        );
    }

    // Phase-pipeline directories MUST NOT be scaffolded for a
    // hub — the absence of `adapter:` (with `hub: true`) is
    // the discriminator that disables the define-build-merge
    // loop on the hub itself.
    assert!(!tmp.path().join(".specify/slices").exists());
    assert!(!tmp.path().join(".specify/specs").exists());
    assert!(!tmp.path().join(".specify/.cache").exists());

    let cfg = ProjectConfig::load(tmp.path()).expect("reload project.yaml");
    assert!(cfg.adapter.is_none(), "hub project.yaml must omit adapter:");
    assert!(cfg.hub, "project.yaml must carry hub: true");
    assert!(cfg.rules.is_empty(), "hubs do not scaffold rules");
    assert_eq!(cfg.name, "platform-hub");

    let on_disk = fs::read_to_string(&project_yaml).expect("read project.yaml");
    assert!(
        !on_disk.contains("adapter:"),
        "hub project.yaml must omit `adapter:`, got:\n{on_disk}"
    );
    assert!(
        !on_disk.contains("schema:"),
        "hub project.yaml must omit the legacy `schema:` field, got:\n{on_disk}"
    );
    assert!(
        on_disk.contains("hub: true"),
        "hub project.yaml must serialise `hub: true`, got:\n{on_disk}"
    );

    let registry = Registry::load(tmp.path()).expect("registry parses").expect("present");
    assert_eq!(registry.version, 1);
    assert!(registry.projects.is_empty(), "hub registry starts empty");

    assert_eq!(result.adapter_name, "hub");
    assert!(result.scaffolded_rule_keys.is_empty());
}

#[test]
fn init_refuses_when_specify_dir_exists() {
    let tmp = tempdir().unwrap();
    // Pre-create `.specify/` with arbitrary content as if a regular
    // `specrun init` had already run here.
    fs::create_dir_all(tmp.path().join(".specify")).unwrap();
    fs::write(tmp.path().join(".specify/project.yaml"), "name: existing\nadapter: omnia\n")
        .unwrap();

    let err = init(hub_opts(tmp.path(), "platform-hub"), fixed_now())
        .expect_err("must refuse over existing dir");
    match err {
        specify_error::Error::Diag { code, detail } => {
            assert_eq!(code, "hub-init-specify-dir-exists");
            assert!(
                detail.contains("refusing to scaffold"),
                "diagnostic should explain the refusal, got: {detail}"
            );
            assert!(
                detail.contains(".specify"),
                "diagnostic should mention .specify, got: {detail}"
            );
        }
        other => panic!("wrong error variant: {other:?}"),
    }
    let on_disk = fs::read_to_string(tmp.path().join(".specify/project.yaml")).unwrap();
    assert_eq!(on_disk, "name: existing\nadapter: omnia\n");
}

#[test]
fn hub_init_writes_default_wasm_pkg_config() {
    let tmp = tempdir().unwrap();
    let result = init(hub_opts(tmp.path(), "platform-hub"), fixed_now()).expect("hub init ok");

    assert!(result.wasm_pkg_config_written, "fresh hub init must write the file");
    let path = tmp.path().join(".specify/wasm-pkg.toml");
    assert!(path.is_file(), "wasm-pkg.toml must exist after hub init");
    let contents = fs::read_to_string(&path).expect("read wasm-pkg.toml");
    assert!(contents.contains("default_registry = \"augentic.io\""));
    assert!(contents.contains("specify = \"augentic.io\""));
}

#[test]
fn hub_init_rejects_non_kebab_name() {
    let tmp = tempdir().unwrap();
    let err = init(hub_opts(tmp.path(), "BadName"), fixed_now()).expect_err("non-kebab name");
    match err {
        specify_error::Error::Diag { code, detail } => {
            assert_eq!(code, "hub-init-name-not-kebab");
            assert!(detail.contains("kebab-case"), "diagnostic should cite the rule: {detail}");
            assert!(detail.contains("BadName"), "diagnostic should echo the bad name: {detail}");
        }
        other => panic!("wrong error variant: {other:?}"),
    }
    assert!(!tmp.path().join(".specify").exists(), "no .specify on validation failure");
}
