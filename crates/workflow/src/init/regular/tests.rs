use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;

use crate::config::ProjectConfig;
use crate::init::cache::CacheMeta;
use crate::init::{InitOptions, fixed_now, init};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root above crates/init")
        .to_path_buf()
}

fn omnia_target_dir() -> PathBuf {
    repo_root().join("tests").join("fixtures").join("adapters").join("targets").join("omnia")
}

/// Recursively copy `src` into `dst`, used to assemble a synthetic
/// framework source tree from the in-repo omnia fixture.
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("mkdir dst");
    for entry in fs::read_dir(src).expect("read_dir src") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// Write a schema-valid shared-rules markdown file under
/// `<root>/adapters/shared/rules/<pack>/<id>.md`.
fn write_shared_rule(root: &Path, pack: &str, id: &str) {
    let path = root.join(format!("adapters/shared/rules/{pack}/{id}.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir rule dir");
    fs::write(
            &path,
            format!(
                "---\nid: {id}\ntitle: {id} fixture\nseverity: important\ntrigger: Synthetic codex distribution fixture trigger sentence for schema.\n---\n\n## Rule\n\nBody for {id}.\n"
            ),
        )
        .expect("write rule fixture");
}

/// Build a synthetic framework source repo under `root` carrying the
/// omnia target adapter plus the shared `universal/` pack (and,
/// when `with_core`, the framework `core/` pack). Returns the path
/// to the target adapter dir for use as the init `<adapter>` arg.
fn synthetic_framework_source(root: &Path, with_core: bool) -> PathBuf {
    let omnia = root.join("adapters/targets/omnia");
    copy_tree(&omnia_target_dir(), &omnia);
    write_shared_rule(root, "universal", "UNI-901");
    if with_core {
        write_shared_rule(root, "core", "CORE-901");
    }
    omnia
}

fn base_opts<'a>(project_dir: &'a Path, target_dir: &'a Path) -> InitOptions<'a> {
    InitOptions {
        project_dir,
        adapter: Some(target_dir.to_str().expect("target path utf8")),
        name: Some("demo"),
        description: None,
        hub: false,
        include_framework: false,
    }
}

#[test]
fn init_creates_specify_tree() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    let result = init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("init ok");

    for sub in
        [".specify", ".specify/slices", ".specify/specs", ".specify/archive", ".specify/.cache"]
    {
        assert!(tmp.path().join(sub).is_dir(), "expected directory {sub} to exist");
    }
    let config_path = tmp.path().join(".specify/project.yaml");
    assert!(config_path.is_file());
    assert_eq!(result.config_path, config_path);
    assert_eq!(result.adapter_name, "omnia");

    // Non-hub init must not pre-touch any platform-component
    // artefact at the repo root. Operators mint these via
    // `specrun registry add` and `specrun plan create`
    // (which scaffolds change.md + plan.yaml together).
    for absent in ["registry.yaml", "plan.yaml", "change.md"] {
        assert!(
            !tmp.path().join(absent).exists(),
            "non-hub init must not pre-touch `{absent}` at the repo root"
        );
    }

    let mut keys = result.scaffolded_rule_keys;
    keys.sort();
    assert_eq!(keys, vec!["design", "proposal", "specs", "tasks"]);

    let cfg = ProjectConfig::load(tmp.path()).expect("reload ok");
    assert_eq!(cfg.name, "demo");
    let cap = cfg.adapter.as_deref().expect("adapter set on regular init");
    assert!(cap.starts_with("file://"), "adapter: {cap}");
    assert!(cap.ends_with("/adapters/targets/omnia"), "adapter: {cap}");
    assert!(!cfg.hub, "regular init must not set hub");
    assert_eq!(cfg.specify_version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    let mut rule_keys: Vec<_> = cfg.rules.keys().cloned().collect();
    rule_keys.sort();
    assert_eq!(rule_keys, vec!["design", "proposal", "specs", "tasks"]);
    for value in cfg.rules.values() {
        assert!(value.is_empty());
    }
}

#[test]
fn init_distributes_shared_codex() {
    let src = tempdir().unwrap();
    let omnia = synthetic_framework_source(src.path(), true);
    let project = tempdir().unwrap();

    let result = init(
        InitOptions {
            project_dir: project.path(),
            adapter: Some(omnia.to_str().expect("adapter path utf8")),
            name: Some("demo"),
            description: None,
            hub: false,
            include_framework: false,
        },
        fixed_now(),
    )
    .expect("init ok");

    assert!(
        result.codex_present,
        "codex must be distributed from a source carrying the shared pack"
    );
    let universal =
        project.path().join(".specify/.cache/codex/adapters/shared/rules/universal/UNI-901.md");
    assert!(universal.is_file(), "universal pack must land in the codex cache");
    let core = project.path().join(".specify/.cache/codex/adapters/shared/rules/core/CORE-901.md");
    assert!(!core.exists(), "core pack must NOT be distributed without --include-framework");

    let meta = project.path().join(".specify/.cache/codex/.codex-meta.yaml");
    let meta_text = fs::read_to_string(&meta).expect("read codex meta");
    assert!(meta_text.contains("include_framework: false"), "meta:\n{meta_text}");
    assert!(
        meta_text.contains("source:"),
        "meta must record the pinned adapter source:\n{meta_text}"
    );
}

#[test]
fn init_include_framework_distributes_core_pack() {
    let src = tempdir().unwrap();
    let omnia = synthetic_framework_source(src.path(), true);
    let project = tempdir().unwrap();

    init(
        InitOptions {
            project_dir: project.path(),
            adapter: Some(omnia.to_str().expect("adapter path utf8")),
            name: Some("demo"),
            description: None,
            hub: false,
            include_framework: true,
        },
        fixed_now(),
    )
    .expect("init ok");

    let core = project.path().join(".specify/.cache/codex/adapters/shared/rules/core/CORE-901.md");
    assert!(core.is_file(), "core pack must be distributed under --include-framework");
    let meta = project.path().join(".specify/.cache/codex/.codex-meta.yaml");
    let meta_text = fs::read_to_string(&meta).expect("read codex meta");
    assert!(meta_text.contains("include_framework: true"), "meta:\n{meta_text}");
}

#[test]
fn init_without_shared_pack_skips_codex() {
    // The in-repo omnia fixture has no sibling
    // `adapters/shared/rules/` tree, so codex distribution is a
    // silent no-op (fail-soft).
    let tmp = tempdir().unwrap();
    let result = init(base_opts(tmp.path(), &omnia_target_dir()), fixed_now()).expect("init ok");
    assert!(!result.codex_present, "no shared pack at the source means no codex distribution");
    assert!(!tmp.path().join(".specify/.cache/codex").exists());
}

#[test]
fn reinit_idempotent() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    let first = init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("first init");
    let config = fs::read(&first.config_path).expect("read first config");

    let second = init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("second init");
    assert!(second.directories_created.is_empty());

    let reread = fs::read(&second.config_path).expect("read second config");
    assert_eq!(config, reread, "project.yaml contents must be stable");
}

