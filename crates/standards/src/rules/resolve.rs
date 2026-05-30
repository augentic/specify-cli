//! Codex resolver (CH-12): roots + overlay discovery.
//!
//! Implements rules root resolution and codex root discovery
//! / §"Resolution inputs" / §"Overlay precedence". This chunk discovers
//! and parses every rule the export envelope eventually carries.
//!
//! Filtering by applicability and deprecation lives in the sibling
//! [`filter`] module (CH-13); stable export ordering (CH-14) and
//! findings (CH-15/16) remain out of scope here.
//!
//! # Closed precedence order
//!
//! Shared root (root 1) resolves from a **rules root**, picked by the
//! closed probe in codex root resolution:
//!
//! 1. `inputs.rules_root` when supplied — use for root 1 and the
//!    rules-root fallback overlay (overlay step 3 below).
//! 2. Else if `{project_dir}/adapters/shared/rules/universal/` exists,
//!    treat `project_dir` as the rules root (monorepo case). In this
//!    case the rules-root fallback overlay step is **skipped** —
//!    re-walking `project_dir` would just shadow the project-local
//!    rung with the same filesystem tree.
//! 3. Else if the distributed codex cache
//!    `{project_dir}/.specify/.cache/codex/adapters/shared/rules/universal/`
//!    exists, treat `{project_dir}/.specify/.cache/codex/` as the rules
//!    root. Populated by codex distribution (RM-07) at `specrun init`
//!    or `specrun rules sync`. Like the monorepo case this is a derived
//!    (non-explicit) root, so the rules-root fallback overlay step is
//!    **skipped**.
//! 4. Else → [`ResolveError::RulesRootRequired`].
//!
//! Source-adapter (root 3) and target-adapter (root 4) overlays follow
//! the closed location order in rules root resolution:
//!
//! 1. project-local `{project_dir}/adapters/{sources,targets}/<name>/rules/`;
//! 2. manifest cache `{project_dir}/.specify/.cache/manifests/{sources,targets}/<name>/rules/`;
//! 3. rules-root fallback `{rules_root}/adapters/{sources,targets}/<name>/rules/`,
//!    **only** when `inputs.rules_root.is_some()` (step 1 of the probe);
//! 4. omit when no rung exists.
//!
//! The **first existing** rung wins; locations never merge. Roots 2
//! (shared language/artifact packs) and 5 (project-local overlays) are
//! reserved by the rules contract and not implemented here. The closed
//! [`Origin`] enum's `Unknown` variant is **not** root 5 — it is the
//! consumer indexer's fallback bucket for cache rule files whose path
//! does not match a recognized adapter shape (see the `infer_origin`
//! function in [`crate::lint::index`]).
//!
//! # Duplicate ids
//!
//! Per overlay precedence: rules never override each other
//! by sharing ids — duplicates always error, regardless of
//! `include_deprecated`. The check runs after every rung is loaded so
//! collisions across overlays surface as
//! [`ResolveError::DuplicateRuleId`].
//!
//! # Out of scope
//!
//! - Applicability + deprecation filtering — see the sibling
//!   [`filter`] module (CH-13). [`resolve`] returns the unfiltered pool;
//!   call [`filter`] on that result to get the narrowed view.
//! - Stable export ordering — CH-14 lives in the sibling `sort`
//!   module. CH-12 only enforces deterministic intra-directory lexical
//!   order so test goldens stay stable; the closed four-tuple sort and
//!   the [`super::ResolvedRules`] envelope are assembled by
//!   [`build_resolved_rules`].

mod filter;
mod sort;

use std::path::{Path, PathBuf};
use std::{fs, io};

pub use filter::filter;
pub use sort::{build_resolved_rules, sort_resolved};
use specify_error::Error;

use super::parse::{ParseError, parse_rule_file};
use super::{Origin, PathRoot, Rule};

