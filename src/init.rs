//! `init` — the orchestration called by `specify init`.
//!
//! Creates `.specify/{slices,specs,archive,.cache}/`, resolves the
//! requested capability identifier into `.specify/.cache/`, writes
//! `.specify/project.yaml` with a `rules:` key scaffolded from the
//! resolved capability's `pipeline.define` briefs, and upserts the
//! `.specify/.cache/` and `.specify/workspace/` lines into the project
//! `.gitignore`. Two calls with identical options are safe — the only
//! effect of the second call is refreshing the cache and overwriting
//! `project.yaml` with byte-identical content.
//!
//! Per RFC-13 chunk 2.9 ("Init wires components, not capabilities"),
//! `init` writes only the per-project skeleton — `project.yaml` plus
//! the `.specify/` tree. Platform-component artefacts at the repo
//! root (`registry.yaml`, `change.md`, `plan.yaml`) are
//! operator-managed: `specify registry add` mints `registry.yaml`,
//! `specify change create` mints `change.md` (RFC-13 chunk 3.7
//! renamed it from the pre-Phase-3.7 `initiative.md`), and
//! `specify change plan create` mints `plan.yaml`. Init never
//! pre-touches them.
//!
//! Hub mode (`InitOptions::hub: true`, RFC-9 §1D / RFC-13 §Migration)
//! is the one principled exception: a registry-only platform hub
//! exists *to* host a `registry.yaml`, so hub init scaffolds the
//! empty registry alongside the sentinel `project.yaml { hub: true }`
//! (with `capability:` omitted) under `.specify/`. It still does not
//! pre-write `change.md` or `plan.yaml` — those are operator
//! actions on a hub too. Hub init refuses to run when `.specify/`
//! already exists so it never clobbers a regular single-repo
//! project.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use specify_capability::{CacheMeta, PipelineView};
use specify_registry::Registry;
use specify_slice::is_valid_kebab_name;
use specify_error::Error;

use crate::config::ProjectConfig;

/// Inputs to [`init`]. Borrow-shaped so callers (the CLI and tests) can
/// build the struct without cloning path buffers.
pub struct InitOptions<'a> {
    /// Root of the project being initialised.
    pub project_dir: &'a Path,
    /// Capability identifier (bare name like `omnia` or a URL) to fetch
    /// or copy into `.specify/.cache/`. Required for regular init; must
    /// be `None` when [`InitOptions::hub`] is `true` (hubs do not
    /// resolve a capability at init time).
    pub capability: Option<&'a str>,
    /// Project name; defaults to the project directory name when `None`.
    pub name: Option<&'a str>,
    /// Optional project domain description.
    pub domain: Option<&'a str>,
    /// Controls what `specify_version` gets written into `project.yaml`.
    pub version_mode: VersionMode,
    /// When `true`, scaffold a registry-only platform **hub** (RFC-9
    /// §1D) instead of a regular project: writes `registry.yaml` at
    /// the repo root and `project.yaml { hub: true }` (with
    /// `capability:` omitted — RFC-13 §Migration "Hub project shape")
    /// under `.specify/`. Hub init refuses to run when `.specify/`
    /// already exists so it never clobbers a regular single-repo
    /// project.
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
    /// Resolved capability name from the capability root. For hub init
    /// this is the literal `"hub"` so the JSON envelope stays stable
    /// for downstream consumers.
    pub capability_name: String,
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
/// Returns an error if the operation fails. Pre-condition: regular
/// (non-hub) init requires [`InitOptions::capability`] to be set; the
/// CLI dispatcher enforces the `init-requires-capability-or-hub`
/// invariant ahead of this call, but `init` re-validates as a defence
/// in depth.
#[allow(clippy::needless_pass_by_value)]
pub fn init(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    if opts.hub {
        return init_hub(opts);
    }
    let capability = opts.capability.ok_or(Error::InitRequiresCapabilityOrHub)?;

    let name = resolved_name(opts.project_dir, opts.name);

    let mut directories_created: Vec<PathBuf> = Vec::new();
    // Per RFC-13 chunk 2.9, the `.specify/` skeleton stays here but
    // platform-component artefacts at the repo root (`registry.yaml`,
    // `change.md`, `plan.yaml`) are not pre-touched — their owning
    // verbs (`specify registry add`, `specify change create`, and
    // `specify change plan create`) mint them on demand. (The brief
    // filename moved from `initiative.md` to `change.md` in RFC-13
    // chunk 3.7.)
    // `.specify/specs/` is retained as a per-project convention used
    // by the bundled `omnia` capability; capabilities that need
    // different layouts can mint their own subdirectories from a
    // brief without core involvement.
    for dir in [
        ProjectConfig::specify_dir(opts.project_dir),
        ProjectConfig::slices_dir(opts.project_dir),
        ProjectConfig::specify_dir(opts.project_dir).join("specs"),
        ProjectConfig::archive_dir(opts.project_dir),
        ProjectConfig::cache_dir(opts.project_dir),
    ] {
        let already = dir.exists();
        fs::create_dir_all(&dir)?;
        if !already {
            directories_created.push(dir);
        }
    }

    let resolved = cache_capability(capability, opts.project_dir)?;
    let view = PipelineView::load(&resolved.capability_value, opts.project_dir)?;
    let capability_name = view.schema.schema.name.clone();
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
        capability: Some(resolved.capability_value),
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
        capability_name,
        cache_present,
        directories_created,
        scaffolded_rule_keys,
        specify_version,
    })
}

