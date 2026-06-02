//! Integration tests for `specrun init` (adapter and `--workspace` modes).
//!
//! Covers the on-disk shape produced by `init`, the JSON envelope, and
//! the clap-level invariants around the positional `<adapter>`
//! argument and the `--workspace` flag.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use specify_workflow::config::ProjectConfig;
use tempfile::tempdir;

mod common;
use common::{omnia_schema_dir, specrun};

#[test]
fn init_text_format_succeeds() {
    let tmp = tempdir().unwrap();
    let assert = specrun()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(stdout.contains("Initialized"));
    assert!(stdout.contains("omnia"));
    assert!(stdout.contains(".specify/project.yaml"));

    let config_path = tmp.path().join(".specify/project.yaml");
    assert!(config_path.is_file(), "project.yaml must exist");
}

#[test]
fn init_json_format_has_stable_shape() {
    let tmp = tempdir().unwrap();
    let assert = specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");

    assert_eq!(value["adapter-name"], "omnia");
    assert!(value["config-path"].is_string());
    let config_path = value["config-path"].as_str().unwrap();
    // Canonicalized tmp path so substring match handles macOS
    // /private/var symlinks gracefully.
    let canonical_tmp = fs::canonicalize(tmp.path()).expect("canonicalize tmp");
    assert!(
        config_path.starts_with(canonical_tmp.to_string_lossy().as_ref()),
        "config_path {config_path} should start with {}",
        canonical_tmp.display()
    );
    assert!(value["specify-version"].is_string());
    assert!(value["scaffolded-rule-keys"].is_array());
}

#[test]
#[ignore = "networked GitHub fetch smoke test"]
fn init_github_directory_uri_succeeds() {
    let tmp = tempdir().unwrap();
    specrun()
        .current_dir(tmp.path())
        .args([
            "init",
            "https://github.com/augentic/specify/adapters/targets/omnia",
            "--name",
            "demo",
        ])
        .assert()
        .success();
}

// ---- `specrun init` adapter/workspace invariant: positional <adapter> + --workspace mutual exclusion ----

#[test]
fn init_writes_adapter_field_for_url_arg() {
    // Acceptance (a): `specrun init <url>` writes `adapter: <url>`
    // and no `schema:` field; `workspace:` either absent or false.
    let tmp = tempdir().unwrap();
    specrun()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .args(["--name", "demo"])
        .assert()
        .success();

    let project_yaml =
        fs::read_to_string(tmp.path().join(".specify/project.yaml")).expect("read project.yaml");
    assert!(
        project_yaml.contains("adapter:"),
        "project.yaml must carry `adapter:` after regular init, got:\n{project_yaml}"
    );
    assert!(
        !project_yaml.lines().any(|line| line.trim_start().starts_with("schema:")),
        "project.yaml must NOT carry the legacy `schema:` field, got:\n{project_yaml}"
    );
    // workspace: absent (or false) means the value is implicit; just check no
    // `workspace: true` line.
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("workspace: true")),
        "regular init must not write `workspace: true`, got:\n{project_yaml}"
    );

    // Regular init writes only `project.yaml` and the `.specify/`
    // skeleton at the project root. Platform-component artefacts at the
    // repo root are operator-managed.
    for absent in ["registry.yaml", "plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "regular init must not pre-touch `{absent}` at the repo root"
        );
    }
}

#[test]
fn init_with_no_args_errors() {
    // Acceptance (c): `specrun init` (no positional, no `--workspace`) must
    // exit `2` (clap's parse-error slot) with clap's standard
    // "required arguments were not provided" diagnostic. The historical
    // post-parse `init-requires-adapter-or-workspace` diagnostic was lifted
    // into the clap surface (`required_unless_present = "workspace"`).
    let tmp = tempdir().unwrap();
    let assert = specrun().current_dir(tmp.path()).args(["init"]).assert().failure();
    assert_eq!(assert.get_output().status.code(), Some(2), "clap parse errors map to exit code 2");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    assert!(
        stderr.contains("required arguments were not provided") && stderr.contains("ADAPTER"),
        "diagnostic must surface clap's required-arg parse error, got stderr:\n{stderr}"
    );
    assert!(
        !tmp.path().join(".specify").exists(),
        "no .specify must be scaffolded on parse failure"
    );
}

