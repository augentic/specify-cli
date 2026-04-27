//! `init` — the orchestration called by `specify init`.
//!
//! Creates `.specify/{changes,specs,archive,.cache}/`, writes
//! `.specify/project.yaml` with a `rules:` key scaffolded from the
//! resolved schema's `pipeline.define` briefs, and upserts the
//! `.specify/.cache/` and `.specify/workspace/` lines into the project
//! `.gitignore`. Two calls with
//! identical options are safe — the only effect of the second call is
//! overwriting `project.yaml` with byte-identical content.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use specify_error::Error;
use specify_schema::{CacheMeta, PipelineView};

use crate::config::ProjectConfig;

/// Inputs to [`init`]. Borrow-shaped so callers (the CLI and tests) can
/// build the struct without cloning path buffers.
pub struct InitOptions<'a> {
    /// Root of the project being initialised.
    pub project_dir: &'a Path,
    /// Schema identifier (bare name or URL).
    pub schema_value: &'a str,
    /// Directory the CLI walks to discover `pipeline.define` briefs. The
    /// agent typically populates this under `.specify/.cache/` before
    /// invoking `specify init`, but any readable schema root works —
    /// `init` never writes into it.
    pub schema_source_dir: &'a Path,
    /// Project name; defaults to the project directory name when `None`.
    pub name: Option<&'a str>,
    /// Optional project domain description.
    pub domain: Option<&'a str>,
    /// Controls what `specify_version` gets written into `project.yaml`.
    pub version_mode: VersionMode,
}

/// How `init` determines the `specify_version` floor in `project.yaml`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VersionMode {
    /// Write the running binary's version as the floor (fresh init and
    /// `init --upgrade`).
    WriteCurrent,
    /// Preserve the existing `specify_version` in `project.yaml` when
    /// present (reinitialize flow).
    Preserve,
}

/// Structured summary of what `init` did, returned for downstream
/// rendering by both the JSON and text CLI paths.
#[derive(Debug, Clone)]
pub struct InitResult {
    /// Path to the written `project.yaml`.
    pub config_path: PathBuf,
    /// Resolved schema name from the schema root.
    pub schema_name: String,
    /// Whether `.specify/.cache/cache_meta.yaml` exists.
    pub cache_present: bool,
    /// Directories that were newly created (empty on re-init).
    pub directories_created: Vec<PathBuf>,
    /// Brief IDs scaffolded into the `rules:` map.
    pub scaffolded_rule_keys: Vec<String>,
    /// The `specify_version` value written into `project.yaml`.
    pub specify_version: String,
}

/// Initialise `.specify/` inside `opts.project_dir`.
///
/// Idempotent: a second call with identical options succeeds, creates no
/// new directories, doesn't duplicate the `.gitignore` entry, and writes
/// byte-identical `project.yaml` contents.
///
/// # Errors
///
/// Returns an error if the operation fails.
#[allow(clippy::needless_pass_by_value)]
pub fn init(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    let name = resolved_name(opts.project_dir, opts.name);

    let mut directories_created: Vec<PathBuf> = Vec::new();
    for dir in [
        ProjectConfig::specify_dir(opts.project_dir),
        ProjectConfig::changes_dir(opts.project_dir),
        ProjectConfig::specs_dir(opts.project_dir),
        ProjectConfig::archive_dir(opts.project_dir),
        ProjectConfig::cache_dir(opts.project_dir),
    ] {
        let already = dir.exists();
        fs::create_dir_all(&dir)?;
        if !already {
            directories_created.push(dir);
        }
    }

    let view = PipelineView::load(opts.schema_value, opts.schema_source_dir)?;
    let schema_name = view.schema.schema.name.clone();
    let scaffolded_rule_keys: Vec<String> =
        view.schema.schema.pipeline.define.iter().map(|entry| entry.id.clone()).collect();

    let specify_version = resolve_version(opts.project_dir, opts.version_mode)?;

    let mut rules: BTreeMap<String, String> = BTreeMap::new();
    for key in &scaffolded_rule_keys {
        rules.insert(key.clone(), String::new());
    }
    let cfg = ProjectConfig {
        name,
        domain: opts.domain.map(str::to_string),
        schema: opts.schema_value.to_string(),
        specify_version: Some(specify_version.clone()),
        rules,
    };

    let config_path = ProjectConfig::config_path(opts.project_dir);
    let serialised = serde_yaml_ng::to_string(&cfg)?;
    fs::write(&config_path, serialised)?;

    upsert_gitignore(opts.project_dir)?;

    let cache_present = CacheMeta::path(opts.project_dir).exists();

    Ok(InitResult {
        config_path,
        schema_name,
        cache_present,
        directories_created,
        scaffolded_rule_keys,
        specify_version,
    })
}