/// Sentinel value reported in [`InitResult::capability_name`] for hub
/// init. Hub `project.yaml` itself does **not** carry this string —
/// RFC-13 §Migration encodes "phase pipelines disabled" as the absence
/// of `capability:`. The constant is kept solely so the JSON envelope
/// and the text response have a stable string to display.
const HUB_INIT_NAME: &str = "hub";

/// Hub variant of [`init`] (RFC-9 §1D, RFC-13 §Migration). Scaffolds a
/// **registry-only platform hub**: the platform repo holds
/// platform-level state (`registry.yaml`, plus the operator-managed
/// `change.md`, `plan.yaml`, and `workspace/` once the operator asks
/// for them) but never appears in its own `registry.yaml` and
/// disables phase pipelines on itself by **omitting** `capability:`
/// from `project.yaml`. The post-RFC-13 hub project carries only
/// `hub: true` (no `capability:` field).
///
/// On-disk shape after success:
///
/// ```text
/// <project_dir>/
/// ├── registry.yaml     # { version: 1, projects: [] }
/// └── .specify/
///     └── project.yaml  # { name: …, hub: true }
/// ```
///
/// `registry.yaml` is the one platform-component artefact init
/// scaffolds — bootstrapping a hub *is* bootstrapping its registry.
/// `change.md` and `plan.yaml` stay operator-managed even on a hub;
/// the operator runs `specify change create <name>` and
/// `specify change plan create <name>` when the work itself begins.
/// (Pre-Phase-3.7 the brief filename was `initiative.md`; chunk 3.7
/// renamed it.)
///
/// Refuses to run when `.specify/` already exists so the operator
/// never accidentally flips an existing single-repo project into a
/// hub. Capability resolution is intentionally skipped — there is no
/// `pipeline.define` for a hub to walk.
///
/// # Errors
///
/// Returns an error if [`InitOptions::capability`] is set (mutually
/// exclusive with `--hub`), if the project name is not kebab-case, if
/// `.specify/` already exists, or if any filesystem write fails.
#[allow(clippy::needless_pass_by_value)]
fn init_hub(opts: InitOptions<'_>) -> Result<InitResult, Error> {
    if opts.capability.is_some() {
        return Err(Error::InitRequiresCapabilityOrHub);
    }

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
        name,
        domain: opts.domain.map(str::to_string),
        capability: None,
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

    upsert_gitignore(opts.project_dir)?;

    let cache_present = CacheMeta::path(opts.project_dir).exists();

    Ok(InitResult {
        config_path,
        capability_name: HUB_INIT_NAME.to_string(),
        cache_present,
        directories_created,
        scaffolded_rule_keys: Vec::new(),
        specify_version,
    })
}

#[derive(Debug)]
struct CachedCapability {
    capability_value: String,
}