#[test]
fn init_with_adapter_and_workspace_errors() {
    // Acceptance (d): `specrun init <url> --workspace` must exit `2` with
    // clap's "the argument cannot be used with" diagnostic. Same
    // motivation as `init_with_no_args_errors`: the invariant lives in
    // clap (`conflicts_with = "workspace"`), not a post-parse diagnostic.
    let tmp = tempdir().unwrap();
    let assert = specrun()
        .current_dir(tmp.path())
        .args(["init"])
        .arg(omnia_schema_dir())
        .arg("--workspace")
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(2), "clap parse errors map to exit code 2");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    assert!(
        stderr.contains("cannot be used with") && stderr.contains("--workspace"),
        "diagnostic must mention the conflicts_with rule, got stderr:\n{stderr}"
    );
}

// ---- specrun init --workspace (registry workspace topology) ----

#[test]
fn init_workspace_writes_canonical_on_disk_shape() {
    let tmp = tempdir().unwrap();
    let assert = specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "init"])
        .args(["--name", "platform-workspace", "--workspace"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(
        value["adapter-name"], "workspace",
        "JSON response must surface adapter-name: \"workspace\", got: {value}"
    );
    assert_eq!(value["workspace-synced"], true);
    assert_eq!(value["workspace-sync-message"], "workspace sync complete");
    assert!(
        value["scaffolded-rule-keys"].as_array().expect("array").is_empty(),
        "workspace init must not scaffold rule keys, got: {}",
        value["scaffolded-rule-keys"]
    );

    // Workspace init scaffolds `project.yaml` (under `.specify/`) plus
    // `registry.yaml` at the repo root, and nothing else. `registry.yaml`
    // survives because bootstrapping a workspace is bootstrapping its registry;
    // `change.md` and `plan.yaml` stay operator-managed.
    assert!(tmp.path().join(".specify/project.yaml").is_file());
    assert!(tmp.path().join("registry.yaml").is_file());
    for absent in ["plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "workspace init must not pre-touch `{absent}` at the repo root"
        );
    }
    // Phase-pipeline directories MUST NOT be present.
    assert!(!tmp.path().join(".specify/slices").exists());
    assert!(!tmp.path().join(".specify/specs").exists());
    assert!(!tmp.path().join(".specify/.cache").exists());

    // project.yaml shape: `workspace: true` only, no `adapter:` field, and
    // no stale `schema:` sentinel.
    let project_yaml =
        fs::read_to_string(tmp.path().join(".specify/project.yaml")).expect("read project.yaml");
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("schema:")),
        "workspace project.yaml must omit the stale `schema:` field:\n{project_yaml}"
    );
    assert!(
        !project_yaml.lines().any(|l| l.trim_start().starts_with("adapter:")),
        "workspace project.yaml must omit the `adapter:` field:\n{project_yaml}"
    );
    assert!(
        project_yaml.contains("workspace: true"),
        "project.yaml must carry `workspace: true`:\n{project_yaml}"
    );

    // registry.yaml shape — version: 1, projects: [].
    let registry_yaml =
        fs::read_to_string(tmp.path().join("registry.yaml")).expect("read registry.yaml");
    assert!(
        registry_yaml.contains("version: 1"),
        "registry.yaml missing version:\n{registry_yaml}"
    );
    let registry: serde_json::Value =
        serde_yaml_to_json(&registry_yaml).expect("registry.yaml parses");
    assert_eq!(registry["version"], 1);
    assert!(
        registry["projects"].as_array().is_some_and(Vec::is_empty),
        "registry.yaml `projects` must be an empty list, got: {registry}"
    );

    // `change.md` is not scaffolded by workspace init; it appears only after
    // the operator runs `/spec:plan <name>` (or `specrun plan create <name>`).
}

