//! Codex resolver (CH-12): roots + overlay discovery.
//!
//! Implements RFC-28 §"Resolution roots" / §"Codex root resolution (v1)"
//! / §"Resolution inputs" / §"Overlay precedence". This chunk discovers
//! and parses every rule the export envelope eventually carries.
//!
//! Filtering by applicability and deprecation lives in the sibling
//! [`filter`] module (CH-13); stable export ordering (CH-14) and
//! findings (CH-15/16) remain out of scope here.
//!
//! # Closed precedence order
//!
//! Shared root (root 1) resolves from a **codex root**, picked by the
//! closed probe in RFC-28 §"Codex root resolution (v1)":
//!
//! 1. `inputs.codex_root` when supplied — use for root 1 and the
//!    codex-root fallback overlay (overlay step 3 below).
//! 2. Else if `{project_dir}/adapters/shared/codex/universal/` exists,
//!    treat `project_dir` as the codex root (monorepo case). In this
//!    case the codex-root fallback overlay step is **skipped** —
//!    re-walking `project_dir` would just shadow the project-local
//!    rung with the same filesystem tree.
//! 3. Else → [`ResolveError::CodexRootRequired`].
//!
//! Source-adapter (root 3) and target-adapter (root 4) overlays follow
//! the closed location order in RFC-28 §"Resolution roots":
//!
//! 1. project-local `{project_dir}/adapters/{sources,targets}/<name>/codex/`;
//! 2. manifest cache `{project_dir}/.specify/.cache/manifests/{sources,targets}/<name>/codex/`;
//! 3. codex-root fallback `{codex_root}/adapters/{sources,targets}/<name>/codex/`,
//!    **only** when `inputs.codex_root.is_some()` (step 1 of the probe);
//! 4. omit when no rung exists.
//!
//! The **first existing** rung wins; locations never merge. Roots 2
//! (shared language/artifact packs) and 5 (organization overlays) are
//! reserved by RFC-28 and not implemented here.
//!
//! # Duplicate ids
//!
//! Per RFC-28 §"Overlay precedence": rules never override each other
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
//!   the [`super::ResolvedCodex`] envelope are assembled by
//!   [`build_resolved_codex`].

mod filter;
mod sort;

use std::path::{Path, PathBuf};
use std::{fs, io};

pub use filter::filter;
pub use sort::{build_resolved_codex, sort_resolved};

use super::parse::{ParseError, parse_codex_rule_file};
use super::{CodexRule, Origin, PathRoot};

/// Closed input contract for [`resolve`] and [`filter`] per RFC-28
/// §"Resolution inputs".
///
/// CH-12's [`resolve`] consumes `project_dir`, `codex_root`,
/// `target_adapter`, and `source_adapters`. CH-13's [`filter`]
/// additionally consumes `artifact_paths`, `languages`,
/// `include_deprecated`, and `include_unmatched`. Calling
/// Callers compose [`resolve`] then [`filter`] when they need the
/// narrowed pool; [`build_resolved_codex`] is the export entry point.
#[derive(Debug, Clone)]
pub struct ResolveInputs<'a> {
    /// Project root used for adapter resolution and optional
    /// project-local overlays.
    pub project_dir: &'a Path,
    /// Root containing first-party codex content for shared rules and
    /// codex-root fallback overlays. See §"Codex root resolution (v1)".
    pub codex_root: Option<&'a Path>,
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
}

/// Pre-sort intermediate emitted by [`resolve`].
///
/// CH-14 owns the final sorted [`super::ResolvedCodex`]; CH-12 returns
/// every discovered rule with `origin`, `path_root`, and `path`
/// populated. The `path` string is always relative to the matching
/// [`PathRoot`], with forward slashes as separators.
#[derive(Debug, Clone)]
pub struct ResolvedRuleEntry {
    /// Parsed codex rule from CH-11.
    pub rule: CodexRule,
    /// Resolver origin tier (shared / source / target).
    pub origin: Origin,
    /// Anchor for [`Self::path`] — codex-root vs project-dir.
    pub path_root: PathRoot,
    /// Path to the rule markdown file relative to `path_root`.
    pub path: String,
}

