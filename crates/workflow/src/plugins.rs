//! Cursor plugin cache inspection and invalidation (RFC-30 §D2, Wave D).
//!
//! Owns the deterministic primitives the `specify plugins {doctor,
//! refresh}` commands drive: marketplace discovery, `$CURSOR_HOME`
//! detection, the cache scan under
//! `$CURSOR_HOME/plugins/cache/<name>/<plugin>/<sha>/`, expected-sha
//! resolution from the marketplace's backing git checkout, and the
//! scoped cache deletion.
//!
//! Bootstrap module: it operates on the Cursor plugin cache and the
//! marketplace manifest, never on a `.specify/` project, so nothing
//! here calls [`crate::config::ProjectConfig::load`].
//!
//! The cache layout (`…/<name>/<plugin>/<sha>/`) and the leaf-sha
//! derivation (the marketplace repo's `HEAD`, shared by every
//! relative-path plugin) are confirmed against a live Cursor install.
//! Expected-sha resolution is injected through the [`ShaResolver`]
//! trait so [`build_report`] is testable with synthetic shas, and the
//! cached-vs-expected comparison is the pure [`classify_status`].

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use specify_error::{Error, Result};
use specify_schema::{MARKETPLACE_JSON_SCHEMA, ValidationStatus, join_details, validate_value};

/// Relative path Cursor stores its plugin cache under, from `$CURSOR_HOME`.
const CACHE_SEGMENTS: &str = "plugins/cache";
/// Marketplace file name probed under `.cursor-plugin/` and the
/// XDG config dir.
const MARKETPLACE_FILE: &str = "marketplace.json";
/// Git ref an absolute-URL plugin `source` resolves against. The
/// marketplace declares no per-plugin ref today, so the `git ls-remote`
/// branch is inert; `HEAD` is the closed default.
const DEFAULT_REF: &str = "HEAD";

/// A declared marketplace plugin (`plugins[]` row).
#[derive(Debug, Clone, Deserialize)]
pub struct PluginEntry {
    /// Plugin display name; the `<plugin>` cache segment.
    pub name: String,
    /// Plugin source — a path relative to `pluginRoot` (the only shape
    /// shipping today) or a future absolute git URL.
    pub source: String,
    /// Short human-readable summary; carried through for completeness.
    #[serde(default)]
    pub description: Option<String>,
}

/// Parsed `.cursor-plugin/marketplace.json`.
///
/// Carries the fields `doctor` / `refresh` consume. Validated against
/// [`MARKETPLACE_JSON_SCHEMA`] before deserialisation, so unmentioned
/// fields (`owner`, `metadata.version`, …) are checked but dropped here.
#[derive(Debug, Clone)]
pub struct MarketplaceManifest {
    /// Top-level marketplace identifier; scopes the cache root
    /// (`$CURSOR_HOME/plugins/cache/<name>/`).
    pub name: String,
    /// `metadata.pluginRoot` — the directory plugins live under,
    /// relative to the marketplace repo root.
    pub plugin_root: String,
    /// Declared plugins, in manifest order.
    pub plugins: Vec<PluginEntry>,
}

/// Internal deserialisation shape mirroring the JSON object. The public
/// [`MarketplaceManifest`] flattens `metadata.pluginRoot` up one level.
#[derive(Deserialize)]
struct RawManifest {
    name: String,
    metadata: RawMetadata,
    plugins: Vec<PluginEntry>,
}

#[derive(Deserialize)]
struct RawMetadata {
    #[serde(rename = "pluginRoot")]
    plugin_root: String,
}

/// Per-plugin drift classification (RFC §"specify plugins doctor").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginStatus {
    /// Cached sha matches the expected sha.
    Ok,
    /// Cached sha differs from the resolvable expected sha.
    Drifted,
    /// Cache entry exists but the expected sha is unresolvable, so
    /// drift cannot be asserted (`expected-sha` is `null`).
    Present,
    /// Declared by the marketplace but no cache entry exists.
    Missing,
    /// Cache entry not declared by the marketplace.
    Extra,
}

impl PluginStatus {
    /// Stable kebab-case wire id for text rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Drifted => "drifted",
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Extra => "extra",
        }
    }
}

/// One plugin's row on the [`DoctorReport`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PluginReport {
    /// Plugin name (the `<plugin>` cache segment).
    pub name: String,
    /// Marketplace-resolved expected sha, or `None` when unresolvable.
    pub expected_sha: Option<String>,
    /// Cached leaf sha, or `None` when no cache entry exists.
    pub cached_sha: Option<String>,
    /// Drift classification.
    pub status: PluginStatus,
}

