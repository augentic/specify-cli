//! Regular (non-hub) init body. Scaffolds the per-project `.specify/`
//! tree, resolves the requested capability into the cache, and writes
//! `project.yaml`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use specify_capability::{CacheMeta, PipelineView};
use specify_config::{LayoutExt, ProjectConfig};
use specify_error::Error;

use crate::cache::cache_capability;
use crate::{InitOptions, InitResult, resolve_version, resolved_name, upsert_gitignore};

#[allow(clippy::needless_pass_by_value)]
pub fn run(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    let capability = opts.capability.ok_or(Error::InitNeedsCapability)?;
    let name = resolved_name(opts.project_dir, opts.name);
    let layout = opts.project_dir.layout();

    let mut directories_created: Vec<PathBuf> = Vec::new();
    // Repo-root artefacts (`registry.yaml`, `change.md`, `plan.yaml`)
    // are not pre-touched — their owning verbs mint them on demand.
    // `.specify/specs/` is retained as a per-project convention used
    // by the bundled `omnia` capability.
    for dir in [
        layout.specify_dir(),
        layout.slices_dir(),
        layout.specify_dir().join("specs"),
        layout.archive_dir(),
        layout.cache_dir(),
    ] {
        let already = dir.exists();
        fs::create_dir_all(&dir)?;
        if !already {
            directories_created.push(dir);
        }
    }

    let resolved = cache_capability(capability, opts.project_dir)?;
    let view = PipelineView::load(&resolved.capability_value, opts.project_dir)?;
    let capability_name = view.capability.manifest.name.clone();
    let scaffolded_rule_keys: Vec<String> =
        view.capability.manifest.pipeline.define.iter().map(|entry| entry.id.clone()).collect();

    let specify_version = resolve_version(opts.project_dir, opts.version_mode)?;

    let mut rules: BTreeMap<String, String> = BTreeMap::new();
    for key in &scaffolded_rule_keys {
        rules.insert(key.clone(), String::new());
    }
    let cfg = ProjectConfig {
        name,
        domain: opts.domain.map(str::to_string),
        capability: Some(resolved.capability_value),
        specify_version: Some(specify_version.clone()),
        rules,
        tools: Vec::new(),
        hub: false,
    };

    let config_path = layout.config_path();
    let serialised = serde_saphyr::to_string(&cfg)?;
    fs::write(&config_path, serialised)?;

    upsert_gitignore(opts.project_dir)?;

    let cache_present = CacheMeta::path(opts.project_dir).exists();

    Ok(InitResult {
        config_path,
        capability_name,
        cache_present,
        directories_created,
        scaffolded_rule_keys,
        specify_version,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use specify_capability::CacheMeta;
    use specify_config::{LayoutExt, ProjectConfig};
    use tempfile::tempdir;

    use crate::{InitOptions, VersionMode, init};

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root above crates/init")
            .to_path_buf()
    }

    fn omnia_schema_dir() -> PathBuf {
        repo_root().join("schemas").join("omnia")
    }

    fn base_opts<'a>(project_dir: &'a Path, schema_dir: &'a Path) -> InitOptions<'a> {
        InitOptions {
            project_dir,
            capability: Some(schema_dir.to_str().expect("schema path utf8")),
            name: Some("demo"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: false,
        }
    }

    #[test]
    fn init_creates_specify_tree() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        let result = init(base_opts(tmp.path(), &schema_dir)).expect("init ok");

        for sub in
            [".specify", ".specify/slices", ".specify/specs", ".specify/archive", ".specify/.cache"]
        {
            assert!(tmp.path().join(sub).is_dir(), "expected directory {sub} to exist");
        }
        let config_path = tmp.path().join(".specify/project.yaml");
        assert!(config_path.is_file());
        assert_eq!(result.config_path, config_path);
        assert_eq!(result.capability_name, "omnia");

        // Non-hub init must not pre-touch any platform-component
        // artefact at the repo root. Operators mint these via
        // `specify registry add`, `specify change create`, and
        // `specify change plan create`.
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
        let cap = cfg.capability.as_deref().expect("capability set on regular init");
        assert!(cap.starts_with("file://"), "capability: {cap}");
        assert!(cap.ends_with("/schemas/omnia"), "capability: {cap}");
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
    fn reinit_idempotent() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        let first = init(base_opts(tmp.path(), &schema_dir)).expect("first init");
        let config = fs::read(&first.config_path).expect("read first config");

        let second = init(base_opts(tmp.path(), &schema_dir)).expect("second init");
        assert!(second.directories_created.is_empty());

        let reread = fs::read(&second.config_path).expect("read second config");
        assert_eq!(config, reread, "project.yaml contents must be stable");
    }

    #[test]
    fn gitignore_missing_existing_duplicate() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        let gitignore = tmp.path().join(".gitignore");

        init(base_opts(tmp.path(), &schema_dir)).expect("init ok");
        let text = fs::read_to_string(&gitignore).expect("read gitignore");
        assert!(text.contains(".specify/.cache/"));
        assert!(text.contains(".specify/workspace/"));

        init(base_opts(tmp.path(), &schema_dir)).expect("re-init ok");
        let text = fs::read_to_string(&gitignore).expect("reread gitignore");
        let occurrences = text.matches(".specify/.cache/").count();
        assert_eq!(occurrences, 1);
        assert_eq!(text.matches(".specify/workspace/").count(), 1);
    }

    #[test]
    fn gitignore_appends_to_existing() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        fs::write(tmp.path().join(".gitignore"), "target/\n").expect("seed gitignore");

        init(base_opts(tmp.path(), &schema_dir)).expect("init ok");

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
        let schema_dir = omnia_schema_dir();
        fs::write(
            tmp.path().join(".gitignore"),
            "target/\n.specify/.cache/\n.specify/workspace/\n",
        )
        .expect("seed gitignore");

        init(base_opts(tmp.path(), &schema_dir)).expect("init ok");

        let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read");
        assert_eq!(text.matches(".specify/.cache/").count(), 1);
        assert_eq!(text.matches(".specify/workspace/").count(), 1);
    }

    #[test]
    fn gitignore_appends_workspace_only() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        fs::write(tmp.path().join(".gitignore"), "target/\n.specify/.cache/\n")
            .expect("seed gitignore");

        init(base_opts(tmp.path(), &schema_dir)).expect("init ok");

        let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read");
        assert_eq!(text.matches(".specify/.cache/").count(), 1);
        assert_eq!(text.matches(".specify/workspace/").count(), 1);
    }

    #[test]
    fn cache_present_matches_cache_meta() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        let result = init(base_opts(tmp.path(), &schema_dir)).expect("init ok");
        assert!(result.cache_present);

        let cache_meta = CacheMeta::path(tmp.path());
        let meta = CacheMeta::load(tmp.path()).expect("load cache meta").expect("cache meta");
        assert!(meta.schema_url.starts_with("file://"));
        assert!(cache_meta.is_file());
    }

    #[test]
    fn preserve_mode_keeps_existing_pinned_version() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        init(base_opts(tmp.path(), &schema_dir)).expect("fresh init");

        // Manually edit the pinned version to an older one; Preserve
        // should keep it on re-init.
        let config_path = tmp.path().layout().config_path();
        let original = fs::read_to_string(&config_path).expect("read");
        let edited = original.replace(
            &format!("specify_version: {}", env!("CARGO_PKG_VERSION")),
            "specify_version: 0.0.1",
        );
        fs::write(&config_path, edited).expect("write edited");

        let result = init(InitOptions {
            version_mode: VersionMode::Preserve,
            ..base_opts(tmp.path(), &schema_dir)
        })
        .expect("preserve init");
        assert_eq!(result.specify_version, "0.0.1");
    }

    #[test]
    fn default_name_is_dir_basename() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("my-project");
        fs::create_dir_all(&project).expect("create project dir");
        let schema_dir = omnia_schema_dir();

        let result = init(InitOptions {
            project_dir: &project,
            capability: Some(schema_dir.to_str().expect("schema path utf8")),
            name: None,
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: false,
        })
        .expect("init ok");

        let cfg = ProjectConfig::load(&project).expect("reload");
        assert_eq!(cfg.name, "my-project");
        assert_eq!(result.capability_name, "omnia");
    }
}
