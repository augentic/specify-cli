//! `init` — the orchestration called by `specify init`.
//!
//! Creates `.specify/{changes,specs,archive,.cache}/`, resolves the
//! requested schema URI into `.specify/.cache/`, writes
//! `.specify/project.yaml` with a `rules:` key scaffolded from the
//! resolved schema's `pipeline.define` briefs, and upserts the
//! `.specify/.cache/` and `.specify/workspace/` lines into the project
//! `.gitignore`. Two calls with identical options are safe — the only
//! effect of the second call is refreshing the schema cache and
//! overwriting `project.yaml` with byte-identical content.
//!
//! Hub mode (`InitOptions::hub: true`, RFC-9 §1D) takes a different
//! shape: a registry-only platform hub holds `registry.yaml` and
//! `initiative.md` at the repo root and a sentinel `project.yaml {
//! schema: hub, hub: true }` under `.specify/`, but never carries
//! phase-pipeline rules of its own. Hub init refuses to run when
//! `.specify/` already exists so it never clobbers a regular
//! single-repo project.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use specify_change::is_valid_kebab_name;
use specify_error::Error;
use specify_capability::{CacheMeta, InitiativeBrief, PipelineView, Registry};

use crate::config::ProjectConfig;

/// Sentinel value written into `project.yaml:schema` for a hub. Read by
/// downstream skills/CLI as "phase pipelines disabled" — the hub never
/// runs define/build/merge against itself.
pub const HUB_SCHEMA_SENTINEL: &str = "hub";

/// Inputs to [`init`]. Borrow-shaped so callers (the CLI and tests) can
/// build the struct without cloning path buffers.
pub struct InitOptions<'a> {
    /// Root of the project being initialised.
    pub project_dir: &'a Path,
    /// Schema URI to fetch or copy into `.specify/.cache/`. Required
    /// for regular init and ignored when [`InitOptions::hub`] is `true`
    /// — hubs use the [`HUB_SCHEMA_SENTINEL`].
    pub schema_uri: Option<&'a str>,
    /// Project name; defaults to the project directory name when `None`.
    pub name: Option<&'a str>,
    /// Optional project domain description.
    pub domain: Option<&'a str>,
    /// Controls what `specify_version` gets written into `project.yaml`.
    pub version_mode: VersionMode,
    /// When `true`, scaffold a registry-only platform **hub** (RFC-9
    /// §1D) instead of a regular project: writes `registry.yaml` and
    /// `initiative.md` at the repo root and a sentinel `project.yaml
    /// { schema: hub, hub: true }` under `.specify/`. Hub init refuses
    /// to run when `.specify/` already exists so it never clobbers a
    /// regular single-repo project.
    pub hub: bool,
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
/// When [`InitOptions::hub`] is `true`, dispatches to `init_hub`
/// instead — see its doc comment for the platform-hub on-disk shape.
///
/// # Errors
///
/// Returns an error if the operation fails.
#[allow(clippy::needless_pass_by_value)]
pub fn init(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    if opts.hub {
        return init_hub(opts);
    }
    let schema_uri = opts.schema_uri.ok_or_else(|| {
        Error::Config("specify init requires --schema-uri <uri> unless --hub is set".to_string())
    })?;

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

    let resolved_uri = cache_schema_uri(schema_uri, opts.project_dir)?;
    let view = PipelineView::load(&resolved_uri.schema_value, opts.project_dir)?;
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
        schema: resolved_uri.schema_value,
        specify_version: Some(specify_version.clone()),
        rules,
        hub: false,
    };

    let config_path = ProjectConfig::config_path(opts.project_dir);
    let serialised = serde_saphyr::to_string(&cfg)?;
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