/// Closed input contract for [`resolve`] and [`filter`] per the rules contract
/// §"Resolution inputs".
///
/// CH-12's [`resolve`] consumes `project_dir`, `rules_root`,
/// `target_adapter`, and `source_adapters`. CH-13's [`filter`]
/// additionally consumes `artifact_paths`, `languages`,
/// `include_deprecated`, `include_unmatched`, and `include_core`.
/// Callers compose [`resolve`] then [`filter`] when they need the
/// narrowed pool; [`build_resolved_rules`] is the export entry point.
#[derive(Debug, Clone)]
pub struct ResolveInputs<'a> {
    /// Project root used for adapter resolution and optional
    /// project-local overlays.
    pub project_dir: &'a Path,
    /// Root containing first-party codex content for shared rules and
    /// rules-root fallback overlays. See §"Codex root resolution (v1)".
    pub rules_root: Option<&'a Path>,
    /// Target adapter name (optionally `<name>@v<major>`; resolver
    /// uses the raw name as the directory segment).
    pub target_adapter: &'a str,
    /// Source adapter names bound by the active plan entry.
    pub source_adapters: &'a [String],
    /// Project-relative artifact paths consumed by CH-13's
    /// `applicability.paths` glob check.
    pub artifact_paths: &'a [PathBuf],
    /// Language tokens consumed by CH-13's `applicability.languages`
    /// match.
    pub languages: &'a [String],
    /// Whether deprecated rules appear in the export. Toggled by CH-13.
    pub include_deprecated: bool,
    /// Whether rules with populated applicability dimensions the
    /// caller did not satisfy are included. Toggled by CH-13.
    pub include_unmatched: bool,
    /// Whether rules with [`super::Origin::Core`] appear in the
    /// export. Default off (consumer-export filtering): consumer-project
    /// review runs never evaluate `CORE-*` hints by accident.
    pub include_core: bool,
}

/// Pre-sort intermediate emitted by [`resolve`].
///
/// CH-14 owns the final sorted [`super::ResolvedRules`]; CH-12 returns
/// every discovered rule with `origin`, `path_root`, and `path`
/// populated. The `path` string is always relative to the matching
/// [`PathRoot`], with forward slashes as separators.
#[derive(Debug, Clone)]
pub struct ResolvedRuleEntry {
    /// Parsed rule from CH-11.
    pub rule: Rule,
    /// Resolver origin tier (shared / source / target).
    pub origin: Origin,
    /// Anchor for [`Self::path`] — rules-root vs project-dir.
    pub path_root: PathRoot,
    /// Path to the rule markdown file relative to `path_root`.
    pub path: String,
}

/// Failure modes for [`resolve`].
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// Shared probe failed: no `--rules-root`, no monorepo fallback
    /// under `project_dir`, and no distributed codex cache. Wire-id
    /// `rules-root-required` per codex root resolution and the §490
    /// golden.
    #[error(
        "rules-root-required: shared UNI-* rules require --rules-root pointing at a tree containing adapters/shared/rules/universal/, a monorepo adapters/shared/rules/universal/ tree, or a distributed codex cache (run `specrun rules sync`)"
    )]
    RulesRootRequired,
    /// A rule id appeared in more than one discovered file. Per
    /// overlay precedence this is always invalid.
    #[error("duplicate rule id '{id}' across files: {paths}")]
    DuplicateRuleId {
        /// The colliding rule id.
        id: String,
        /// Comma-joined relative paths of every offending file.
        paths: String,
    },
    /// CH-11 parser rejected one of the discovered files.
    #[error("rule parse failed: {path}: {error}")]
    Parse {
        /// Absolute path of the failing file.
        path: PathBuf,
        /// Underlying parser error.
        error: ParseError,
    },
    /// `read_dir` / `read_to_string` failed while walking a rung.
    #[error("rule discovery failed: {path}: {source}")]
    Filesystem {
        /// Absolute path of the failing directory or file.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
}