fn resolved_name(project_dir: &Path, explicit: Option<&str>) -> String {
    if let Some(explicit) = explicit {
        return explicit.to_string();
    }
    project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| "project".to_string(), str::to_string)
}

fn resolve_version(project_dir: &Path, mode: VersionMode) -> Result<String, Error> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    if matches!(mode, VersionMode::WriteCurrent) {
        return Ok(current);
    }

    // Preserve: keep the existing value when `project.yaml` already
    // carries one. Reading the file directly avoids re-running the
    // version-floor check inside `ProjectConfig::load` (which would
    // reject the load if the existing floor is newer than the running
    // binary — but `Preserve` is meant precisely for that case).
    let path = ProjectConfig::config_path(project_dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(current),
        Err(err) => return Err(Error::Io(err)),
    };
    let existing: ProjectConfig = serde_yaml_ng::from_str(&text)?;
    Ok(existing.specify_version.unwrap_or(current))
}

const SPECIFY_GITIGNORE_ENTRIES: &[&str] = &[".specify/.cache/", ".specify/workspace/"];

/// Idempotent: ensure each line in [`SPECIFY_GITIGNORE_ENTRIES`] appears
/// exactly once (matched with `trim()` per line) in the project
/// `.gitignore`, appending missing lines with a trailing newline.
///
/// Used by [`init`] and by `specify workspace sync` (RFC-3a
/// C29).
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn ensure_specify_gitignore_entries(project_dir: &Path) -> Result<(), Error> {
    let path = project_dir.join(".gitignore");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(Error::Io(err)),
    };

    let mut updated = existing;
    let mut changed = false;
    for entry in SPECIFY_GITIGNORE_ENTRIES {
        if updated.lines().any(|line| line.trim() == *entry) {
            continue;
        }
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(entry);
        updated.push('\n');
        changed = true;
    }

    if changed {
        fs::write(&path, updated)?;
    }
    Ok(())
}

