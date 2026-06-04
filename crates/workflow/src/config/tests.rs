use std::fs;

use proptest::prelude::*;
use tempfile::tempdir;

use super::*;

fn write_config(dir: &Path, yaml: &str) {
    let specify = dir.join(".specify");
    fs::create_dir_all(&specify).expect("create .specify");
    fs::write(specify.join("project.yaml"), yaml).expect("write project.yaml");
}

#[test]
fn specify_subpaths() {
    let base = Path::new("/a/b");
    let layout = Layout::new(base);
    assert_eq!(layout.project_dir(), base);
    assert_eq!(layout.specify_dir(), PathBuf::from("/a/b/.specify"));
    assert_eq!(layout.config_path(), PathBuf::from("/a/b/.specify/project.yaml"));
    assert_eq!(layout.slices_dir(), PathBuf::from("/a/b/.specify/slices"));
    assert_eq!(layout.topology_lock_path(), PathBuf::from("/a/b/.specify/topology.lock"));
    assert_eq!(layout.registry_path(), PathBuf::from("/a/b/registry.yaml"));
    assert_eq!(layout.plan_path(), PathBuf::from("/a/b/plan.yaml"));
    assert_eq!(layout.change_brief_path(), PathBuf::from("/a/b/change.md"));
    assert_eq!(layout.discovery_path(), PathBuf::from("/a/b/discovery.md"));
    assert_eq!(layout.cache_dir(), PathBuf::from("/a/b/.specify/.cache"));
    assert_eq!(layout.archive_dir(), PathBuf::from("/a/b/.specify/archive"));
}

fn sample_cfg(rules: BTreeMap<String, String>) -> ProjectConfig {
    ProjectConfig {
        name: "demo".to_string(),
        description: None,
        adapter: Some("omnia".to_string()),
        specify_version: None,
        rules,
        tools: Vec::new(),
        platforms: Vec::new(),
        workspace: false,
    }
}

#[test]
fn rule_path_empty_map_is_none() {
    let cfg = sample_cfg(BTreeMap::new());
    assert!(cfg.rule_path(Path::new("/proj"), "proposal").is_none());
}

#[test]
fn rule_path_empty_value_is_none() {
    let mut rules = BTreeMap::new();
    rules.insert("proposal".to_string(), String::new());
    let cfg = sample_cfg(rules);
    assert!(cfg.rule_path(Path::new("/proj"), "proposal").is_none());
}

#[test]
fn rule_path_resolves_under_specify_dir() {
    let mut rules = BTreeMap::new();
    rules.insert("proposal".to_string(), "rules/proposal.md".to_string());
    let cfg = sample_cfg(rules);
    assert_eq!(
        cfg.rule_path(Path::new("/proj"), "proposal"),
        Some(PathBuf::from("/proj/.specify/rules/proposal.md"))
    );
}

#[test]
fn load_not_initialized_when_missing() {
    let tmp = tempdir().unwrap();
    let err = ProjectConfig::load(tmp.path()).expect_err("missing file errs");
    assert!(matches!(err, Error::NotInitialized));
}

#[test]
fn load_refuses_future_specify_version() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\nspecify_version: \"99.0.0\"\n");
    let err = ProjectConfig::load(tmp.path()).expect_err("future version rejected");
    match err {
        Error::CliTooOld { required, found } => {
            assert_eq!(required, "99.0.0");
            assert_eq!(found, env!("CARGO_PKG_VERSION"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn load_accepts_floor_lte_current() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\nspecify_version: \"0.0.1\"\n");
    ProjectConfig::load(tmp.path()).expect("older version loads");

    let tmp = tempdir().unwrap();
    let exact = env!("CARGO_PKG_VERSION");
    write_config(
        tmp.path(),
        &format!("name: demo\nadapter: omnia\nspecify_version: \"{exact}\"\n"),
    );
    ProjectConfig::load(tmp.path()).expect("exact version loads");
}

#[test]
fn major_parses_or_none() {
    assert_eq!(major("1.2.3"), Some(1));
    assert_eq!(major("nope"), None);
}

#[test]
fn needs_migration_detects_older_major() {
    assert_eq!(needs_migration("2.0.0", "1.5.0"), Some(("1.5.0".to_string(), "2.0.0".to_string())));
}

#[test]
fn needs_migration_none_for_same_major() {
    assert_eq!(needs_migration("2.3.0", "2.0.0"), None);
}

#[test]
fn needs_migration_none_for_newer_pin() {
    assert_eq!(needs_migration("1.0.0", "2.0.0"), None);
}

#[test]
fn needs_migration_none_for_unparseable_pin() {
    assert_eq!(needs_migration("2.0.0", "not-a-semver"), None);
}

#[test]
fn needs_migration_unparseable_current() {
    assert_eq!(needs_migration("not-a-semver", "1.0.0"), None);
}

#[test]
fn load_no_migration_same_major() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\nspecify_version: \"0.0.1\"\n");
    let cfg = ProjectConfig::load(tmp.path()).expect("same-major pin loads");
    assert_eq!(cfg.specify_version.as_deref(), Some("0.0.1"));
}

