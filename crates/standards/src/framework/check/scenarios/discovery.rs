//! Scenario-file discovery across the acceptance lifecycle pack, target tests, and plugin fixtures.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::framework::context::Context;
use crate::framework::helpers::under_symlink;

pub(super) fn discover_scenario_candidates(ctx: &Context) -> Vec<PathBuf> {
    let root = ctx.framework_root();
    let mut candidates = Vec::new();

    collect_lifecycle_scenarios(&root.join("acceptance").join("lifecycle"), &mut candidates);
    collect_target_scenarios(&ctx.targets_dir(), &mut candidates);
    collect_plugin_fixture_scenarios(&root.join("plugins"), root, &mut candidates);

    candidates.sort();
    candidates.dedup();
    candidates
}

/// Collects the flat `acceptance/lifecycle/<id>.md` scenario files (one self-contained
/// scenario per `.md`), skipping the pack `README.md` catalog.
fn collect_lifecycle_scenarios(lifecycle_dir: &Path, out: &mut Vec<PathBuf>) {
    if !lifecycle_dir.is_dir() {
        return;
    }
    for entry in WalkDir::new(lifecycle_dir).max_depth(1).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "README.md" || !name.ends_with(".md") {
            continue;
        }
        out.push(path);
    }
}

fn collect_target_scenarios(targets_dir: &Path, out: &mut Vec<PathBuf>) {
    if !targets_dir.is_dir() {
        return;
    }
    for entry in WalkDir::new(targets_dir).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let rel = path.strip_prefix(targets_dir).unwrap_or(&path);
        let parts: Vec<_> = rel.components().filter_map(|c| c.as_os_str().to_str()).collect();
        if parts.len() == 3 && parts[1] == "tests" {
            out.push(path.clone());
        }
        if parts.len() == 4 && parts[1] == "tests" && parts[3] == "scenario.md" {
            out.push(path);
        }
    }
}

fn collect_plugin_fixture_scenarios(plugins_dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    if !plugins_dir.is_dir() {
        return;
    }
    for entry in WalkDir::new(plugins_dir).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.file_name().and_then(|name| name.to_str()) != Some("scenario.md") {
            continue;
        }
        if under_symlink(root, &path).unwrap_or(true) {
            continue;
        }
        let rel = path.strip_prefix(plugins_dir).unwrap_or(&path);
        let parts: Vec<_> = rel.components().filter_map(|c| c.as_os_str().to_str()).collect();
        if parts.len() == 6
            && parts[1] == "skills"
            && parts[3] == "fixtures"
            && parts[5] == "scenario.md"
        {
            out.push(path);
        }
    }
}