/// Status tallies on the [`DoctorReport`]. Carries `present` alongside
/// the RFC's four documented buckets so the degraded path is counted.
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Summary {
    /// Count of `ok` plugins.
    pub ok: usize,
    /// Count of `drifted` plugins.
    pub drifted: usize,
    /// Count of `present` (expected-unresolvable) plugins.
    pub present: usize,
    /// Count of `missing` plugins.
    pub missing: usize,
    /// Count of `extra` cache entries.
    pub extra: usize,
}

impl Summary {
    /// Tally the per-plugin statuses into bucket counts.
    fn tally(plugins: &[PluginReport]) -> Self {
        let mut summary = Self::default();
        for plugin in plugins {
            match plugin.status {
                PluginStatus::Ok => summary.ok += 1,
                PluginStatus::Drifted => summary.drifted += 1,
                PluginStatus::Present => summary.present += 1,
                PluginStatus::Missing => summary.missing += 1,
                PluginStatus::Extra => summary.extra += 1,
            }
        }
        summary
    }
}

/// Wire-stable `specify plugins doctor` envelope (text + JSON). Change
/// G's `/spec:init` skill parses this from `--format json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DoctorReport {
    /// Schema marker; `1` for this shape.
    pub version: u32,
    /// Absolute path to the resolved marketplace file.
    pub marketplace: PathBuf,
    /// Absolute path to the cache root
    /// (`$CURSOR_HOME/plugins/cache/<name>`).
    pub cache_root: PathBuf,
    /// Per-plugin rows: declared plugins in manifest order, then any
    /// `extra` cache entries sorted by name.
    pub plugins: Vec<PluginReport>,
    /// Status tallies.
    pub summary: Summary,
}

/// Outcome of a `specify plugins refresh`.
///
/// The deleted cache scope and the marketplace that scoped it. Drives
/// the `plugins.refreshed` journal event and the command's confirmation
/// text.
#[derive(Debug, Clone)]
pub struct RefreshOutcome {
    /// Absolute path to the resolved marketplace file.
    pub marketplace: PathBuf,
    /// Absolute path to the cache root that was (or would be) deleted.
    pub cache_root: PathBuf,
    /// Cache directories actually removed; empty when nothing existed.
    pub deleted_paths: Vec<PathBuf>,
}

/// Resolve the expected sha for a marketplace plugin.
///
/// Injected so [`build_report`] can be unit-tested with synthetic shas.
/// The live implementation is [`GitCli`].
pub trait ShaResolver {
    /// Resolve `HEAD` of the git worktree at `repo_dir` — the shared
    /// expected sha for every relative-path plugin. `None` when
    /// `repo_dir` is not a git checkout or the ref cannot resolve.
    fn head(&self, repo_dir: &Path) -> Option<String>;

    /// Resolve `git_ref` on the remote `url` — the future absolute-URL
    /// plugin path. `None` on any failure. Inert today: no shipping
    /// `source` is a URL.
    fn ls_remote(&self, url: &str, git_ref: &str) -> Option<String>;
}

/// Live [`ShaResolver`] shelling out to `git`.
#[derive(Debug, Clone, Copy)]
pub struct GitCli;

impl ShaResolver for GitCli {
    fn head(&self, repo_dir: &Path) -> Option<String> {
        let output =
            crate::cmd::git(&crate::cmd::real_cmd, Some(repo_dir), ["rev-parse", "HEAD"]).ok()?;
        first_sha(&output)
    }

    fn ls_remote(&self, url: &str, git_ref: &str) -> Option<String> {
        let output =
            crate::cmd::git(&crate::cmd::real_cmd, None, ["ls-remote", url, git_ref]).ok()?;
        first_sha(&output)
    }
}

/// Read the leading sha token from a successful `git` invocation's
/// stdout (`rev-parse` prints the sha alone; `ls-remote` prints
/// `<sha>\t<ref>`).
fn first_sha(output: &std::process::Output) -> Option<String> {
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout.clone()).ok()?;
    let token = text.split_whitespace().next()?;
    if token.is_empty() { None } else { Some(token.to_string()) }
}

