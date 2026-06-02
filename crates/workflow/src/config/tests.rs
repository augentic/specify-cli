use std::fs;

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
        capabilities: Vec::new(),
        keywords: Vec::new(),
        adapter: Some("omnia".to_string()),
        specify_version: None,
        rules,
        tools: Vec::new(),
        hub: false,
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
fn needs_migration_none_for_unparseable_current() {
    assert_eq!(needs_migration("not-a-semver", "1.0.0"), None);
}

#[test]
fn load_does_not_raise_migration_for_same_or_newer_major_pin() {
    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nadapter: omnia\nspecify_version: \"0.0.1\"\n");
    let cfg = ProjectConfig::load(tmp.path()).expect("same-major pin loads");
    assert_eq!(cfg.specify_version.as_deref(), Some("0.0.1"));
}

#[test]
fn load_for_migration_returns_no_tuple_for_same_major_pin() {
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
    assert!(!cfg.hub, "hub must default to false when absent");
    assert_eq!(cfg.adapter.as_deref(), Some("omnia"));
    assert!(cfg.tools.is_empty(), "tools must default empty when absent");

    let tmp = tempdir().unwrap();
    write_config(tmp.path(), "name: demo\nhub: true\n");
    let cfg = ProjectConfig::load(tmp.path()).expect("loads");
    assert!(cfg.hub, "hub: true must round-trip through deserialize");
    assert!(cfg.adapter.is_none(), "hub project.yaml must omit adapter:");
}

#[test]
fn hub_field_omitted_when_false_in_serialise() {
    let cfg = ProjectConfig {
        name: "demo".to_string(),
        description: None,
        capabilities: Vec::new(),
        keywords: Vec::new(),
        adapter: Some("omnia".to_string()),
        specify_version: None,
        rules: BTreeMap::new(),
        tools: Vec::new(),
        hub: false,
    };
    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(!yaml.contains("hub:"), "hub: false should be omitted, got:\n{yaml}");
    assert!(yaml.contains("adapter: omnia"), "adapter: must serialise, got:\n{yaml}");
}

#[test]
fn hub_field_serialised_when_true() {
    let cfg = ProjectConfig {
        name: "platform".to_string(),
        description: None,
        capabilities: Vec::new(),
        keywords: Vec::new(),
        adapter: None,
        specify_version: None,
        rules: BTreeMap::new(),
        tools: Vec::new(),
        hub: true,
    };
    let yaml = serde_saphyr::to_string(&cfg).expect("serialise");
    assert!(yaml.contains("hub: true"), "hub: true must serialise, got:\n{yaml}");
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
fn workspace_clone_detects_literal_workspace_slot() {
    let path = Path::new("/repo/.specify/workspace/orders");
    assert!(is_workspace_clone(path));
}

#[test]
fn workspace_clone_detects_nested() {
    let path = Path::new("/repo/.specify/workspace/orders/src/service");
    assert!(is_workspace_clone(path));
}

#[test]
fn workspace_clone_rejects_non_workspace_paths() {
    assert!(!is_workspace_clone(Path::new("/repo")));
    assert!(!is_workspace_clone(Path::new("/repo/.specify")));
    assert!(!is_workspace_clone(Path::new("/repo/.specify/workspace")));
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