#[test]
fn load_for_migration_same_major() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\nspecify_version: \"0.0.1\"\n");
    let (cfg, migration) =
        ProjectConfig::load_for_migration(tmp.path()).expect("loads for migration");
    assert!(migration.is_none(), "same-major pin needs no migration");
    assert_eq!(cfg.name, "demo");
    assert_eq!(cfg.adapter.as_deref(), Some("omnia"));
    assert_eq!(cfg.specify_version.as_deref(), Some("0.0.1"));
}

#[test]
fn load_allows_invalid_pinned_version() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\nspecify_version: not-a-semver\n");
    let cfg = ProjectConfig::load(tmp.path()).expect("unparseable version is permissive");
    assert_eq!(cfg.specify_version.as_deref(), Some("not-a-semver"));
}

#[test]
fn hub_field_defaults_false_round_trips() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\n");
    let cfg = ProjectConfig::load(tmp.path()).expect("loads");
    assert!(!cfg.workspace, "workspace must default to false when absent");
    assert_eq!(cfg.adapter.as_deref(), Some("omnia"));
    assert!(cfg.tools.is_empty(), "tools must default empty when absent");

    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nworkspace: true\n");
    let cfg = ProjectConfig::load(tmp.path()).expect("loads");
    assert!(cfg.workspace, "workspace: true must round-trip through deserialize");
    assert!(cfg.adapter.is_none(), "hub project.yaml must omit adapter:");
}

#[test]
fn workspace_omitted_when_false() {
    let cfg = ProjectConfig {
        name: "demo".to_string(),
        description: None,
        adapter: Some("omnia".to_string()),
        specify_version: None,
        rules: BTreeMap::new(),
        tools: Vec::new(),
        platforms: Vec::new(),
        workspace: false,
    };
    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(!yaml.contains("workspace:"), "workspace: false should be omitted, got:\n{yaml}");
    assert!(yaml.contains("adapter: omnia"), "adapter: must serialise, got:\n{yaml}");
}

#[test]
fn hub_field_serialised_when_true() {
    let cfg = ProjectConfig {
        name: "platform".to_string(),
        description: None,
        adapter: None,
        specify_version: None,
        rules: BTreeMap::new(),
        tools: Vec::new(),
        platforms: Vec::new(),
        workspace: true,
    };
    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(yaml.contains("workspace: true"), "workspace: true must serialise, got:\n{yaml}");
    assert!(!yaml.contains("adapter:"), "hub project.yaml must omit `adapter:`, got:\n{yaml}");
}

#[test]
fn tools_field_round_trips() {
    let tmp = tempdir().unwrap();
    write_config(
        tmp.path(),
        "name: demo\nadapter: omnia\ntools:\n  - name: contract\n    version: 1.0.0\n    source: https://example.com/contract.wasm\n",
    );
    let cfg = ProjectConfig::load(tmp.path()).expect("loads");
    assert_eq!(cfg.tools.len(), 1);
    assert_eq!(cfg.tools[0].name, "contract");
    assert!(matches!(
        &cfg.tools[0].source,
        specify_tool::manifest::ToolSource::HttpsUri(uri) if uri == "https://example.com/contract.wasm"
    ));

    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(yaml.contains("tools:"), "tools should serialise when present, got:\n{yaml}");
    assert!(
        yaml.contains("source: https://example.com/contract.wasm"),
        "tool source should stay in string form, got:\n{yaml}"
    );
}

#[test]
fn tools_field_omitted_when_empty() {
    let cfg = sample_cfg(BTreeMap::new());
    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(!yaml.contains("tools:"), "empty tools should be omitted, got:\n{yaml}");
}

#[test]
fn slot_detects_literal_path() {
    let path = Path::new("/repo/.specify/workspace/orders");
    assert!(is_slot(path));
}

#[test]
fn workspace_clone_detects_nested() {
    let path = Path::new("/repo/.specify/workspace/orders/src/service");
    assert!(is_slot(path));
}