/// Translate the resolver's typed [`ResolveError`] onto the closed
/// [`specify_error::Error`] enum.
///
/// `Exit::from(&Error)` then picks the right exit code per
/// `docs/standards/handler-shape.md`: the three codex-shape failures
/// map to `Validation` (exit 2) and the I/O failure maps to
/// `Filesystem` (exit 1).
#[must_use]
pub fn map_resolve_error(err: ResolveError) -> Error {
    match err {
        ResolveError::RulesRootRequired => Error::validation_failed(
            "rules-root-required",
            "shared UNI-* rules require --rules-root, a project-local \
             adapters/shared/rules/universal/ tree, or a distributed \
             codex cache under .specify/.cache/codex/",
            "run `specrun rules sync` to distribute the shared codex, or \
             pass --rules-root pointing at a tree containing \
             adapters/shared/rules/universal/",
        ),
        ResolveError::DuplicateRuleId { id, paths } => Error::validation_failed(
            "rules-duplicate-rule-id",
            format!("rule id '{id}' appears in multiple files"),
            paths,
        ),
        ResolveError::Parse { path, error } => Error::validation_failed(
            "rules-parse-error",
            format!("failed to parse rule {}", path.display()),
            error.to_string(),
        ),
        ResolveError::Filesystem { path, source } => Error::Filesystem {
            op: "readdir",
            path,
            source,
        },
    }
}

const SHARED_REL: &str = "adapters/shared/rules/universal";
const CORE_REL: &str = "adapters/shared/rules/core";
const MANIFEST_CACHE_REL: &str = ".specify/.cache/manifests";
/// Project codex cache root populated by codex distribution (RM-07).
/// Probe step 3 treats it as a derived rules root when it carries the
/// shared `universal/` pack. Kept in lockstep with
/// `specify_workflow::init::codex_cache_root`.
const CODEX_CACHE_REL: &str = ".specify/.cache/codex";

/// Discover every rule visible to `inputs` and parse it.
///
/// See the module docs for the closed precedence order, rules-root
/// probe, and duplicate-id rules. Filtering and sorting are deferred
/// to CH-13 / CH-14.
///
/// # Errors
///
/// Returns the matching [`ResolveError`] variant for each failure
/// mode; see the variant docs.
pub fn resolve(inputs: &ResolveInputs<'_>) -> Result<Vec<ResolvedRuleEntry>, ResolveError> {
    let rules_root = probe_rules_root(inputs)?;
    let explicit_rules_root = inputs.rules_root.is_some();

    let mut entries: Vec<ResolvedRuleEntry> = Vec::new();

    let shared_dir = rules_root.join(SHARED_REL);
    if shared_dir.is_dir() {
        for path in list_rule_files(&shared_dir)? {
            let rel = relative_path(&rules_root, &path);
            let rule = parse(&path)?;
            entries.push(ResolvedRuleEntry {
                rule,
                origin: Origin::Shared,
                path_root: PathRoot::RulesRoot,
                path: rel,
            });
        }
    }

    let core_dir = rules_root.join(CORE_REL);
    if core_dir.is_dir() {
        for path in list_rule_files(&core_dir)? {
            let rel = relative_path(&rules_root, &path);
            let rule = parse(&path)?;
            entries.push(ResolvedRuleEntry {
                rule,
                origin: Origin::Core,
                path_root: PathRoot::RulesRoot,
                path: rel,
            });
        }
    }

    for source_name in inputs.source_adapters {
        load_overlay(
            inputs.project_dir,
            &rules_root,
            explicit_rules_root,
            "sources",
            source_name,
            Origin::Source,
            &mut entries,
        )?;
    }

    load_overlay(
        inputs.project_dir,
        &rules_root,
        explicit_rules_root,
        "targets",
        inputs.target_adapter,
        Origin::Target,
        &mut entries,
    )?;

    detect_duplicates(&entries)?;

    Ok(entries)
}

/// Pick the rules root from the §"Codex root resolution (v1)" probe.
fn probe_rules_root(inputs: &ResolveInputs<'_>) -> Result<PathBuf, ResolveError> {
    if let Some(explicit) = inputs.rules_root {
        return Ok(explicit.to_path_buf());
    }
    let project_shared = inputs.project_dir.join(SHARED_REL);
    if project_shared.is_dir() {
        return Ok(inputs.project_dir.to_path_buf());
    }
    let codex_cache = inputs.project_dir.join(CODEX_CACHE_REL);
    if codex_cache.join(SHARED_REL).is_dir() {
        return Ok(codex_cache);
    }
    Err(ResolveError::RulesRootRequired)
}