/// Hub variant of [`init`] (RFC-9 §1D). Scaffolds a **registry-only
/// platform hub**: the platform repo holds platform-level state
/// (`registry.yaml`, `initiative.md`, `plan.yaml`, plans, `workspace/`)
/// but never appears in its own `registry.yaml` and disables phase
/// pipelines on itself via the `schema: hub` sentinel.
///
/// On-disk shape after success:
///
/// ```text
/// <project_dir>/
/// ├── registry.yaml     # { version: 1, projects: [] }
/// ├── initiative.md     # canonical template, named after the project
/// └── .specify/
///     └── project.yaml  # { schema: hub, hub: true, … }
/// ```
///
/// Refuses to run when `.specify/` already exists so the operator
/// never accidentally flips an existing single-repo project into a
/// hub. Schema resolution is intentionally skipped — there is no
/// `pipeline.define` for a hub to walk.
///
/// # Errors
///
/// Returns an error if the project name is not kebab-case, if
/// `.specify/` already exists, or if any filesystem write fails.
#[allow(clippy::needless_pass_by_value)]
fn init_hub(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    let specify_dir = ProjectConfig::specify_dir(opts.project_dir);
    if specify_dir.exists() {
        return Err(Error::Config(format!(
            "init --hub: refusing to scaffold over an existing `.specify/` at {}; \
             remove it first or run without --hub for a regular project",
            specify_dir.display()
        )));
    }

    let name = resolved_name(opts.project_dir, opts.name);
    if !is_valid_kebab_name(&name) {
        return Err(Error::Config(format!(
            "init --hub: project name `{name}` must be kebab-case \
             (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens). \
             Pass --name <kebab-name> to override the directory basename."
        )));
    }

    fs::create_dir_all(&specify_dir)?;
    let directories_created: Vec<PathBuf> = vec![specify_dir];

    let specify_version = resolve_version(opts.project_dir, opts.version_mode)?;

    let cfg = ProjectConfig {
        name: name.clone(),
        domain: opts.domain.map(str::to_string),
        schema: HUB_SCHEMA_SENTINEL.to_string(),
        specify_version: Some(specify_version.clone()),
        rules: BTreeMap::new(),
        hub: true,
    };
    let config_path = ProjectConfig::config_path(opts.project_dir);
    let serialised = serde_saphyr::to_string(&cfg)?;
    fs::write(&config_path, serialised)?;

    let registry = Registry {
        version: 1,
        projects: Vec::new(),
    };
    let registry_path = Registry::path(opts.project_dir);
    let registry_yaml = serde_saphyr::to_string(&registry)?;
    fs::write(&registry_path, registry_yaml)?;
    // Trivially passes for an empty list, but exercise the hub-mode
    // shape check so any future registry-write code paths inherit the
    // same invariant from this seed.
    registry.validate_shape_hub()?;

    let brief_path = InitiativeBrief::path(opts.project_dir);
    let brief_body = InitiativeBrief::template(&name);
    fs::write(&brief_path, brief_body)?;

    upsert_gitignore(opts.project_dir)?;

    let cache_present = CacheMeta::path(opts.project_dir).exists();

    Ok(InitResult {
        config_path,
        schema_name: HUB_SCHEMA_SENTINEL.to_string(),
        cache_present,
        directories_created,
        scaffolded_rule_keys: Vec::new(),
        specify_version,
    })
}

#[derive(Debug)]
struct CachedSchema {
    schema_value: String,
}

fn cache_schema_uri(schema_uri: &str, project_dir: &Path) -> Result<CachedSchema, Error> {
    if schema_uri.trim().is_empty() || schema_uri != schema_uri.trim() {
        return Err(Error::SchemaResolution(
            "--schema-uri must be non-empty and must not have leading or trailing whitespace"
                .to_string(),
        ));
    }

    let source = SchemaUri::parse(schema_uri, project_dir)?;
    let cache_dir = ProjectConfig::cache_dir(project_dir);
    let target = cache_dir.join(&source.schema_name);
    refresh_cached_schema(&source.source_dir, &target)?;
    write_cache_meta(project_dir, &source.schema_value)?;

    Ok(CachedSchema {
        schema_value: source.schema_value,
    })
}

#[derive(Debug)]
struct SchemaUri {
    schema_value: String,
    schema_name: String,
    source_dir: PathBuf,
}

impl SchemaUri {
    fn parse(schema_uri: &str, project_dir: &Path) -> Result<Self, Error> {
        if is_github_url(schema_uri) {
            return Self::from_github(schema_uri);
        }
        Self::from_local(schema_uri, project_dir)
    }

