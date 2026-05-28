//! Consumer indexer per the standards-layer contract §"`WorkspaceModel`" and
//! the file scan contract — Consumer scan scope.
//!
//! v1 (Phase 2) ships the consumer scan: a `.gitignore`-aware file
//! walk, per-file extractors that run in parallel via `rayon`, and a
//! sequential second pass that records symlinks, discovers codex
//! rules, and resolves cross-file edges. The byte-stable assembly
//! follows `WorkspaceModel` stability: every output collection is sorted by
//! its documented sort key before envelope emission so the JSON
//! serialisation is reproducible irrespective of thread scheduling.
//!
//! The umbrella owns the [`build`] entry point and the closed
//! [`IndexError`] enum the runtime maps to exit codes via
//! `Exit::from(&Error)`. `scan_profile: framework` is reserved for
//! future framework scanning and surfaces here as [`IndexError::UnsupportedScanProfile`].

pub mod discover;
pub mod files;
pub mod frontmatter;
pub mod markdown;
pub mod symlinks;

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::lint::{
    File, Frontmatter, MarkdownLink, MarkdownSection, ScanProfile, WorkspaceModel,
    WorkspaceModelVersion,
};

/// Closed error set for [`build`].
///
/// Per-file extractor failures (malformed YAML frontmatter,
/// unreadable bytes, etc.) collapse to silent per-file skips in v1;
/// reserved-hint diagnostics reserves the `index.warning` finding for S7's hint
/// runner. Only conditions the indexer cannot meaningfully recover
/// from surface as `Err`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexError {
    /// `scan_profile: framework` is reserved for a future framework scan; v1 supports
    /// only [`ScanProfile::Consumer`].
    #[error("unsupported scan profile: {0:?} (v1 supports only `consumer`)")]
    UnsupportedScanProfile(ScanProfile),
    /// `project_dir` is not an existing directory; the walk cannot
    /// proceed.
    #[error("project directory not found: {0}")]
    ProjectDirMissing(PathBuf),
    /// Always-ignore override compilation failed. Indicates a
    /// programmer error in [`files`]'s static glob list.
    #[error("always-ignore override compilation failed: {0}")]
    OverrideCompile(String),
}

/// Build the consumer [`WorkspaceModel`] for `project_dir`.
///
/// When `artifact_paths` is empty the walk roots at `project_dir`;
/// otherwise each entry becomes a project-relative walk root per
/// lint scope resolution. `languages`, when non-empty, narrows the discovered
/// file set to extensions whose inferred language token matches one
/// of the supplied tokens — unknown / binary files are kept so that
/// symlinks and asset-adjacent files still appear in the model.
///
/// # Errors
///
/// Returns the matching [`IndexError`] variant; see the per-variant
/// documentation. Per-file extractor failures are swallowed silently.
pub fn build(
    project_dir: &Path, scan_profile: ScanProfile, artifact_paths: &[PathBuf], languages: &[String],
) -> Result<WorkspaceModel, IndexError> {
    if scan_profile != ScanProfile::Consumer {
        return Err(IndexError::UnsupportedScanProfile(scan_profile));
    }

    let discovery = files::discover(project_dir, artifact_paths, languages)?;
    let discovered = discovery.files;
    let symlinks_facts = discovery.symlinks;

    let per_file: Vec<PerFile> = discovered
        .par_iter()
        .map(|file| PerFile {
            file: File {
                path: file.relative.clone(),
                kind: file.kind,
                language: file.language.clone(),
                sha256: None,
            },
            frontmatter: frontmatter::extract(file),
            sections: markdown::extract_sections(file),
            links: markdown::extract_links(file),
        })
        .collect();

    let mut files_out: Vec<File> = Vec::with_capacity(per_file.len());
    let mut frontmatter_out: Vec<Frontmatter> = Vec::new();
    let mut sections_out: Vec<MarkdownSection> = Vec::new();
    let mut links_out: Vec<MarkdownLink> = Vec::new();
    for entry in per_file {
        files_out.push(entry.file);
        if let Some(fm) = entry.frontmatter {
            frontmatter_out.push(fm);
        }
        sections_out.extend(entry.sections);
        links_out.extend(entry.links);
    }

    let known_paths: std::collections::HashSet<String> =
        files_out.iter().map(|f| f.path.clone()).collect();
    for link in &mut links_out {
        link.resolves = resolve_link(project_dir, &link.from_path, &link.to_raw, &known_paths);
    }

    let rule_index = discover::discover(project_dir);

    files_out.sort_by(|a, b| a.path.cmp(&b.path));
    frontmatter_out.sort_by(|a, b| a.path.cmp(&b.path));
    sections_out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line_start.cmp(&b.line_start)));
    links_out.sort_by(|a, b| {
        a.from_path
            .cmp(&b.from_path)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.to_raw.cmp(&b.to_raw))
    });

    Ok(WorkspaceModel {
        version: WorkspaceModelVersion,
        project_dir: project_dir.to_string_lossy().into_owned(),
        scan_profile: ScanProfile::Consumer,
        artifact_paths: artifact_paths.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
        languages: languages.to_vec(),
        files: files_out,
        frontmatter: frontmatter_out,
        markdown_sections: sections_out,
        markdown_links: links_out,
        symlinks: symlinks_facts,
        skills: Vec::new(),
        adapter_manifests: Vec::new(),
        marketplace_entries: Vec::new(),
        rule_index,
        text_matches: Vec::new(),
    })
}