/// Walk overlay rungs for one adapter and append discovered rules.
///
/// `axis_segment` is the literal directory name (`"sources"` or
/// `"targets"`). The first existing rung wins per the rules contract §"Resolution
/// roots".
fn load_overlay(
    project_dir: &Path, rules_root: &Path, explicit_rules_root: bool, axis_segment: &str,
    adapter_name: &str, origin: Origin, out: &mut Vec<ResolvedRuleEntry>,
) -> Result<(), ResolveError> {
    let project_local =
        project_dir.join("adapters").join(axis_segment).join(adapter_name).join("rules");
    let manifest_cache =
        project_dir.join(MANIFEST_CACHE_REL).join(axis_segment).join(adapter_name).join("rules");

    if project_local.is_dir() {
        return collect_overlay(&project_local, project_dir, PathRoot::ProjectDir, origin, out);
    }
    if manifest_cache.is_dir() {
        return collect_overlay(&manifest_cache, project_dir, PathRoot::ProjectDir, origin, out);
    }
    if explicit_rules_root {
        let fallback =
            rules_root.join("adapters").join(axis_segment).join(adapter_name).join("rules");
        if fallback.is_dir() {
            return collect_overlay(&fallback, rules_root, PathRoot::RulesRoot, origin, out);
        }
    }
    Ok(())
}

/// Parse every `.md` file under `dir` and append to `out`.
fn collect_overlay(
    dir: &Path, path_root_dir: &Path, path_root: PathRoot, origin: Origin,
    out: &mut Vec<ResolvedRuleEntry>,
) -> Result<(), ResolveError> {
    for path in list_rule_files(dir)? {
        let rel = relative_path(path_root_dir, &path);
        let rule = parse(&path)?;
        out.push(ResolvedRuleEntry {
            rule,
            origin,
            path_root,
            path: rel,
        });
    }
    Ok(())
}