/// Resolve the marketplace file path (RFC §"Marketplace discovery").
///
/// First hit wins: an explicit `--marketplace <path>`, then
/// `<project_dir>/.cursor-plugin/marketplace.json`, then
/// `$XDG_CONFIG_HOME/cursor/marketplace.json`.
///
/// # Errors
///
/// - [`Error::Diag`] `marketplace-flag-missing` when an explicit flag
///   path does not exist.
/// - [`Error::Diag`] `marketplace-not-found` when no candidate resolves.
pub fn discover_marketplace(flag: Option<&Path>, project_dir: &Path) -> Result<PathBuf> {
    if let Some(path) = flag {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(Error::Diag {
            code: "marketplace-flag-missing",
            detail: format!("--marketplace path does not exist: {}", path.display()),
        });
    }
    let project_candidate = project_dir.join(".cursor-plugin").join(MARKETPLACE_FILE);
    if project_candidate.is_file() {
        return Ok(project_candidate);
    }
    if let Some(xdg) = xdg_marketplace()
        && xdg.is_file()
    {
        return Ok(xdg);
    }
    Err(Error::Diag {
        code: "marketplace-not-found",
        detail: format!(
            "no marketplace.json found; looked for {}, then $XDG_CONFIG_HOME/cursor/{MARKETPLACE_FILE}",
            project_candidate.display()
        ),
    })
}

/// `$XDG_CONFIG_HOME/cursor/marketplace.json` (or `~/.config/...` when
/// `XDG_CONFIG_HOME` is unset), when a config or home dir is known.
fn xdg_marketplace() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir()?.join(".config"),
    };
    Some(base.join("cursor").join(MARKETPLACE_FILE))
}

/// Load and schema-validate a `marketplace.json` file.
///
/// # Errors
///
/// - [`Error::Io`] when the file cannot be read.
/// - [`Error::Diag`] `marketplace-parse-failed` when the file is not
///   valid JSON.
/// - [`Error::Validation`] `marketplace-schema` when the document fails
///   [`MARKETPLACE_JSON_SCHEMA`].
pub fn load_marketplace(path: &Path) -> Result<MarketplaceManifest> {
    let raw = fs::read_to_string(path).map_err(Error::Io)?;
    let value: JsonValue = serde_json::from_str(&raw).map_err(|err| Error::Diag {
        code: "marketplace-parse-failed",
        detail: format!("{}: not valid JSON: {err}", path.display()),
    })?;
    let failures: Vec<_> = validate_value(
        &value,
        MARKETPLACE_JSON_SCHEMA,
        "marketplace-schema",
        "marketplace manifest",
    )
    .into_iter()
    .filter(|summary| summary.status == ValidationStatus::Fail)
    .collect();
    if !failures.is_empty() {
        return Err(Error::Validation {
            code: "marketplace-schema".to_string(),
            detail: join_details(&failures),
        });
    }
    let raw_manifest: RawManifest = serde_json::from_value(value).map_err(|err| Error::Diag {
        code: "marketplace-parse-failed",
        detail: format!("{}: unexpected marketplace shape: {err}", path.display()),
    })?;
    Ok(MarketplaceManifest {
        name: raw_manifest.name,
        plugin_root: raw_manifest.metadata.plugin_root,
        plugins: raw_manifest.plugins,
    })
}

/// Resolve `$CURSOR_HOME`.
///
/// The `CURSOR_HOME` env override (when set and non-empty), else
/// `~/.cursor`. The default matches Cursor's own layout on macOS and
/// Linux; `CURSOR_HOME` covers Windows and non-standard installs.
///
/// # Errors
///
/// [`Error::Diag`] `cursor-home-unresolved` when neither `CURSOR_HOME`
/// nor a home directory can be determined.
pub fn cursor_home() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("CURSOR_HOME")
        && !value.is_empty()
    {
        return Ok(PathBuf::from(value));
    }
    home_dir().map(|home| home.join(".cursor")).ok_or_else(|| Error::Diag {
        code: "cursor-home-unresolved",
        detail: "could not determine $CURSOR_HOME; set CURSOR_HOME or a HOME directory".to_string(),
    })
}

/// Cache root for a marketplace: `$CURSOR_HOME/plugins/cache/<name>`.
#[must_use]
pub fn cache_root(cursor_home: &Path, marketplace_name: &str) -> PathBuf {
    cursor_home.join(CACHE_SEGMENTS).join(marketplace_name)
}