#[test]
fn gitignore_missing_existing_duplicate() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    let gitignore = tmp.path().join(".gitignore");

    init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("init ok");
    let text = fs::read_to_string(&gitignore).expect("read gitignore");
    assert!(text.contains(".specify/.cache/"));
    assert!(text.contains(".specify/workspace/"));

    init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("re-init ok");
    let text = fs::read_to_string(&gitignore).expect("reread gitignore");
    let occurrences = text.matches(".specify/.cache/").count();
    assert_eq!(occurrences, 1);
    assert_eq!(text.matches(".specify/workspace/").count(), 1);
}

#[test]
fn gitignore_appends_to_existing() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    fs::write(tmp.path().join(".gitignore"), "target/\n").expect("seed gitignore");

    init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("init ok");

    let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read gitignore");
    assert!(text.contains("target/"));
    assert!(text.contains(".specify/.cache/"));
    assert!(text.contains(".specify/workspace/"));
    assert_eq!(text.matches(".specify/.cache/").count(), 1);
    assert_eq!(text.matches(".specify/workspace/").count(), 1);
}

#[test]
fn gitignore_existing_entry_noop() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    fs::write(tmp.path().join(".gitignore"), "target/\n.specify/.cache/\n.specify/workspace/\n")
        .expect("seed gitignore");

    init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("init ok");

    let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read");
    assert_eq!(text.matches(".specify/.cache/").count(), 1);
    assert_eq!(text.matches(".specify/workspace/").count(), 1);
}