    fn from_local(schema_uri: &str, project_dir: &Path) -> Result<Self, Error> {
        let path = schema_uri
            .strip_prefix("file://")
            .map_or_else(|| PathBuf::from(schema_uri), PathBuf::from);
        let source_dir = if path.is_absolute() { path } else { project_dir.join(path) };
        ensure_schema_dir(&source_dir, schema_uri)?;
        let canonical = fs::canonicalize(&source_dir).map_err(|err| {
            Error::SchemaResolution(format!(
                "failed to canonicalize local schema URI `{schema_uri}` at {}: {err}",
                source_dir.display()
            ))
        })?;
        let schema_name = schema_name_from_dir(&canonical)?;
        let schema_value = format!("file://{}", canonical.display());
        Ok(Self {
            schema_value,
            schema_name,
            source_dir: canonical,
        })
    }

    fn from_github(schema_uri: &str) -> Result<Self, Error> {
        let spec = GithubSchemaUri::parse(schema_uri)?;
        let repo_url = format!("https://github.com/{}/{}.git", spec.owner, spec.repo);
        let checkout_dir =
            sparse_checkout_github(&repo_url, spec.checkout_ref.as_deref(), &spec.schema_path)?;
        let source_dir = checkout_dir.join(&spec.schema_path);
        ensure_schema_dir(&source_dir, schema_uri)?;

        Ok(Self {
            schema_value: schema_uri.to_string(),
            schema_name: spec.schema_name,
            source_dir,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GithubSchemaUri {
    owner: String,
    repo: String,
    checkout_ref: Option<String>,
    schema_path: String,
    schema_name: String,
}

impl GithubSchemaUri {
    fn parse(schema_uri: &str) -> Result<Self, Error> {
        let (without_suffix, suffix_ref) = split_ref_suffix(schema_uri);
        let pathless = without_suffix.strip_prefix("https://github.com/").ok_or_else(|| {
            Error::SchemaResolution(format!("unsupported GitHub URI `{schema_uri}`"))
        })?;
        let mut parts: Vec<&str> = pathless.split('/').filter(|part| !part.is_empty()).collect();
        if parts.len() < 3 {
            return Err(Error::SchemaResolution(format!(
                "GitHub schema URI `{schema_uri}` must include owner, repo, and schema path"
            )));
        }
        let owner = parts.remove(0).to_string();
        let repo = parts.remove(0).to_string();

        let (tree_ref, schema_parts): (Option<&str>, Vec<&str>) = if parts.first() == Some(&"tree")
        {
            if parts.len() < 3 {
                return Err(Error::SchemaResolution(format!(
                    "GitHub tree schema URI `{schema_uri}` must include a ref and schema path"
                )));
            }
            (Some(parts[1]), parts[2..].to_vec())
        } else {
            (None, parts)
        };

        let checkout_ref = suffix_ref.or(tree_ref).map(str::to_string);
        let schema_path = schema_parts.join("/");
        let schema_name = schema_parts.last().ok_or_else(|| {
            Error::SchemaResolution(format!("cannot derive a schema name from `{schema_uri}`"))
        })?;

        Ok(Self {
            owner,
            repo,
            checkout_ref,
            schema_path,
            schema_name: (*schema_name).to_string(),
        })
    }
}

fn is_github_url(schema_uri: &str) -> bool {
    schema_uri.starts_with("https://github.com/")
}

fn split_ref_suffix(schema_uri: &str) -> (&str, Option<&str>) {
    let last_slash = schema_uri.rfind('/').unwrap_or(0);
    if let Some(at) = schema_uri.rfind('@')
        && at > last_slash
        && at + 1 < schema_uri.len()
    {
        return (&schema_uri[..at], Some(&schema_uri[at + 1..]));
    }
    (schema_uri, None)
}

fn sparse_checkout_github(
    repo_url: &str, checkout_ref: Option<&str>, schema_path: &str,
) -> Result<PathBuf, Error> {
    let checkout_dir = unique_temp_dir("specify-capability-checkout")?;
    let mut clone_args = vec!["clone", "--depth", "1", "--filter=blob:none", "--sparse"];
    if let Some(reference) = checkout_ref {
        clone_args.push("--branch");
        clone_args.push(reference);
    }
    clone_args.push(repo_url);
    let checkout_arg = checkout_dir.to_string_lossy().to_string();
    clone_args.push(&checkout_arg);
    run_git(&clone_args, "clone schema repository")?;

    let checkout_dir_arg = checkout_dir.to_string_lossy().to_string();
    run_git(
        &["-C", &checkout_dir_arg, "sparse-checkout", "set", "--", schema_path],
        "sparse-checkout schema path",
    )?;
    Ok(checkout_dir)
}

fn run_git(args: &[&str], action: &str) -> Result<(), Error> {
    let output = Command::new("git").args(args).output().map_err(|err| {
        Error::SchemaResolution(format!("failed to spawn `git` to {action}: {err}"))
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Error::SchemaResolution(format!("git failed to {action}: {}", stderr.trim())))
}

fn unique_temp_dir(prefix: &str) -> Result<PathBuf, Error> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| Error::SchemaResolution(format!("system clock before unix epoch: {err}")))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn ensure_schema_dir(path: &Path, original_uri: &str) -> Result<(), Error> {
    let schema_path = path.join("schema.yaml");
    if schema_path.is_file() {
        return Ok(());
    }
    Err(Error::SchemaResolution(format!(
        "schema URI `{original_uri}` did not resolve to a schema directory with schema.yaml at {}",
        schema_path.display()
    )))
}

fn schema_name_from_dir(path: &Path) -> Result<String, Error> {
    path.file_name().and_then(|name| name.to_str()).map(str::to_string).ok_or_else(|| {
        Error::SchemaResolution(format!("cannot derive schema name from {}", path.display()))
    })
}

fn refresh_cached_schema(source: &Path, target: &Path) -> Result<(), Error> {
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    copy_dir_recursive(source, target)
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), Error> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn write_cache_meta(project_dir: &Path, schema_value: &str) -> Result<(), Error> {
    let meta = CacheMeta {
        schema_url: schema_value.to_string(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
    };
    let meta_path = CacheMeta::path(project_dir);
    let serialised = serde_saphyr::to_string(&meta)?;
    fs::write(meta_path, serialised)?;
    Ok(())
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
    let existing: ProjectConfig = serde_saphyr::from_str(&text)?;
    Ok(existing.specify_version.unwrap_or(current))
}

const SPECIFY_GITIGNORE_ENTRIES: &[&str] = &[".specify/.cache/", ".specify/workspace/"];

/// Idempotent: ensure each line in `SPECIFY_GITIGNORE_ENTRIES` appears
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

    fn omnia_schema_dir() -> PathBuf {
        repo_root().join("schemas").join("omnia")
    }

    fn base_opts<'a>(project_dir: &'a Path, schema_dir: &'a Path) -> InitOptions<'a> {
        InitOptions {
            project_dir,
            schema_uri: Some(schema_dir.to_str().expect("schema path utf8")),
            name: Some("demo"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: false,
        }
    }

    #[test]
    fn github_schema_uri_parses_default_main() {
        let parsed = GithubSchemaUri::parse("https://github.com/owner/repo/schemas/omnia")
            .expect("parse GitHub URI");
        assert_eq!(
            parsed,
            GithubSchemaUri {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
                checkout_ref: None,
                schema_path: "schemas/omnia".to_string(),
                schema_name: "omnia".to_string(),
            }
        );
    }

    #[test]
    fn github_schema_uri_parses_suffix_ref() {
        let parsed = GithubSchemaUri::parse("https://github.com/owner/repo/schemas/omnia@v1")
            .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("v1"));
        assert_eq!(parsed.schema_path, "schemas/omnia");
        assert_eq!(parsed.schema_name, "omnia");
    }

    #[test]
    fn github_schema_uri_parses_tree_ref() {
        let parsed =
            GithubSchemaUri::parse("https://github.com/owner/repo/tree/main/schemas/omnia")
                .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("main"));
        assert_eq!(parsed.schema_path, "schemas/omnia");
        assert_eq!(parsed.schema_name, "omnia");
    }

    #[test]
    fn init_creates_specify_tree() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        let result = init(base_opts(tmp.path(), &schema_dir)).expect("init ok");

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
        assert!(cfg.schema.starts_with("file://"));
        assert!(cfg.schema.ends_with("/schemas/omnia"));
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

        // Re-init must not duplicate the entry.
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
        // Exactly one occurrence even after the upsert.
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
        let config_path = ProjectConfig::config_path(tmp.path());
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
            schema_uri: Some(schema_dir.to_str().expect("schema path utf8")),
            name: None,
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: false,
        })
        .expect("init ok");

        let cfg = ProjectConfig::load(&project).expect("reload");
        assert_eq!(cfg.name, "my-project");
        assert_eq!(result.schema_name, "omnia");
    }