struct PerFile {
    file: File,
    frontmatter: Option<Frontmatter>,
    sections: Vec<MarkdownSection>,
    links: Vec<MarkdownLink>,
}

/// Resolve a markdown link target. URL-style targets (matching
/// `^[a-z][a-z0-9+\-.]*://`) leave `resolves` unset; relative paths
/// are joined against `from_path`'s parent and checked against both
/// the discovered file set and the filesystem.
fn resolve_link(
    project_dir: &Path, from_path: &str, to_raw: &str,
    known_paths: &std::collections::HashSet<String>,
) -> Option<bool> {
    if is_url_scheme(to_raw) {
        return None;
    }
    let target_without_fragment = to_raw.split(['#', '?']).next().unwrap_or(to_raw);
    if target_without_fragment.is_empty() {
        return None;
    }
    let from = Path::new(from_path);
    let base = from.parent().unwrap_or_else(|| Path::new(""));
    let joined = base.join(target_without_fragment);
    let normalised = normalise_relative(&joined);
    let candidate_rel = normalised.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
    if known_paths.contains(&candidate_rel) {
        return Some(true);
    }
    Some(project_dir.join(&candidate_rel).exists())
}

fn is_url_scheme(target: &str) -> bool {
    let Some(colon) = target.find("://") else {
        return false;
    };
    let scheme = &target[..colon];
    if scheme.is_empty() {
        return false;
    }
    scheme
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+' || c == '-' || c == '.')
}

/// Collapse `./` segments and resolve `..` against earlier segments
/// without touching the filesystem; project-relative paths use
/// forward slashes per `WorkspaceModel` stability.
fn normalise_relative(path: &Path) -> PathBuf {
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s.to_os_string()),
            Component::CurDir | Component::Prefix(_) | Component::RootDir => {}
        }
    }
    let mut buf = PathBuf::new();
    for segment in out {
        buf.push(segment);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_scheme_detection_accepts_common_schemes() {
        assert!(is_url_scheme("https://example.com"));
        assert!(is_url_scheme("http://example.com"));
        assert!(is_url_scheme("mailto://x"));
        assert!(is_url_scheme("file://something"));
        assert!(!is_url_scheme("./local.md"));
        assert!(!is_url_scheme("../other.md"));
        assert!(!is_url_scheme("plain.md"));
    }

    #[test]
    fn normalise_collapses_dot_segments() {
        let p = normalise_relative(Path::new("docs/./foo/../bar.md"));
        assert_eq!(p, PathBuf::from("docs/bar.md"));
    }
}