#[test]
fn gitignore_appends_workspace_only() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    fs::write(tmp.path().join(".gitignore"), "target/\n.specify/.cache/\n")
        .expect("seed gitignore");

    init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("init ok");

    let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read");
    assert_eq!(text.matches(".specify/.cache/").count(), 1);
    assert_eq!(text.matches(".specify/workspace/").count(), 1);
}

#[test]
fn cache_present_matches_cache_meta() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    let result = init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("init ok");
    assert!(result.cache_present);

    let cache_meta = CacheMeta::path(tmp.path());
    assert!(cache_meta.is_file(), "expected cache-meta yaml at {}", cache_meta.display());
    let yaml = fs::read_to_string(&cache_meta).expect("read cache meta");
    assert!(
        yaml.contains("schema_url:") && yaml.contains("file://"),
        "expected schema_url with file:// in cache-meta:\n{yaml}",
    );
}

#[test]
fn init_writes_default_wasm_pkg_config() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    let result = init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("init ok");

    assert!(result.wasm_pkg_config_written, "fresh init must write the file");
    let path = tmp.path().join(".specify/wasm-pkg.toml");
    assert!(path.is_file(), "wasm-pkg.toml must exist after init");
    let contents = fs::read_to_string(&path).expect("read wasm-pkg.toml");
    assert!(contents.contains("default_registry = \"augentic.io\""), "{contents}");
    assert!(
        contents.contains("specify = \"augentic.io\""),
        "namespace mapping missing from {contents}"
    );
}

#[test]
fn reinit_preserves_wasm_pkg_config() {
    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("first init");

    let path = tmp.path().join(".specify/wasm-pkg.toml");
    let edited =
        "[namespace_registries]\nspecify = \"mirror.internal\"\nacme = \"acme.example.com\"\n";
    fs::write(&path, edited).expect("operator edit");

    let result = init(base_opts(tmp.path(), &target_dir), fixed_now()).expect("re-init");
    assert!(!result.wasm_pkg_config_written, "re-init must not report writing the file");
    let contents = fs::read_to_string(&path).expect("read after re-init");
    assert_eq!(contents, edited, "operator edits must be preserved byte-for-byte");
}

#[test]
fn init_rejects_cross_axis_name_collision() {
    // DECISIONS.md §"Adapter name uniqueness": initialising a
    // project as `<adapter>` (target axis) when a source-axis
    // sibling of the same name already exists in-repo must fail
    // before any cache directory is rewritten.
    use specify_error::Error;

    let tmp = tempdir().unwrap();
    let target_dir = omnia_target_dir();
    // Plant a colliding source adapter under `adapters/sources/omnia/`.
    let source_root = tmp.path().join("adapters").join("sources").join("omnia");
    fs::create_dir_all(&source_root).expect("create colliding source dir");
    fs::write(
        source_root.join("adapter.yaml"),
        r"name: omnia
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
description: Colliding source adapter for the init-time uniqueness check.
",
    )
    .expect("write colliding source manifest");

    let err = init(base_opts(tmp.path(), &target_dir), fixed_now())
        .expect_err("cross-axis name collision must fail init");
    let Error::Validation { code, .. } = err else {
        panic!("expected Error::Validation, got: {err:?}");
    };
    assert_eq!(code, "adapter-name-axis-collision");
    // Cache must not have been clobbered: the target cache dir
    // should be absent because the check fires before the copy.
    let cache_dir = tmp.path().join(".specify/.cache/manifests/targets/omnia");
    assert!(
        !cache_dir.exists(),
        "init must reject the collision before writing {}",
        cache_dir.display()
    );
}

#[test]
fn default_name_is_dir_basename() {
    let tmp = tempdir().unwrap();
    let project = tmp.path().join("my-project");
    fs::create_dir_all(&project).expect("create project dir");
    let target_dir = omnia_target_dir();

    let result = init(
        InitOptions {
            project_dir: &project,
            adapter: Some(target_dir.to_str().expect("target path utf8")),
            name: None,
            description: None,
            hub: false,
            include_framework: false,
        },
        fixed_now(),
    )
    .expect("init ok");

    let cfg = ProjectConfig::load(&project).expect("reload");
    assert_eq!(cfg.name, "my-project");
    assert_eq!(result.adapter_name, "omnia");
}