/// Failure modes for [`resolve`].
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// Shared probe failed: no `--codex-root` and no monorepo
    /// fallback under `project_dir`. Wire-id `codex-root-required`
    /// per RFC-28 §"Codex root resolution (v1)" and the §490 golden.
    #[error(
        "codex-root-required: shared UNI-* rules require --codex-root pointing at a tree containing adapters/shared/codex/universal/"
    )]
    CodexRootRequired,
    /// A rule id appeared in more than one discovered file. Per
    /// RFC-28 §"Overlay precedence" this is always invalid.
    #[error("duplicate rule id '{id}' across files: {paths}")]
    DuplicateRuleId {
        /// The colliding rule id.
        id: String,
        /// Comma-joined relative paths of every offending file.
        paths: String,
    },
    /// CH-11 parser rejected one of the discovered files.
    #[error("codex rule parse failed: {path}: {error}")]
    Parse {
        /// Absolute path of the failing file.
        path: PathBuf,
        /// Underlying parser error.
        error: ParseError,
    },
    /// `read_dir` / `read_to_string` failed while walking a rung.
    #[error("codex rule discovery failed: {path}: {source}")]
    Filesystem {
        /// Absolute path of the failing directory or file.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
}

const SHARED_REL: &str = "adapters/shared/codex/universal";
const MANIFEST_CACHE_REL: &str = ".specify/.cache/manifests";

/// Discover every codex rule visible to `inputs` and parse it.
///
/// See the module docs for the closed precedence order, codex-root
/// probe, and duplicate-id rules. Filtering and sorting are deferred
/// to CH-13 / CH-14.
///
/// # Errors
///
/// Returns the matching [`ResolveError`] variant for each failure
/// mode; see the variant docs.
pub fn resolve(inputs: &ResolveInputs<'_>) -> Result<Vec<ResolvedRuleEntry>, ResolveError> {
    let codex_root = probe_codex_root(inputs)?;
    let explicit_codex_root = inputs.codex_root.is_some();

    let mut entries: Vec<ResolvedRuleEntry> = Vec::new();

    let shared_dir = codex_root.join(SHARED_REL);
    if shared_dir.is_dir() {
        for path in list_rule_files(&shared_dir)? {
            let rel = relative_path(&codex_root, &path);
            let rule = parse(&path)?;
            entries.push(ResolvedRuleEntry {
                rule,
                origin: Origin::Shared,
                path_root: PathRoot::CodexRoot,
                path: rel,
            });
        }
    }

    for source_name in inputs.source_adapters {
        load_overlay(
            inputs.project_dir,
            &codex_root,
            explicit_codex_root,
            "sources",
            source_name,
            Origin::Source,
            &mut entries,
        )?;
    }

    load_overlay(
        inputs.project_dir,
        &codex_root,
        explicit_codex_root,
        "targets",
        inputs.target_adapter,
        Origin::Target,
        &mut entries,
    )?;

    detect_duplicates(&entries)?;

    Ok(entries)
}

/// Pick the codex root from the §"Codex root resolution (v1)" probe.
fn probe_codex_root(inputs: &ResolveInputs<'_>) -> Result<PathBuf, ResolveError> {
    if let Some(explicit) = inputs.codex_root {
        return Ok(explicit.to_path_buf());
    }
    let project_shared = inputs.project_dir.join(SHARED_REL);
    if project_shared.is_dir() {
        return Ok(inputs.project_dir.to_path_buf());
    }
    Err(ResolveError::CodexRootRequired)
}