fn cache_capability(capability: &str, project_dir: &Path) -> Result<CachedCapability, Error> {
    if capability.trim().is_empty() || capability != capability.trim() {
        return Err(Error::SchemaResolution(
            "<capability> must be non-empty and must not have leading or trailing whitespace"
                .to_string(),
        ));
    }

    let source = CapabilityUri::parse(capability, project_dir)?;
    let cache_dir = ProjectConfig::cache_dir(project_dir);
    let target = cache_dir.join(&source.capability_name);
    refresh_cached_capability(&source.source_dir, &target)?;
    write_cache_meta(project_dir, &source.capability_value)?;

    Ok(CachedCapability {
        capability_value: source.capability_value,
    })
}

#[derive(Debug)]
struct CapabilityUri {
    capability_value: String,
    capability_name: String,
    source_dir: PathBuf,
}

impl CapabilityUri {
    fn parse(capability: &str, project_dir: &Path) -> Result<Self, Error> {
        if is_github_url(capability) {
            return Self::from_github(capability);
        }
        Self::from_local(capability, project_dir)
    }

    fn from_local(capability: &str, project_dir: &Path) -> Result<Self, Error> {
        let path = capability
            .strip_prefix("file://")
            .map_or_else(|| PathBuf::from(capability), PathBuf::from);
        let source_dir = if path.is_absolute() { path } else { project_dir.join(path) };
        ensure_capability_dir(&source_dir, capability)?;
        let canonical = fs::canonicalize(&source_dir).map_err(|err| {
            Error::SchemaResolution(format!(
                "failed to canonicalize local capability `{capability}` at {}: {err}",
                source_dir.display()
            ))
        })?;
        let capability_name = capability_name_from_dir(&canonical)?;
        let capability_value = format!("file://{}", canonical.display());
        Ok(Self {
            capability_value,
            capability_name,
            source_dir: canonical,
        })
    }