/// Best-effort home directory via `HOME` (unix) or `USERPROFILE`
/// (windows). The env vars suffice for cache discovery; no `home`
/// crate dependency.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Classify a plugin from its cached and expected shas.
///
/// The pure drift kernel (RFC §"specify plugins doctor"); `Extra` is
/// decided separately by [`build_report`] (a cache dir not declared by
/// the marketplace).
#[must_use]
pub fn classify_status(cached: Option<&str>, expected: Option<&str>) -> PluginStatus {
    match (cached, expected) {
        (None, _) => PluginStatus::Missing,
        (Some(_), None) => PluginStatus::Present,
        (Some(cached), Some(expected)) if cached == expected => PluginStatus::Ok,
        (Some(_), Some(_)) => PluginStatus::Drifted,
    }
}

/// Resolve a plugin's expected sha: a relative-path `source` shares the
/// marketplace repo's `HEAD`; an absolute-URL `source` resolves via
/// `git ls-remote` against the default `HEAD` ref.
#[must_use]
pub fn expected_sha(
    manifest_dir: &Path, entry: &PluginEntry, resolver: &dyn ShaResolver,
) -> Option<String> {
    if is_url(&entry.source) {
        resolver.ls_remote(&entry.source, DEFAULT_REF)
    } else {
        resolver.head(manifest_dir)
    }
}

/// A plugin `source` is an absolute git URL (rather than a same-repo
/// relative path) when it carries a scheme or an `scp`-style git host.
fn is_url(source: &str) -> bool {
    source.contains("://") || source.starts_with("git@")
}