/// Non-recursive `.md` listing for a codex directory, excluding
/// `README.md` (case-insensitive) and symlinks.
///
/// Results are sorted lexically by absolute path so per-directory
/// iteration order is deterministic for goldens.
fn list_rule_files(dir: &Path) -> Result<Vec<PathBuf>, ResolveError> {
    let entries = fs::read_dir(dir).map_err(|source| ResolveError::Filesystem {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ResolveError::Filesystem {
            path: dir.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| ResolveError::Filesystem {
            path: entry.path(),
            source,
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.to_ascii_lowercase().ends_with(".md") {
            continue;
        }
        if name_str.eq_ignore_ascii_case("README.md") {
            continue;
        }
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

fn parse(path: &Path) -> Result<Rule, ResolveError> {
    parse_rule_file(path).map_err(|error| ResolveError::Parse {
        path: path.to_path_buf(),
        error,
    })
}

/// Compute `path` relative to `root` and normalise separators to `/`.
///
/// Falls back to the absolute path string if `strip_prefix` fails (in
/// practice this only happens when callers pass a path outside the
/// claimed root, which `list_rule_files` never does).
fn relative_path(root: &Path, path: &Path) -> String {
    let stripped = path.strip_prefix(root).unwrap_or(path);
    let display = stripped.to_string_lossy().into_owned();
    if cfg!(windows) { display.replace('\\', "/") } else { display }
}

/// Per overlay precedence: duplicates always error.
fn detect_duplicates(entries: &[ResolvedRuleEntry]) -> Result<(), ResolveError> {
    use std::collections::BTreeMap;

    let mut by_id: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for entry in entries {
        by_id.entry(entry.rule.id.as_str()).or_default().push(entry.path.as_str());
    }
    for (id, paths) in by_id {
        if paths.len() > 1 {
            return Err(ResolveError::DuplicateRuleId {
                id: id.to_string(),
                paths: paths.join(", "),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;

    /// Minimal frontmatter + body that satisfies the CH-11 parser and
    /// the codex-rule schema. The shared `id` namespace varies by
    /// caller; a 30+ char trigger keeps schema validation happy.
    fn rule_markdown(id: &str, title: &str) -> String {
        format!(
            "---\nid: {id}\ntitle: {title}\nseverity: important\ntrigger: Synthetic CH-12 resolver fixture trigger sentence long enough for schema.\n---\n\n## Rule\n\nBody for {id}.\n"
        )
    }

    fn write_rule(path: &Path, id: &str, title: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, rule_markdown(id, title)).expect("write rule fixture");
    }

    fn inputs<'a>(
        project_dir: &'a Path, rules_root: Option<&'a Path>, target_adapter: &'a str,
        source_adapters: &'a [String],
    ) -> ResolveInputs<'a> {
        ResolveInputs {
            project_dir,
            rules_root,
            target_adapter,
            source_adapters,
            artifact_paths: &[],
            languages: &[],
            include_deprecated: false,
            include_unmatched: false,
            include_core: false,
        }
    }

    fn no_sources() -> Vec<String> {
        Vec::new()
    }

    /// Test 1: shared rules under explicit `--rules-root` flow through
    /// as `origin=shared`, `path-root=rules-root`, and the path is
    /// relative to the rules root.
    #[test]
    fn shared_rules_from_explicit_rules_root() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        assert_eq!(result.len(), 1, "exactly one shared rule expected");
        let entry = &result[0];
        assert_eq!(entry.rule.id, "UNI-001");
        assert_eq!(entry.origin, Origin::Shared);
        assert_eq!(entry.path_root, PathRoot::RulesRoot);
        assert_eq!(entry.path, "adapters/shared/rules/universal/uni-001.md");
    }

    /// Core pack root: rules under
    /// `adapters/shared/rules/core/` resolve with `Origin::Core` and
    /// `PathRoot::RulesRoot`, alongside any shared-pack rules.
    #[test]
    fn core_rules_from_explicit_rules_root() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &rules_root.path().join("adapters/shared/rules/core/CORE-fixture.md"),
            "CORE-001",
            "Core fixture",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds with core pack");

        let core = result.iter().find(|e| e.rule.id == "CORE-001").expect("core rule present");
        assert_eq!(core.origin, Origin::Core);
        assert_eq!(core.path_root, PathRoot::RulesRoot);
        assert_eq!(core.path, "adapters/shared/rules/core/CORE-fixture.md");

        let shared = result.iter().find(|e| e.rule.id == "UNI-001").expect("shared still present");
        assert_eq!(shared.origin, Origin::Shared);
    }

    /// Test 2: monorepo / co-located case — no `--rules-root`, but the
    /// project tree carries the shared rules. Probe step 2 fires and
    /// resolution succeeds with `project_dir` as the rules root.
    #[test]
    fn rules_root_probe_falls_back() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Monorepo shared",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), None, "omnia", &sources))
            .expect("resolve succeeds in monorepo layout");

        assert_eq!(result.len(), 1);
        let entry = &result[0];
        assert_eq!(entry.origin, Origin::Shared);
        assert_eq!(entry.path_root, PathRoot::RulesRoot);
        assert_eq!(entry.path, "adapters/shared/rules/universal/uni-001.md");
    }

    /// Test 3: probe step 4 — no explicit root, no monorepo fallback,
    /// no distributed codex cache — must produce the closed
    /// `rules-root-required` error.
    #[test]
    fn rules_root_required_when_no_probe() {
        let project = TempDir::new().expect("project");
        let sources = no_sources();
        let err = resolve(&inputs(project.path(), None, "omnia", &sources)).unwrap_err();
        assert!(matches!(err, ResolveError::RulesRootRequired), "got: {err:?}");
    }

    /// Probe step 3 (RM-07): with no `--rules-root` and no monorepo
    /// tree, the distributed codex cache under
    /// `.specify/.cache/codex/` resolves shared rules. The cache root
    /// becomes the rules root, so the path is relative to it.
    #[test]
    fn shared_rules_from_codex_cache() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project
                .path()
                .join(".specify/.cache/codex/adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Distributed codex shared",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), None, "omnia", &sources))
            .expect("resolve succeeds via the distributed codex cache");

        assert_eq!(result.len(), 1);
        let entry = &result[0];
        assert_eq!(entry.rule.id, "UNI-001");
        assert_eq!(entry.origin, Origin::Shared);
        assert_eq!(entry.path_root, PathRoot::RulesRoot);
        assert_eq!(entry.path, "adapters/shared/rules/universal/uni-001.md");
    }

    /// Probe precedence: the monorepo tree (step 2) wins over the
    /// distributed codex cache (step 3). Only the monorepo rule
    /// resolves; the cache tree is never walked.
    #[test]
    fn monorepo_wins_over_codex_cache() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Monorepo shared",
        );
        write_rule(
            &project
                .path()
                .join(".specify/.cache/codex/adapters/shared/rules/universal/uni-002.md"),
            "UNI-002",
            "Cache shared",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), None, "omnia", &sources))
            .expect("resolve succeeds choosing the monorepo root");

        assert_eq!(result.len(), 1, "only the monorepo tree should be walked");
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Probe precedence: an explicit `--rules-root` (step 1) wins over
    /// a distributed codex cache (step 3).
    #[test]
    fn explicit_rules_root_wins_over_codex_cache() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Explicit shared",
        );
        write_rule(
            &project
                .path()
                .join(".specify/.cache/codex/adapters/shared/rules/universal/uni-002.md"),
            "UNI-002",
            "Cache shared",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds choosing the explicit rules root");

        assert_eq!(result.len(), 1, "only the explicit rules root should be walked");
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Test 4: target overlay resolves from the project-local rung
    /// while shared rules continue to flow from the explicit
    /// `--rules-root`. The target entry carries `path-root=project-dir`.
    #[test]
    fn target_overlay_from_project_local() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/rules/omnia-001.md"),
            "OMNIA-001",
            "Omnia overlay",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        assert_eq!(result.len(), 2);
        let shared = result.iter().find(|e| e.rule.id == "UNI-001").expect("shared present");
        let target = result.iter().find(|e| e.rule.id == "OMNIA-001").expect("target present");
        assert_eq!(shared.origin, Origin::Shared);
        assert_eq!(shared.path_root, PathRoot::RulesRoot);
        assert_eq!(target.origin, Origin::Target);
        assert_eq!(target.path_root, PathRoot::ProjectDir);
        assert_eq!(target.path, "adapters/targets/omnia/rules/omnia-001.md");
    }

    /// Test 5: rules-root fallback — project-local rung empty, manifest
    /// cache empty, explicit `--rules-root` carries the target overlay.
    #[test]
    fn target_overlay_falls_back_to_rules_root() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &rules_root.path().join("adapters/targets/omnia/rules/omnia-001.md"),
            "OMNIA-001",
            "Omnia fallback overlay",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        let target = result.iter().find(|e| e.rule.id == "OMNIA-001").expect("target present");
        assert_eq!(target.origin, Origin::Target);
        assert_eq!(target.path_root, PathRoot::RulesRoot);
        assert_eq!(target.path, "adapters/targets/omnia/rules/omnia-001.md");
    }

    /// Test 6: source overlay from project-local rung. Confirms
    /// `Origin::Source` + `PathRoot::ProjectDir` assignment.
    #[test]
    fn source_overlay_from_project_local() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/sources/code-typescript/rules/src-001.md"),
            "SRC-001",
            "TS source overlay",
        );

        let sources = vec!["code-typescript".to_string()];
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        let src = result.iter().find(|e| e.rule.id == "SRC-001").expect("source present");
        assert_eq!(src.origin, Origin::Source);
        assert_eq!(src.path_root, PathRoot::ProjectDir);
        assert_eq!(src.path, "adapters/sources/code-typescript/rules/src-001.md");
    }

    /// Test 7: multiple bound source adapters each contribute their
    /// own overlay; both `Source` entries appear in the result.
    #[test]
    fn multiple_source_overlays() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/sources/code-typescript/rules/src-001.md"),
            "SRC-001",
            "TS overlay",
        );
        write_rule(
            &project.path().join("adapters/sources/documentation/rules/src-002.md"),
            "SRC-002",
            "Docs overlay",
        );

        let sources = vec!["code-typescript".to_string(), "documentation".to_string()];
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        let source_entries: Vec<_> = result.iter().filter(|e| e.origin == Origin::Source).collect();
        assert_eq!(source_entries.len(), 2);
        assert!(source_entries.iter().any(|e| e.rule.id == "SRC-001"));
        assert!(source_entries.iter().any(|e| e.rule.id == "SRC-002"));
    }

    /// Test 8: manifest-cache rung. Project-local missing, manifest
    /// cache present — the result carries `PathRoot::ProjectDir` and
    /// the path starts with `.specify/.cache/manifests/...`.
    #[test]
    fn cache_overlay_when_local_missing() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project
                .path()
                .join(".specify/.cache/manifests/sources/code-typescript/rules/src-001.md"),
            "SRC-001",
            "TS cache overlay",
        );

        let sources = vec!["code-typescript".to_string()];
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        let src = result.iter().find(|e| e.rule.id == "SRC-001").expect("source present");
        assert_eq!(src.origin, Origin::Source);
        assert_eq!(src.path_root, PathRoot::ProjectDir);
        assert_eq!(src.path, ".specify/.cache/manifests/sources/code-typescript/rules/src-001.md");
    }

    /// Test 9: duplicate id across overlays — same `UNI-001` declared
    /// twice — fails with [`ResolveError::DuplicateRuleId`] regardless
    /// of namespace ownership (which is `check::rules`'s problem).
    #[test]
    fn duplicate_rule_id_errors() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/rules/uni-001-clone.md"),
            "UNI-001",
            "Clone in omnia overlay",
        );

        let sources = no_sources();
        let err = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .unwrap_err();
        match err {
            ResolveError::DuplicateRuleId { id, paths } => {
                assert_eq!(id, "UNI-001");
                assert!(
                    paths.contains("adapters/shared/rules/universal/uni-001.md"),
                    "duplicate paths must cite the shared file: {paths}",
                );
                assert!(
                    paths.contains("adapters/targets/omnia/rules/uni-001-clone.md"),
                    "duplicate paths must cite the target overlay file: {paths}",
                );
            }
            other => panic!("expected DuplicateRuleId, got {other:?}"),
        }
    }

    /// Test 10: README.md (case-insensitive) is excluded from
    /// discovery.
    #[test]
    fn readme_md_is_skipped() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        // README intentionally has no frontmatter — if discovery
        // walked into it the CH-11 parser would also raise a
        // ParseError, which the assertion below would catch.
        let readme = rules_root.path().join("adapters/shared/rules/universal/README.md");
        fs::write(&readme, "# Shared codex\n\nNotes about shared rules.\n").expect("write readme");

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds with README present");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Test 11: monorepo golden path. `project_dir == rules_root`
    /// (probe step 2). Shared rules anchor on `RulesRoot`; project-local
    /// target/source overlays anchor on `ProjectDir`, because they were
    /// found via the project-local rung — not the rules-root fallback.
    #[test]
    fn monorepo_split_anchors() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/rules/omnia-001.md"),
            "OMNIA-001",
            "Target",
        );
        write_rule(
            &project.path().join("adapters/sources/code-typescript/rules/src-001.md"),
            "SRC-001",
            "Source",
        );

        let sources = vec!["code-typescript".to_string()];
        let result = resolve(&inputs(project.path(), None, "omnia", &sources))
            .expect("resolve succeeds in monorepo layout");

        let shared = result.iter().find(|e| e.rule.id == "UNI-001").expect("shared present");
        let target = result.iter().find(|e| e.rule.id == "OMNIA-001").expect("target present");
        let source = result.iter().find(|e| e.rule.id == "SRC-001").expect("source present");

        assert_eq!(shared.path_root, PathRoot::RulesRoot);
        assert_eq!(target.path_root, PathRoot::ProjectDir);
        assert_eq!(source.path_root, PathRoot::ProjectDir);
    }

    /// In the monorepo probe-step-2 path, the rules-root fallback rung
    /// must NOT also run for target/source overlays — otherwise a
    /// project-local entry would shadow itself and surface as a
    /// duplicate-id error. Regression guard for the explicit-vs-derived
    /// rules-root distinction.
    #[test]
    fn monorepo_no_double_fallback_walk() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/rules/omnia-001.md"),
            "OMNIA-001",
            "Target",
        );
        let sources = no_sources();
        let result = resolve(&inputs(project.path(), None, "omnia", &sources))
            .expect("resolve must not produce a duplicate-id error");
        assert_eq!(result.len(), 2);
    }

    /// Discovery is non-recursive — a stray nested rule must not be
    /// picked up, mirroring CH-09's flat-directory expectation.
    #[test]
    fn discovery_is_non_recursive() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/nested/uni-002.md"),
            "UNI-002",
            "Nested",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .expect("resolve succeeds");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Empty source adapter list and missing target overlay are not
    /// errors — only shared rules surface.
    #[test]
    fn missing_overlays_are_silent() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );

        let sources: Vec<String> = vec!["unbound-source".to_string()];
        let result =
            resolve(&inputs(project.path(), Some(rules_root.path()), "unbound-target", &sources))
                .expect("missing overlays must not error");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Parse failures bubble up as [`ResolveError::Parse`] carrying
    /// the offending absolute path.
    #[test]
    fn parse_error_includes_path() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        let bad_path = rules_root.path().join("adapters/shared/rules/universal/broken.md");
        fs::create_dir_all(bad_path.parent().unwrap()).expect("parent");
        fs::write(&bad_path, "no frontmatter here\n").expect("write broken rule");

        let sources = no_sources();
        let err = resolve(&inputs(project.path(), Some(rules_root.path()), "omnia", &sources))
            .unwrap_err();
        match err {
            ResolveError::Parse { path, .. } => {
                assert_eq!(path, bad_path);
            }
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    /// `artifact_paths` / `languages` / `include_*` flags are accepted
    /// but unused in CH-12 — the resolver must not error on populated
    /// fields and must produce the same result as the default-empty
    /// case so CH-13 can layer filtering on top without rewriting the
    /// caller surface.
    #[test]
    fn ch13_inputs_are_accepted_but_ignored() {
        let rules_root = TempDir::new().expect("rules root");
        let project = TempDir::new().expect("project");
        write_rule(
            &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );

        let artifact_paths = vec![PathBuf::from("crates/billing/src/lib.rs")];
        let languages = vec!["rust".to_string()];
        let sources = no_sources();
        let inputs = ResolveInputs {
            project_dir: project.path(),
            rules_root: Some(rules_root.path()),
            target_adapter: "omnia",
            source_adapters: &sources,
            artifact_paths: &artifact_paths,
            languages: &languages,
            include_deprecated: true,
            include_unmatched: true,
            include_core: true,
        };

        let result = resolve(&inputs).expect("resolve succeeds with CH-13 inputs populated");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// `rules-root-required` from CH-12 maps to a payload-free
    /// `Error::Validation` so the wire envelope carries the closed
    /// kebab discriminant in the top-level `error` code.
    #[test]
    fn maps_rules_root_required_to_validation() {
        let err = map_resolve_error(ResolveError::RulesRootRequired);
        match err {
            Error::Validation { code, .. } => {
                assert_eq!(code, "rules-root-required");
            }
            other => panic!("expected Error::Validation, got {other:?}"),
        }
    }

    /// `DuplicateRuleId` lands on a payload-free `Error::Validation`
    /// keyed on `rules-duplicate-rule-id`, with the colliding id and
    /// joined paths folded into the `detail` message.
    #[test]
    fn maps_duplicate_rule_id_to_validation() {
        let err = map_resolve_error(ResolveError::DuplicateRuleId {
            id: "UNI-001".into(),
            paths: "a.md, b.md".into(),
        });
        match err {
            Error::Validation { code, detail } => {
                assert_eq!(code, "rules-duplicate-rule-id");
                assert!(detail.contains("UNI-001"), "{detail}");
                assert!(detail.contains("a.md, b.md"), "{detail}");
            }
            other => panic!("expected Error::Validation, got {other:?}"),
        }
    }

    /// Filesystem failures map to `Error::Filesystem { op: "readdir" }`
    /// so the JSON discriminant becomes `filesystem-readdir` (exit 1).
    #[test]
    fn maps_filesystem_to_filesystem_error() {
        let err = map_resolve_error(ResolveError::Filesystem {
            path: PathBuf::from("/missing"),
            source: io::Error::from(io::ErrorKind::NotFound),
        });
        match err {
            Error::Filesystem { op, path, .. } => {
                assert_eq!(op, "readdir");
                assert_eq!(path, PathBuf::from("/missing"));
            }
            other => panic!("expected Error::Filesystem, got {other:?}"),
        }
    }
}