    fn from_github(capability: &str) -> Result<Self, Error> {
        let spec = GithubCapabilityUri::parse(capability)?;
        let repo_url = format!("https://github.com/{}/{}.git", spec.owner, spec.repo);
        let checkout_dir =
            sparse_checkout_github(&repo_url, spec.checkout_ref.as_deref(), &spec.capability_path)?;
        let source_dir = checkout_dir.join(&spec.capability_path);
        ensure_capability_dir(&source_dir, capability)?;

        Ok(Self {
            capability_value: capability.to_string(),
            capability_name: spec.capability_name,
            source_dir,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct GithubCapabilityUri {
    owner: String,
    repo: String,
    checkout_ref: Option<String>,
    capability_path: String,
    capability_name: String,
}

impl GithubCapabilityUri {
    fn parse(capability: &str) -> Result<Self, Error> {
        let (without_suffix, suffix_ref) = split_ref_suffix(capability);
        let pathless = without_suffix.strip_prefix("https://github.com/").ok_or_else(|| {
            Error::SchemaResolution(format!("unsupported GitHub capability URI `{capability}`"))
        })?;
        let mut parts: Vec<&str> = pathless.split('/').filter(|part| !part.is_empty()).collect();
        if parts.len() < 3 {
            return Err(Error::SchemaResolution(format!(
                "GitHub capability URI `{capability}` must include owner, repo, and capability path"
            )));
        }
        let owner = parts.remove(0).to_string();
        let repo = parts.remove(0).to_string();

        let (tree_ref, capability_parts): (Option<&str>, Vec<&str>) = if parts.first()
            == Some(&"tree")
        {
            if parts.len() < 3 {
                return Err(Error::SchemaResolution(format!(
                    "GitHub tree capability URI `{capability}` must include a ref and capability path"
                )));
            }
            (Some(parts[1]), parts[2..].to_vec())
        } else {
            (None, parts)
        };

        let checkout_ref = suffix_ref.or(tree_ref).map(str::to_string);
        let capability_path = capability_parts.join("/");
        let capability_name = capability_parts.last().ok_or_else(|| {
            Error::SchemaResolution(format!("cannot derive a capability name from `{capability}`"))
        })?;

        Ok(Self {
            owner,
            repo,
            checkout_ref,
            capability_path,
            capability_name: (*capability_name).to_string(),
        })
    }
}

fn is_github_url(capability: &str) -> bool {
    capability.starts_with("https://github.com/")
}

fn split_ref_suffix(capability: &str) -> (&str, Option<&str>) {
    let last_slash = capability.rfind('/').unwrap_or(0);
    if let Some(at) = capability.rfind('@')
        && at > last_slash
        && at + 1 < capability.len()
    {
        return (&capability[..at], Some(&capability[at + 1..]));
    }
    (capability, None)
}

fn sparse_checkout_github(
    repo_url: &str, checkout_ref: Option<&str>, capability_path: &str,
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
    run_git(&clone_args, "clone capability repository")?;

    let checkout_dir_arg = checkout_dir.to_string_lossy().to_string();
    run_git(
        &["-C", &checkout_dir_arg, "sparse-checkout", "set", "--", capability_path],
        "sparse-checkout capability path",
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

fn ensure_capability_dir(path: &Path, original: &str) -> Result<(), Error> {
    // Pre-RFC-13 manifests still on disk used `schema.yaml`; the cache
    // has not yet flipped over to the new filename. We accept either
    // here so a freshly-resolved local capability without
    // `capability.yaml` (still common during the cut-over) keeps
    // working.
    for filename in
        [specify_capability::CAPABILITY_FILENAME, specify_capability::LEGACY_SCHEMA_FILENAME]
    {
        if path.join(filename).is_file() {
            return Ok(());
        }
    }
    Err(Error::SchemaResolution(format!(
        "capability `{original}` did not resolve to a directory with `{}` (or legacy `{}`) at {}",
        specify_capability::CAPABILITY_FILENAME,
        specify_capability::LEGACY_SCHEMA_FILENAME,
        path.display()
    )))
}

fn capability_name_from_dir(path: &Path) -> Result<String, Error> {
    path.file_name().and_then(|name| name.to_str()).map(str::to_string).ok_or_else(|| {
        Error::SchemaResolution(format!("cannot derive capability name from {}", path.display()))
    })
}

fn refresh_cached_capability(source: &Path, target: &Path) -> Result<(), Error> {
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

fn write_cache_meta(project_dir: &Path, capability_value: &str) -> Result<(), Error> {
    let meta = CacheMeta {
        schema_url: capability_value.to_string(),
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

fn upsert_gitignore(project_dir: &Path) -> Result<(), Error> {
    specify_registry::ensure_specify_gitignore_entries(project_dir)
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
            capability: Some(schema_dir.to_str().expect("schema path utf8")),
            name: Some("demo"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: false,
        }
    }

    #[test]
    fn github_capability_uri_parses_default_main() {
        let parsed = GithubCapabilityUri::parse("https://github.com/owner/repo/schemas/omnia")
            .expect("parse GitHub URI");
        assert_eq!(
            parsed,
            GithubCapabilityUri {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
                checkout_ref: None,
                capability_path: "schemas/omnia".to_string(),
                capability_name: "omnia".to_string(),
            }
        );
    }

    #[test]
    fn github_capability_uri_parses_suffix_ref() {
        let parsed = GithubCapabilityUri::parse("https://github.com/owner/repo/schemas/omnia@v1")
            .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("v1"));
        assert_eq!(parsed.capability_path, "schemas/omnia");
        assert_eq!(parsed.capability_name, "omnia");
    }

    #[test]
    fn github_capability_uri_parses_tree_ref() {
        let parsed =
            GithubCapabilityUri::parse("https://github.com/owner/repo/tree/main/schemas/omnia")
                .expect("parse GitHub URI");
        assert_eq!(parsed.checkout_ref.as_deref(), Some("main"));
        assert_eq!(parsed.capability_path, "schemas/omnia");
        assert_eq!(parsed.capability_name, "omnia");
    }

    #[test]
    fn init_creates_specify_tree() {
        let tmp = tempdir().unwrap();
        let schema_dir = omnia_schema_dir();
        let result = init(base_opts(tmp.path(), &schema_dir)).expect("init ok");

        for sub in [
            ".specify",
            ".specify/slices",
            ".specify/specs",
            ".specify/archive",
            ".specify/.cache",
        ] {
            assert!(tmp.path().join(sub).is_dir(), "expected directory {sub} to exist");
        }
        let config_path = tmp.path().join(".specify/project.yaml");
        assert!(config_path.is_file());
        assert_eq!(result.config_path, config_path);
        assert_eq!(result.capability_name, "omnia");

        // RFC-13 chunk 2.9 — non-hub init must not pre-touch any
        // platform-component artefact at the repo root. Operators
        // mint these via `specify registry add`, `specify change
        // create`, and `specify change plan create`.
        for absent in ["registry.yaml", "initiative.md", "plan.yaml", "change.md"] {
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

    fn hub_opts<'a>(project_dir: &'a Path, name: &'a str) -> InitOptions<'a> {
        InitOptions {
            project_dir,
            capability: None,
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
        assert!(project_yaml.is_file(), "project.yaml missing");
        assert!(registry_yaml.is_file(), "registry.yaml missing at repo root");

        // RFC-13 chunk 2.9 — hub init scaffolds `registry.yaml`
        // (intrinsic to the hub's purpose) but no other
        // platform-component artefact. `initiative.md` and
        // `plan.yaml` stay operator-managed even on a hub.
        for absent in ["initiative.md", "plan.yaml", "change.md"] {
            assert!(
                !tmp.path().join(absent).exists(),
                "hub init must not pre-touch `{absent}` at the repo root"
            );
        }

        // Phase-pipeline directories MUST NOT be scaffolded for a hub —
        // the absence of `capability:` (with `hub: true`) is the
        // post-RFC-13 discriminator that disables the
        // define-build-merge loop on the hub itself.
        assert!(!tmp.path().join(".specify/slices").exists());
        assert!(!tmp.path().join(".specify/specs").exists());
        assert!(!tmp.path().join(".specify/.cache").exists());

        let cfg = ProjectConfig::load(tmp.path()).expect("reload project.yaml");
        assert!(cfg.capability.is_none(), "hub project.yaml must omit capability:");
        assert!(cfg.hub, "project.yaml must carry hub: true");
        assert!(cfg.rules.is_empty(), "hubs do not scaffold rules");
        assert_eq!(cfg.name, "platform-hub");

        let on_disk = fs::read_to_string(&project_yaml).expect("read project.yaml");
        assert!(
            !on_disk.contains("capability:"),
            "hub project.yaml must omit `capability:`, got:\n{on_disk}"
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

        assert_eq!(result.capability_name, HUB_INIT_NAME);
        assert!(result.scaffolded_rule_keys.is_empty());
    }

    #[test]
    fn hub_init_refuses_when_specify_dir_exists() {
        let tmp = tempdir().unwrap();
        // Pre-create `.specify/` with arbitrary content as if a regular
        // `specify init` had already run here.
        fs::create_dir_all(tmp.path().join(".specify")).unwrap();
        fs::write(tmp.path().join(".specify/project.yaml"), "name: existing\ncapability: omnia\n")
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
        assert_eq!(on_disk, "name: existing\ncapability: omnia\n");
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

    #[test]
    fn hub_init_rejects_capability_argument() {
        // `--hub` and `<capability>` are mutually exclusive (RFC-13
        // §1.3); the orchestrator re-checks even when the CLI layer
        // already filtered.
        let tmp = tempdir().unwrap();
        let err = init(InitOptions {
            project_dir: tmp.path(),
            capability: Some("omnia"),
            name: Some("platform-hub"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: true,
        })
        .expect_err("hub + capability must error");
        assert!(matches!(err, Error::InitRequiresCapabilityOrHub), "got: {err:?}");
    }

    #[test]
    fn regular_init_rejects_missing_capability() {
        let tmp = tempdir().unwrap();
        let err = init(InitOptions {
            project_dir: tmp.path(),
            capability: None,
            name: Some("demo"),
            domain: None,
            version_mode: VersionMode::WriteCurrent,
            hub: false,
        })
        .expect_err("missing capability must error");
        assert!(matches!(err, Error::InitRequiresCapabilityOrHub), "got: {err:?}");
    }
}