    fn hub_opts<'a>(project_dir: &'a Path, name: &'a str) -> InitOptions<'a> {
        InitOptions {
            project_dir,
            schema_uri: None,
            name: Some(name),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: true,
        }
    }

    #[test]
    fn hub_init_writes_canonical_on_disk_shape() {
        let tmp = tempdir().unwrap();
        let result = init(hub_opts(tmp.path(), "platform-hub")).expect("hub init ok");

        let project_yaml = tmp.path().join(".specify/project.yaml");
        let registry_yaml = tmp.path().join("registry.yaml");
        let initiative_md = tmp.path().join("initiative.md");
        assert!(project_yaml.is_file(), "project.yaml missing");
        assert!(registry_yaml.is_file(), "registry.yaml missing at repo root");
        assert!(initiative_md.is_file(), "initiative.md missing at repo root");

        // Phase-pipeline directories MUST NOT be scaffolded for a hub —
        // the sentinel `schema: hub` disables the define-build-merge
        // loop on the hub itself.
        assert!(!tmp.path().join(".specify/changes").exists());
        assert!(!tmp.path().join(".specify/specs").exists());
        assert!(!tmp.path().join(".specify/.cache").exists());

        let cfg = ProjectConfig::load(tmp.path()).expect("reload project.yaml");
        assert_eq!(cfg.schema, HUB_SCHEMA_SENTINEL);
        assert!(cfg.hub, "project.yaml must carry hub: true");
        assert!(cfg.rules.is_empty(), "hubs do not scaffold rules");
        assert_eq!(cfg.name, "platform-hub");

        let registry = Registry::load(tmp.path()).expect("registry parses").expect("present");
        assert_eq!(registry.version, 1);
        assert!(registry.projects.is_empty(), "hub registry starts empty");

        let brief = InitiativeBrief::load(tmp.path()).expect("brief parses").expect("present");
        assert_eq!(brief.frontmatter.name, "platform-hub");

        assert_eq!(result.schema_name, HUB_SCHEMA_SENTINEL);
        assert!(result.scaffolded_rule_keys.is_empty());
    }