/// Walk overlay rungs for one adapter and append discovered rules.
///
/// `axis_segment` is the literal directory name (`"sources"` or
/// `"targets"`). The first existing rung wins per RFC-28 §"Resolution
/// roots".
fn load_overlay(
    project_dir: &Path, codex_root: &Path, explicit_codex_root: bool, axis_segment: &str,
    adapter_name: &str, origin: Origin, out: &mut Vec<ResolvedRuleEntry>,
) -> Result<(), ResolveError> {
    let project_local =
        project_dir.join("adapters").join(axis_segment).join(adapter_name).join("codex");
    let manifest_cache =
        project_dir.join(MANIFEST_CACHE_REL).join(axis_segment).join(adapter_name).join("codex");

    if project_local.is_dir() {
        return collect_overlay(&project_local, project_dir, PathRoot::ProjectDir, origin, out);
    }
    if manifest_cache.is_dir() {
        return collect_overlay(&manifest_cache, project_dir, PathRoot::ProjectDir, origin, out);
    }
    if explicit_codex_root {
        let fallback =
            codex_root.join("adapters").join(axis_segment).join(adapter_name).join("codex");
        if fallback.is_dir() {
            return collect_overlay(&fallback, codex_root, PathRoot::CodexRoot, origin, out);
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

fn parse(path: &Path) -> Result<CodexRule, ResolveError> {
    parse_codex_rule_file(path).map_err(|error| ResolveError::Parse {
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

/// Per RFC-28 §"Overlay precedence": duplicates always error.
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
        project_dir: &'a Path, codex_root: Option<&'a Path>, target_adapter: &'a str,
        source_adapters: &'a [String],
    ) -> ResolveInputs<'a> {
        ResolveInputs {
            project_dir,
            codex_root,
            target_adapter,
            source_adapters,
            artifact_paths: &[],
            languages: &[],
            include_deprecated: false,
            include_unmatched: false,
        }
    }

    fn no_sources() -> Vec<String> {
        Vec::new()
    }

    /// Test 1: shared rules under explicit `--codex-root` flow through
    /// as `origin=shared`, `path-root=codex-root`, and the path is
    /// relative to the codex root.
    #[test]
    fn shared_rules_from_explicit_codex_root() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        assert_eq!(result.len(), 1, "exactly one shared rule expected");
        let entry = &result[0];
        assert_eq!(entry.rule.id, "UNI-001");
        assert_eq!(entry.origin, Origin::Shared);
        assert_eq!(entry.path_root, PathRoot::CodexRoot);
        assert_eq!(entry.path, "adapters/shared/codex/universal/uni-001.md");
    }

    /// Test 2: monorepo / co-located case — no `--codex-root`, but the
    /// project tree carries the shared rules. Probe step 2 fires and
    /// resolution succeeds with `project_dir` as the codex root.
    #[test]
    fn codex_root_probe_falls_back_to_project_dir() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Monorepo shared",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), None, "omnia", &sources))
            .expect("resolve succeeds in monorepo layout");

        assert_eq!(result.len(), 1);
        let entry = &result[0];
        assert_eq!(entry.origin, Origin::Shared);
        assert_eq!(entry.path_root, PathRoot::CodexRoot);
        assert_eq!(entry.path, "adapters/shared/codex/universal/uni-001.md");
    }

    /// Test 3: probe step 3 — no explicit root, no monorepo fallback —
    /// must produce the closed `codex-root-required` error.
    #[test]
    fn codex_root_required_error_when_no_probe_succeeds() {
        let project = TempDir::new().expect("project");
        let sources = no_sources();
        let err = resolve(&inputs(project.path(), None, "omnia", &sources)).unwrap_err();
        assert!(matches!(err, ResolveError::CodexRootRequired), "got: {err:?}");
    }

    /// Test 4: target overlay resolves from the project-local rung
    /// while shared rules continue to flow from the explicit
    /// `--codex-root`. The target entry carries `path-root=project-dir`.
    #[test]
    fn target_overlay_from_project_local() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/codex/omnia-001.md"),
            "OMNIA-001",
            "Omnia overlay",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        assert_eq!(result.len(), 2);
        let shared = result.iter().find(|e| e.rule.id == "UNI-001").expect("shared present");
        let target = result.iter().find(|e| e.rule.id == "OMNIA-001").expect("target present");
        assert_eq!(shared.origin, Origin::Shared);
        assert_eq!(shared.path_root, PathRoot::CodexRoot);
        assert_eq!(target.origin, Origin::Target);
        assert_eq!(target.path_root, PathRoot::ProjectDir);
        assert_eq!(target.path, "adapters/targets/omnia/codex/omnia-001.md");
    }

    /// Test 5: codex-root fallback — project-local rung empty, manifest
    /// cache empty, explicit `--codex-root` carries the target overlay.
    #[test]
    fn target_overlay_falls_back_to_codex_root() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &codex_root.path().join("adapters/targets/omnia/codex/omnia-001.md"),
            "OMNIA-001",
            "Omnia fallback overlay",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        let target = result.iter().find(|e| e.rule.id == "OMNIA-001").expect("target present");
        assert_eq!(target.origin, Origin::Target);
        assert_eq!(target.path_root, PathRoot::CodexRoot);
        assert_eq!(target.path, "adapters/targets/omnia/codex/omnia-001.md");
    }

    /// Test 6: source overlay from project-local rung. Confirms
    /// `Origin::Source` + `PathRoot::ProjectDir` assignment.
    #[test]
    fn source_overlay_from_project_local() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/sources/code-typescript/codex/src-001.md"),
            "SRC-001",
            "TS source overlay",
        );

        let sources = vec!["code-typescript".to_string()];
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        let src = result.iter().find(|e| e.rule.id == "SRC-001").expect("source present");
        assert_eq!(src.origin, Origin::Source);
        assert_eq!(src.path_root, PathRoot::ProjectDir);
        assert_eq!(src.path, "adapters/sources/code-typescript/codex/src-001.md");
    }

    /// Test 7: multiple bound source adapters each contribute their
    /// own overlay; both `Source` entries appear in the result.
    #[test]
    fn multiple_source_overlays() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/sources/code-typescript/codex/src-001.md"),
            "SRC-001",
            "TS overlay",
        );
        write_rule(
            &project.path().join("adapters/sources/documentation/codex/src-002.md"),
            "SRC-002",
            "Docs overlay",
        );

        let sources = vec!["code-typescript".to_string(), "documentation".to_string()];
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
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
    fn manifest_cache_overlay_when_project_local_missing() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project
                .path()
                .join(".specify/.cache/manifests/sources/code-typescript/codex/src-001.md"),
            "SRC-001",
            "TS cache overlay",
        );

        let sources = vec!["code-typescript".to_string()];
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .expect("resolve succeeds");

        let src = result.iter().find(|e| e.rule.id == "SRC-001").expect("source present");
        assert_eq!(src.origin, Origin::Source);
        assert_eq!(src.path_root, PathRoot::ProjectDir);
        assert_eq!(src.path, ".specify/.cache/manifests/sources/code-typescript/codex/src-001.md");
    }

    /// Test 9: duplicate id across overlays — same `UNI-001` declared
    /// twice — fails with [`ResolveError::DuplicateRuleId`] regardless
    /// of namespace ownership (which is `check::codex`'s problem).
    #[test]
    fn duplicate_rule_id_across_overlays_errors() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/codex/uni-001-clone.md"),
            "UNI-001",
            "Clone in omnia overlay",
        );

        let sources = no_sources();
        let err = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .unwrap_err();
        match err {
            ResolveError::DuplicateRuleId { id, paths } => {
                assert_eq!(id, "UNI-001");
                assert!(
                    paths.contains("adapters/shared/codex/universal/uni-001.md"),
                    "duplicate paths must cite the shared file: {paths}",
                );
                assert!(
                    paths.contains("adapters/targets/omnia/codex/uni-001-clone.md"),
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
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared universal",
        );
        // README intentionally has no frontmatter — if discovery
        // walked into it the CH-11 parser would also raise a
        // ParseError, which the assertion below would catch.
        let readme = codex_root.path().join("adapters/shared/codex/universal/README.md");
        fs::write(&readme, "# Shared codex\n\nNotes about shared rules.\n").expect("write readme");

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .expect("resolve succeeds with README present");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Test 11: monorepo golden path. `project_dir == codex_root`
    /// (probe step 2). Shared rules anchor on `CodexRoot`; project-local
    /// target/source overlays anchor on `ProjectDir`, because they were
    /// found via the project-local rung — not the codex-root fallback.
    #[test]
    fn monorepo_anchors_split_between_codex_root_and_project_dir() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/codex/omnia-001.md"),
            "OMNIA-001",
            "Target",
        );
        write_rule(
            &project.path().join("adapters/sources/code-typescript/codex/src-001.md"),
            "SRC-001",
            "Source",
        );

        let sources = vec!["code-typescript".to_string()];
        let result = resolve(&inputs(project.path(), None, "omnia", &sources))
            .expect("resolve succeeds in monorepo layout");

        let shared = result.iter().find(|e| e.rule.id == "UNI-001").expect("shared present");
        let target = result.iter().find(|e| e.rule.id == "OMNIA-001").expect("target present");
        let source = result.iter().find(|e| e.rule.id == "SRC-001").expect("source present");

        assert_eq!(shared.path_root, PathRoot::CodexRoot);
        assert_eq!(target.path_root, PathRoot::ProjectDir);
        assert_eq!(source.path_root, PathRoot::ProjectDir);
    }

    /// In the monorepo probe-step-2 path, the codex-root fallback rung
    /// must NOT also run for target/source overlays — otherwise a
    /// project-local entry would shadow itself and surface as a
    /// duplicate-id error. Regression guard for the explicit-vs-derived
    /// codex-root distinction.
    #[test]
    fn monorepo_does_not_double_walk_codex_root_fallback() {
        let project = TempDir::new().expect("project");
        write_rule(
            &project.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/codex/omnia-001.md"),
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
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/nested/uni-002.md"),
            "UNI-002",
            "Nested",
        );

        let sources = no_sources();
        let result = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
            .expect("resolve succeeds");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Empty source adapter list and missing target overlay are not
    /// errors — only shared rules surface.
    #[test]
    fn missing_overlays_are_silent() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );

        let sources: Vec<String> = vec!["unbound-source".to_string()];
        let result =
            resolve(&inputs(project.path(), Some(codex_root.path()), "unbound-target", &sources))
                .expect("missing overlays must not error");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }

    /// Parse failures bubble up as [`ResolveError::Parse`] carrying
    /// the offending absolute path.
    #[test]
    fn parse_error_surfaces_with_offending_path() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        let bad_path = codex_root.path().join("adapters/shared/codex/universal/broken.md");
        fs::create_dir_all(bad_path.parent().unwrap()).expect("parent");
        fs::write(&bad_path, "no frontmatter here\n").expect("write broken rule");

        let sources = no_sources();
        let err = resolve(&inputs(project.path(), Some(codex_root.path()), "omnia", &sources))
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
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared",
        );

        let artifact_paths = vec![PathBuf::from("crates/billing/src/lib.rs")];
        let languages = vec!["rust".to_string()];
        let sources = no_sources();
        let inputs = ResolveInputs {
            project_dir: project.path(),
            codex_root: Some(codex_root.path()),
            target_adapter: "omnia",
            source_adapters: &sources,
            artifact_paths: &artifact_paths,
            languages: &languages,
            include_deprecated: true,
            include_unmatched: true,
        };

        let result = resolve(&inputs).expect("resolve succeeds with CH-13 inputs populated");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rule.id, "UNI-001");
    }
}