#[test]
fn slot_rejects_non_slot_paths() {
    assert!(!is_slot(Path::new("/repo")));
    assert!(!is_slot(Path::new("/repo/.specify")));
    assert!(!is_slot(Path::new("/repo/.specify/workspace")));
}

#[test]
fn find_root_walks_up_to_specify_project() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let nested = root.join("sub").join("dir");
    fs::create_dir_all(&nested).expect("mkdir nested");
    write_config(root, "name: demo\nadapter: omnia\n");

    assert_eq!(ProjectConfig::find_root(root).as_deref(), Some(root));
    assert_eq!(ProjectConfig::find_root(&nested).as_deref(), Some(root));
}

#[test]
fn find_root_none_outside_tree() {
    let tmp = tempdir().unwrap();
    assert!(ProjectConfig::find_root(tmp.path()).is_none());
}

#[test]
fn platforms_absent_is_empty() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\n");
    let cfg = ProjectConfig::load(tmp.path()).expect("loads without platforms");
    assert!(cfg.platforms.is_empty());
}

#[test]
fn platforms_field_round_trips() {
    use crate::platform::Platform;

    let tmp = tempdir().unwrap();
    write_config(
        tmp.path(),
        "name: demo\nadapter: vectis\nplatforms:\n  - core\n  - ios\n  - android\n",
    );
    let cfg = ProjectConfig::load(tmp.path()).expect("loads with platforms");
    assert_eq!(cfg.platforms, vec![Platform::Core, Platform::Ios, Platform::Android]);

    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(yaml.contains("platforms:"), "platforms must serialise when present, got:\n{yaml}");
    assert!(yaml.contains("- core"), "must contain core, got:\n{yaml}");
    assert!(yaml.contains("- ios"), "must contain ios, got:\n{yaml}");
    assert!(yaml.contains("- android"), "must contain android, got:\n{yaml}");
}

#[test]
fn platforms_omitted_when_empty() {
    let cfg = sample_cfg(BTreeMap::new());
    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(!yaml.contains("platforms:"), "empty platforms should be omitted, got:\n{yaml}");
}

#[test]
fn platforms_field_preserves_order() {
    use crate::platform::Platform;

    let tmp = tempdir().unwrap();
    write_config(
        tmp.path(),
        "name: demo\nadapter: vectis\nplatforms:\n  - core\n  - android\n  - ios\n  - web\n  - desktop\n",
    );
    let cfg = ProjectConfig::load(tmp.path()).expect("loads");
    assert_eq!(
        cfg.platforms,
        vec![Platform::Core, Platform::Android, Platform::Ios, Platform::Web, Platform::Desktop]
    );
}

proptest! {
    // `major` returns the leading component of a well-formed semver.
    #[test]
    fn major_extracts_first_component(maj in 0_u64..50, min in 0_u64..50, pat in 0_u64..50) {
        prop_assert_eq!(major(&format!("{maj}.{min}.{pat}")), Some(maj));
    }

    // Migration is required exactly when the pinned major is strictly
    // older than the current major; otherwise the result is `None`.
    #[test]
    fn migration_iff_pin_major_older(
        cur in (0_u64..8, 0_u64..5, 0_u64..5),
        pin in (0_u64..8, 0_u64..5, 0_u64..5),
    ) {
        let current = format!("{}.{}.{}", cur.0, cur.1, cur.2);
        let pinned = format!("{}.{}.{}", pin.0, pin.1, pin.2);
        let got = needs_migration(&current, &pinned);
        if pin.0 < cur.0 {
            prop_assert_eq!(got, Some((pinned, current)));
        } else {
            prop_assert!(got.is_none());
        }
    }

    // The relation is antisymmetric: if one direction needs migration the
    // reverse direction never does.
    #[test]
    fn migration_is_antisymmetric(
        cur in (0_u64..8, 0_u64..5, 0_u64..5),
        pin in (0_u64..8, 0_u64..5, 0_u64..5),
    ) {
        let current = format!("{}.{}.{}", cur.0, cur.1, cur.2);
        let pinned = format!("{}.{}.{}", pin.0, pin.1, pin.2);
        if needs_migration(&current, &pinned).is_some() {
            prop_assert!(needs_migration(&pinned, &current).is_none());
        }
    }

    // A version is never older than itself.
    #[test]
    fn same_version_never_migrates(maj in 0_u64..50, min in 0_u64..50, pat in 0_u64..50) {
        let v = format!("{maj}.{min}.{pat}");
        prop_assert!(needs_migration(&v, &v).is_none());
    }
}