    #[test]
    fn hub_init_refuses_when_specify_dir_exists() {
        let tmp = tempdir().unwrap();
        // Pre-create `.specify/` with arbitrary content as if a regular
        // `specify init` had already run here.
        fs::create_dir_all(tmp.path().join(".specify")).unwrap();
        fs::write(tmp.path().join(".specify/project.yaml"), "name: existing\nschema: omnia\n")
            .unwrap();

        let err =
            init(hub_opts(tmp.path(), "platform-hub")).expect_err("must refuse over existing dir");
        match err {
            Error::Config(msg) => {
                assert!(
                    msg.contains("refusing to scaffold"),
                    "diagnostic should explain the refusal, got: {msg}"
                );
                assert!(msg.contains(".specify"), "diagnostic should mention .specify, got: {msg}");
            }
            other => panic!("wrong error variant: {other:?}"),
        }
        // The pre-existing project.yaml must be untouched.
        let on_disk = fs::read_to_string(tmp.path().join(".specify/project.yaml")).unwrap();
        assert_eq!(on_disk, "name: existing\nschema: omnia\n");
    }

    #[test]
    fn hub_init_rejects_non_kebab_name() {
        let tmp = tempdir().unwrap();
        let err = init(hub_opts(tmp.path(), "BadName")).expect_err("non-kebab name");
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("kebab-case"), "diagnostic should cite the rule: {msg}");
                assert!(msg.contains("BadName"), "diagnostic should echo the bad name: {msg}");
            }
            other => panic!("wrong error variant: {other:?}"),
        }
        assert!(!tmp.path().join(".specify").exists(), "no .specify on validation failure");
    }
}