/// Cached leaf sha for one `<plugin>` cache directory.
///
/// The leaf is the single `<sha>` subdirectory. Returns `None` when the
/// plugin dir holds no subdirectory (a `missing`-equivalent stub). When
/// more than one leaf is present — unusual; Cursor keeps a single
/// resolved sha — the lexicographically first is reported, leaving the
/// entry `present`/`ok`/`drifted` rather than inventing a verdict.
fn leaf_sha(plugin_dir: &Path) -> Result<Option<String>> {
    let mut leaves: Vec<String> = Vec::new();
    for entry in fs::read_dir(plugin_dir).map_err(Error::Io)? {
        let entry = entry.map_err(Error::Io)?;
        if entry.file_type().map_err(Error::Io)?.is_dir() {
            leaves.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    leaves.sort();
    Ok(leaves.into_iter().next())
}

/// Scan the cache root into `<plugin> -> Option<cached-sha>`. A missing
/// cache root is an empty map (every declared plugin reads `missing`).
fn scan_cache(cache_root: &Path) -> Result<BTreeMap<String, Option<String>>> {
    let mut cache = BTreeMap::new();
    let entries = match fs::read_dir(cache_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(cache),
        Err(err) => return Err(Error::Io(err)),
    };
    for entry in entries {
        let entry = entry.map_err(Error::Io)?;
        if !entry.file_type().map_err(Error::Io)?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let sha = leaf_sha(&entry.path())?;
        cache.insert(name, sha);
    }
    Ok(cache)
}

/// Build the [`DoctorReport`] — the pure-ish core of `doctor`.
///
/// Scans `cache_root`, resolves each declared plugin's expected sha via
/// `resolver`, cross-references the cache, and appends any `extra`
/// entries.
///
/// `marketplace_path`'s parent directory is the marketplace repo the
/// relative-path resolver runs `git -C` against.
///
/// # Errors
///
/// [`Error::Io`] when the cache root or a plugin directory cannot be
/// read.
pub fn build_report(
    marketplace_path: &Path, manifest: &MarketplaceManifest, cache_root: &Path,
    resolver: &dyn ShaResolver,
) -> Result<DoctorReport> {
    let manifest_dir = marketplace_path.parent().unwrap_or_else(|| Path::new("."));
    let cache = scan_cache(cache_root)?;

    let mut declared: BTreeSet<&str> = BTreeSet::new();
    let mut plugins = Vec::with_capacity(manifest.plugins.len());
    for entry in &manifest.plugins {
        declared.insert(entry.name.as_str());
        let cached = cache.get(&entry.name).cloned().flatten();
        let expected = expected_sha(manifest_dir, entry, resolver);
        let status = classify_status(cached.as_deref(), expected.as_deref());
        plugins.push(PluginReport {
            name: entry.name.clone(),
            expected_sha: expected,
            cached_sha: cached,
            status,
        });
    }
    for (name, cached) in &cache {
        if !declared.contains(name.as_str()) {
            plugins.push(PluginReport {
                name: name.clone(),
                expected_sha: None,
                cached_sha: cached.clone(),
                status: PluginStatus::Extra,
            });
        }
    }

    let summary = Summary::tally(&plugins);
    Ok(DoctorReport {
        version: 1,
        marketplace: marketplace_path.to_path_buf(),
        cache_root: cache_root.to_path_buf(),
        plugins,
        summary,
    })
}

/// Delete the marketplace-scoped cache root, returning the removed
/// paths for the journal. A missing cache root is a no-op (empty
/// `deleted_paths`); the refresh still succeeds.
///
/// # Errors
///
/// [`Error::Io`] when the directory exists but cannot be removed.
pub fn refresh(marketplace_path: &Path, cache_root: &Path) -> Result<RefreshOutcome> {
    let mut deleted_paths = Vec::new();
    match fs::remove_dir_all(cache_root) {
        Ok(()) => deleted_paths.push(cache_root.to_path_buf()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(Error::Io(err)),
    }
    Ok(RefreshOutcome {
        marketplace: marketplace_path.to_path_buf(),
        cache_root: cache_root.to_path_buf(),
        deleted_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake resolver returning canned shas, so status computation is
    /// driven without a real git checkout.
    struct FakeResolver {
        head: Option<String>,
        remote: Option<String>,
    }

    impl ShaResolver for FakeResolver {
        fn head(&self, _repo_dir: &Path) -> Option<String> {
            self.head.clone()
        }

        fn ls_remote(&self, _url: &str, _git_ref: &str) -> Option<String> {
            self.remote.clone()
        }
    }

    #[test]
    fn classify_matches_ok() {
        assert_eq!(classify_status(Some("abc"), Some("abc")), PluginStatus::Ok);
    }

    #[test]
    fn classify_differs_drifted() {
        assert_eq!(classify_status(Some("abc"), Some("def")), PluginStatus::Drifted);
    }

    #[test]
    fn classify_unresolvable_present() {
        assert_eq!(classify_status(Some("abc"), None), PluginStatus::Present);
    }

    #[test]
    fn classify_no_cache_missing() {
        assert_eq!(classify_status(None, Some("abc")), PluginStatus::Missing);
        assert_eq!(classify_status(None, None), PluginStatus::Missing);
    }

    #[test]
    fn is_url_distinguishes_paths_from_remotes() {
        assert!(!is_url("spec"));
        assert!(is_url("https://github.com/augentic/specify"));
        assert!(is_url("git@github.com:augentic/specify.git"));
    }

    /// Build a `<name>/<plugin>/<sha>/` cache tree plus a sibling
    /// marketplace.json under a tempdir, returning the marketplace path.
    fn fixture(tmp: &Path, name: &str, leaves: &[(&str, Option<&str>)]) -> (PathBuf, PathBuf) {
        let root = cache_root(tmp, name);
        for (plugin, sha) in leaves {
            let plugin_dir = root.join(plugin);
            fs::create_dir_all(&plugin_dir).unwrap();
            if let Some(sha) = sha {
                fs::create_dir_all(plugin_dir.join(sha)).unwrap();
            }
        }
        let marketplace = tmp.join("marketplace.json");
        fs::write(&marketplace, "{}").unwrap();
        (marketplace, root)
    }

    fn manifest(name: &str, plugins: &[(&str, &str)]) -> MarketplaceManifest {
        MarketplaceManifest {
            name: name.to_string(),
            plugin_root: "plugins".to_string(),
            plugins: plugins
                .iter()
                .map(|(n, s)| PluginEntry {
                    name: (*n).to_string(),
                    source: (*s).to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    #[test]
    fn report_flags_missing_extra_and_present() {
        let tmp = tempfile::tempdir().unwrap();
        // `spec` declared with a cache leaf; `omnia` is an undeclared
        // extra; `client` is declared with no cache leaf.
        let (marketplace, root) =
            fixture(tmp.path(), "augentic", &[("spec", Some("cafe")), ("omnia", Some("beef"))]);
        let mani = manifest("augentic", &[("spec", "spec"), ("client", "client")]);
        // Expected unresolvable -> declared+cached collapses to present.
        let resolver = FakeResolver {
            head: None,
            remote: None,
        };

        let report = build_report(&marketplace, &mani, &root, &resolver).unwrap();

        let by_name = |n: &str| report.plugins.iter().find(|p| p.name == n).unwrap().status;
        assert_eq!(by_name("spec"), PluginStatus::Present, "cached but expected unresolvable");
        assert_eq!(by_name("client"), PluginStatus::Missing, "declared, no cache leaf");
        assert_eq!(by_name("omnia"), PluginStatus::Extra, "cached, not declared");
        assert_eq!(report.summary.present, 1);
        assert_eq!(report.summary.missing, 1);
        assert_eq!(report.summary.extra, 1);
    }

    #[test]
    fn report_drifted_and_ok_with_resolved_head() {
        let tmp = tempfile::tempdir().unwrap();
        let (marketplace, root) =
            fixture(tmp.path(), "augentic", &[("spec", Some("oldsha")), ("client", Some("head"))]);
        let mani = manifest("augentic", &[("spec", "spec"), ("client", "client")]);
        // Relative-path sources share the resolved HEAD.
        let resolver = FakeResolver {
            head: Some("head".to_string()),
            remote: None,
        };

        let report = build_report(&marketplace, &mani, &root, &resolver).unwrap();

        let by_name = |n: &str| report.plugins.iter().find(|p| p.name == n).unwrap();
        assert_eq!(by_name("spec").status, PluginStatus::Drifted);
        assert_eq!(by_name("spec").expected_sha.as_deref(), Some("head"));
        assert_eq!(by_name("client").status, PluginStatus::Ok);
        assert_eq!(report.summary.ok, 1);
        assert_eq!(report.summary.drifted, 1);
    }

    #[test]
    fn report_missing_cache_root_is_all_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let marketplace = tmp.path().join("marketplace.json");
        fs::write(&marketplace, "{}").unwrap();
        let root = cache_root(tmp.path(), "augentic");
        let mani = manifest("augentic", &[("spec", "spec")]);
        let resolver = FakeResolver {
            head: Some("head".to_string()),
            remote: None,
        };

        let report = build_report(&marketplace, &mani, &root, &resolver).unwrap();
        assert_eq!(report.plugins[0].status, PluginStatus::Missing);
        assert_eq!(report.summary.missing, 1);
    }

    #[test]
    fn refresh_deletes_only_scoped_root() {
        let tmp = tempfile::tempdir().unwrap();
        let (marketplace, root) = fixture(tmp.path(), "augentic", &[("spec", Some("cafe"))]);
        let (_, other) = fixture(tmp.path(), "acme", &[("widget", Some("beef"))]);

        let outcome = refresh(&marketplace, &root).unwrap();
        assert_eq!(outcome.deleted_paths.len(), 1);
        assert!(!root.exists(), "scoped cache removed");
        assert!(other.exists(), "sibling marketplace cache survives");
    }

    #[test]
    fn refresh_missing_root_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let marketplace = tmp.path().join("marketplace.json");
        let root = cache_root(tmp.path(), "augentic");
        let outcome = refresh(&marketplace, &root).unwrap();
        assert!(outcome.deleted_paths.is_empty());
    }

    #[test]
    fn load_marketplace_parses_and_validates() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.json");
        fs::write(
            &path,
            r#"{
              "name": "augentic",
              "owner": { "name": "augentic", "email": "info@augentic.io" },
              "metadata": { "description": "d", "version": "0.27.0", "pluginRoot": "plugins" },
              "plugins": [ { "name": "spec", "source": "spec", "description": "Spec skills." } ]
            }"#,
        )
        .unwrap();
        let manifest = load_marketplace(&path).unwrap();
        assert_eq!(manifest.name, "augentic");
        assert_eq!(manifest.plugin_root, "plugins");
        assert_eq!(manifest.plugins[0].source, "spec");
    }

    #[test]
    fn marketplace_rejects_schema_violation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("marketplace.json");
        fs::write(&path, r#"{ "name": "augentic" }"#).unwrap();
        let err = load_marketplace(&path).expect_err("missing required fields");
        assert_eq!(err.variant_str(), "marketplace-schema");
    }

    #[test]
    fn discover_prefers_flag_then_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let cursor_plugin = project.join(".cursor-plugin");
        fs::create_dir_all(&cursor_plugin).unwrap();
        let project_file = cursor_plugin.join("marketplace.json");
        fs::write(&project_file, "{}").unwrap();

        // No flag -> project hit.
        let found = discover_marketplace(None, &project).unwrap();
        assert_eq!(found, project_file);

        // Flag overrides.
        let flag = tmp.path().join("custom.json");
        fs::write(&flag, "{}").unwrap();
        let found = discover_marketplace(Some(&flag), &project).unwrap();
        assert_eq!(found, flag);
    }

    #[test]
    fn discover_missing_flag_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = discover_marketplace(Some(&tmp.path().join("nope.json")), tmp.path())
            .expect_err("missing flag path");
        assert_eq!(err.variant_str(), "marketplace-flag-missing");
    }
}