#[test]
fn init_workspace_refuses_when_present() {
    let tmp = tempdir().unwrap();
    // Pre-create `.specify/` with arbitrary content.
    fs::create_dir_all(tmp.path().join(".specify")).unwrap();
    fs::write(tmp.path().join(".specify/project.yaml"), "name: existing\nadapter: omnia\n")
        .unwrap();

    let assert = specrun()
        .current_dir(tmp.path())
        .args(["init"])
        .args(["--name", "platform-workspace", "--workspace"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
    assert!(
        stderr.contains("refusing to scaffold"),
        "stderr should explain the refusal, got: {stderr:?}"
    );

    let on_disk = fs::read_to_string(tmp.path().join(".specify/project.yaml")).unwrap();
    assert_eq!(on_disk, "name: existing\nadapter: omnia\n");
}

// ---- `specrun init --upgrade` (RFC-30 §D5 re-entry version bump) ----

/// Recursively snapshot every regular file under `root` as a
/// `relative-path -> bytes` map, so an upgrade's write set can be
/// asserted by diffing two snapshots.
fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if entry.file_type().expect("file_type").is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).expect("strip prefix").to_path_buf();
                out.insert(rel, fs::read(&path).expect("read file"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Populate a brownfield regular project: an older-but-same-major pin
/// (`0.2.0`; the binary is `0.3.0`, same major `0`, so no migration),
/// a bare `adapter:`, a spread of operator artifacts, and a sentinel
/// `AGENTS.md`.
fn seed_brownfield_regular(root: &Path) {
    let specify = root.join(".specify");
    fs::create_dir_all(specify.join("slices/my-slice")).unwrap();
    fs::create_dir_all(specify.join("specs")).unwrap();
    fs::create_dir_all(specify.join("archive")).unwrap();
    fs::create_dir_all(specify.join("design-system")).unwrap();
    fs::write(
        specify.join("project.yaml"),
        "name: brownfield\ndescription: existing project\nadapter: omnia\nspecify_version: 0.2.0\nrules:\n  specs: specs.md\n",
    )
    .unwrap();
    fs::write(specify.join("slices/my-slice/spec.md"), "# operator slice\n").unwrap();
    fs::write(specify.join("specs/baseline.md"), "# baseline spec\n").unwrap();
    fs::write(specify.join("archive/old.md"), "# archived\n").unwrap();
    fs::write(
        specify.join("design-system/components.yaml"),
        "components:\n  - id: button\n    status: confirmed\n",
    )
    .unwrap();
    fs::write(root.join("AGENTS.md"), "# Sentinel AGENTS.md — operator authored\n").unwrap();
}

#[test]
fn init_upgrade_bumps_only_version_and_preserves_artifacts() {
    let tmp = tempdir().unwrap();
    seed_brownfield_regular(tmp.path());

    let before = snapshot_tree(tmp.path());
    let before_cfg: ProjectConfig = serde_saphyr::from_str(
        std::str::from_utf8(&before[Path::new(".specify/project.yaml")]).unwrap(),
    )
    .expect("parse before");

    let assert = specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["specify-version"], "0.3.0");
    assert_eq!(value["specify-version-changed"], true);
    assert_eq!(value["adapter-name"], "omnia");

    let after = snapshot_tree(tmp.path());

    // Every path other than project.yaml is byte-identical, and the
    // path set is unchanged (nothing added, nothing removed).
    let project_yaml = PathBuf::from(".specify/project.yaml");
    let before_keys: Vec<_> = before.keys().filter(|k| **k != project_yaml).collect();
    let after_keys: Vec<_> = after.keys().filter(|k| **k != project_yaml).collect();
    assert_eq!(before_keys, after_keys, "upgrade must not add or remove files");
    for key in before_keys {
        assert_eq!(before[key], after[key], "file {} must be byte-identical", key.display());
    }

    // Within project.yaml only `specify_version` changed.
    let after_cfg: ProjectConfig =
        serde_saphyr::from_str(std::str::from_utf8(&after[&project_yaml]).unwrap())
            .expect("parse after");
    assert_eq!(after_cfg.specify_version.as_deref(), Some("0.3.0"));
    let normalised = ProjectConfig {
        specify_version: before_cfg.specify_version.clone(),
        ..after_cfg
    };
    assert_eq!(normalised, before_cfg, "only specify_version may change in project.yaml");

    // Second run is a byte-stable no-op.
    let snapshot_after_first = snapshot_tree(tmp.path());
    let assert2 = specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .success();
    let value2: serde_json::Value =
        serde_json::from_slice(&assert2.get_output().stdout).expect("json");
    assert_eq!(value2["specify-version-changed"], false, "re-run must be a no-op");
    assert_eq!(
        snapshot_tree(tmp.path()),
        snapshot_after_first,
        "second --upgrade must leave the tree byte-identical"
    );
}

#[test]
fn init_upgrade_preserves_workspace_and_registry() {
    let tmp = tempdir().unwrap();
    let specify = tmp.path().join(".specify");
    fs::create_dir_all(&specify).unwrap();
    fs::write(
        specify.join("project.yaml"),
        "name: platform-workspace\nspecify_version: 0.2.0\nworkspace: true\n",
    )
    .unwrap();
    fs::write(tmp.path().join("registry.yaml"), "version: 1\nprojects: []\n").unwrap();
    fs::write(tmp.path().join("AGENTS.md"), "# Workspace sentinel\n").unwrap();

    let registry_before = fs::read(tmp.path().join("registry.yaml")).unwrap();

    let assert = specrun()
        .current_dir(tmp.path())
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["specify-version"], "0.3.0");
    assert_eq!(value["specify-version-changed"], true);
    assert_eq!(value["adapter-name"], "workspace");

    let cfg: ProjectConfig =
        serde_saphyr::from_str(&fs::read_to_string(specify.join("project.yaml")).unwrap())
            .expect("parse workspace project.yaml");
    assert!(cfg.workspace, "workspace discriminator must survive");
    assert!(cfg.adapter.is_none(), "workspace upgrade must not synthesise an adapter");
    assert_eq!(cfg.specify_version.as_deref(), Some("0.3.0"));
    let project_yaml = fs::read_to_string(specify.join("project.yaml")).unwrap();
    assert!(project_yaml.contains("workspace: true"), "upgrade must preserve workspace: key");
    assert_eq!(
        fs::read(tmp.path().join("registry.yaml")).unwrap(),
        registry_before,
        "registry.yaml must be byte-identical after a workspace upgrade"
    );

    // Second run no-op.
    let project_after_first = fs::read(specify.join("project.yaml")).unwrap();
    specrun().current_dir(tmp.path()).args(["init", "--upgrade"]).assert().success();
    assert_eq!(
        fs::read(specify.join("project.yaml")).unwrap(),
        project_after_first,
        "second workspace --upgrade must be byte-stable"
    );
}

#[test]
fn init_upgrade_conflicts_with_adapter_workspace_and_check_migration() {
    for extra in [vec!["omnia"], vec!["--workspace"], vec!["--check-migration"]] {
        let tmp = tempdir().unwrap();
        let mut cmd = specrun();
        cmd.current_dir(tmp.path()).args(["init", "--upgrade"]).args(&extra);
        let assert = cmd.assert().failure();
        assert_eq!(
            assert.get_output().status.code(),
            Some(2),
            "clap conflict for `init --upgrade {}` maps to exit 2",
            extra.join(" ")
        );
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8");
        assert!(
            stderr.contains("cannot be used with"),
            "diagnostic must surface clap's conflict for `--upgrade {}`, got:\n{stderr}",
            extra.join(" ")
        );
    }
}

/// Tiny YAML→JSON helper — we only need it for the workspace on-disk shape
/// assertion, and pulling in a full yaml dependency for one test is
/// overkill. The registry file we write is shallow so a minimal hand
/// parser via `serde_json::from_str` after an indent-stripped
/// transform would be fragile; instead we re-use `serde_saphyr` (the
/// crate the rest of the CLI uses) by routing through a `Value`.
fn serde_yaml_to_json(yaml: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value =
        serde_saphyr::from_str(yaml).map_err(|err| format!("parse error: {err}"))?;
    Ok(value)
}