fn upsert_gitignore(project_dir: &Path) -> Result<(), Error> {
    ensure_specify_gitignore_entries(project_dir)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn base_opts<'a>(project_dir: &'a Path, repo: &'a Path) -> InitOptions<'a> {
        InitOptions {
            project_dir,
            schema_value: "omnia",
            schema_source_dir: repo,
            name: Some("demo"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
        }
    }

    #[test]
    fn fresh_init_creates_directories_and_config() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        let result = init(base_opts(tmp.path(), &repo)).expect("init ok");

        for sub in [
            ".specify",
            ".specify/changes",
            ".specify/specs",
            ".specify/archive",
            ".specify/.cache",
        ] {
            assert!(tmp.path().join(sub).is_dir(), "expected directory {sub} to exist");
        }
        let config_path = tmp.path().join(".specify/project.yaml");
        assert!(config_path.is_file());
        assert_eq!(result.config_path, config_path);
        assert_eq!(result.schema_name, "omnia");

        let mut keys = result.scaffolded_rule_keys;
        keys.sort();
        assert_eq!(keys, vec!["design", "proposal", "specs", "tasks"]);

        let cfg = ProjectConfig::load(tmp.path()).expect("reload ok");
        assert_eq!(cfg.name, "demo");
        assert_eq!(cfg.schema, "omnia");
        assert_eq!(cfg.specify_version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        let mut rule_keys: Vec<_> = cfg.rules.keys().cloned().collect();
        rule_keys.sort();
        assert_eq!(rule_keys, vec!["design", "proposal", "specs", "tasks"]);
        for value in cfg.rules.values() {
            assert!(value.is_empty());
        }
    }

    #[test]
    fn re_init_is_idempotent_and_reports_no_new_dirs() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        let first = init(base_opts(tmp.path(), &repo)).expect("first init");
        let config = fs::read(&first.config_path).expect("read first config");

        let second = init(base_opts(tmp.path(), &repo)).expect("second init");
        assert!(second.directories_created.is_empty());

        let reread = fs::read(&second.config_path).expect("read second config");
        assert_eq!(config, reread, "project.yaml contents must be stable");
    }

    #[test]
    fn gitignore_upsert_handles_missing_existing_and_duplicate() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        let gitignore = tmp.path().join(".gitignore");

        init(base_opts(tmp.path(), &repo)).expect("init ok");
        let text = fs::read_to_string(&gitignore).expect("read gitignore");
        assert!(text.contains(".specify/.cache/"));
        assert!(text.contains(".specify/workspace/"));

        // Re-init must not duplicate the entry.
        init(base_opts(tmp.path(), &repo)).expect("re-init ok");
        let text = fs::read_to_string(&gitignore).expect("reread gitignore");
        let occurrences = text.matches(".specify/.cache/").count();
        assert_eq!(occurrences, 1);
        assert_eq!(text.matches(".specify/workspace/").count(), 1);
    }

    #[test]
    fn gitignore_upsert_appends_to_existing_content() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        fs::write(tmp.path().join(".gitignore"), "target/\n").expect("seed gitignore");

        init(base_opts(tmp.path(), &repo)).expect("init ok");

        let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read gitignore");
        assert!(text.contains("target/"));
        assert!(text.contains(".specify/.cache/"));
        assert!(text.contains(".specify/workspace/"));
        // Exactly one occurrence even after the upsert.
        assert_eq!(text.matches(".specify/.cache/").count(), 1);
        assert_eq!(text.matches(".specify/workspace/").count(), 1);
    }

    #[test]
    fn gitignore_upsert_leaves_existing_entry_alone() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        fs::write(
            tmp.path().join(".gitignore"),
            "target/\n.specify/.cache/\n.specify/workspace/\n",
        )
        .expect("seed gitignore");

        init(base_opts(tmp.path(), &repo)).expect("init ok");

        let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read");
        assert_eq!(text.matches(".specify/.cache/").count(), 1);
        assert_eq!(text.matches(".specify/workspace/").count(), 1);
    }

    #[test]
    fn gitignore_upsert_appends_workspace_when_only_cache_present() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        fs::write(tmp.path().join(".gitignore"), "target/\n.specify/.cache/\n")
            .expect("seed gitignore");

        init(base_opts(tmp.path(), &repo)).expect("init ok");

        let text = fs::read_to_string(tmp.path().join(".gitignore")).expect("read");
        assert_eq!(text.matches(".specify/.cache/").count(), 1);
        assert_eq!(text.matches(".specify/workspace/").count(), 1);
    }

    #[test]
    fn cache_present_reflects_cache_meta_presence() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        let result = init(base_opts(tmp.path(), &repo)).expect("init ok");
        assert!(!result.cache_present);

        let cache_meta = CacheMeta::path(tmp.path());
        fs::write(cache_meta, "schema_url: local:omnia\nfetched_at: 2025-01-01T00:00:00Z\n")
            .expect("write cache meta");
        let result = init(base_opts(tmp.path(), &repo)).expect("re-init ok");
        assert!(result.cache_present);
    }

    #[test]
    fn preserve_mode_keeps_existing_pinned_version() {
        let tmp = tempdir().unwrap();
        let repo = repo_root();
        init(base_opts(tmp.path(), &repo)).expect("fresh init");

        // Manually edit the pinned version to an older one; Preserve
        // should keep it on re-init.
        let config_path = ProjectConfig::config_path(tmp.path());
        let original = fs::read_to_string(&config_path).expect("read");
        let edited = original.replace(
            &format!("specify_version: {}", env!("CARGO_PKG_VERSION")),
            "specify_version: 0.0.1",
        );
        fs::write(&config_path, edited).expect("write edited");

        let result = init(InitOptions {
            version_mode: VersionMode::Preserve,
            ..base_opts(tmp.path(), &repo)
        })
        .expect("preserve init");
        assert_eq!(result.specify_version, "0.0.1");
    }

    #[test]
    fn default_name_falls_back_to_directory_basename() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("my-project");
        fs::create_dir_all(&project).expect("create project dir");
        let repo = repo_root();

        let result = init(InitOptions {
            project_dir: &project,
            schema_value: "omnia",
            schema_source_dir: &repo,
            name: None,
            domain: None,
            version_mode: VersionMode::WriteCurrent,
        })
        .expect("init ok");

        let cfg = ProjectConfig::load(&project).expect("reload");
        assert_eq!(cfg.name, "my-project");
        assert_eq!(result.schema_name, "omnia");
    }
}
