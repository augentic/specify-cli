//! Scenario-file discovery across acceptance suites, target tests, and plugin fixtures.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::framework::context::Context;
use crate::framework::helpers::under_symlink;

pub(super) fn discover_scenario_candidates(ctx: &Context) -> Vec<PathBuf> {
    let root = ctx.framework_root();
    let mut candidates = Vec::new();

    collect_acceptance_suite_scenarios(&root.join("acceptance").join("suites"), &mut candidates);
    collect_target_scenarios(&ctx.targets_dir(), &mut candidates);
    collect_plugin_fixture_scenarios(&root.join("plugins"), root, &mut candidates);

    candidates.sort();
    candidates.dedup();
    candidates
}

/// Collects `acceptance/suites/<pack>/scenario.md` (umbrella) and
/// `acceptance/suites/<pack>/<id>/scenario.md` (per-scenario).
fn collect_acceptance_suite_scenarios(suites_dir: &Path, out: &mut Vec<PathBuf>) {
    if !suites_dir.is_dir() {
        return;
    }
    for entry in WalkDir::new(suites_dir).max_depth(3).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.file_name().and_then(|name| name.to_str()) != Some("scenario.md") {
            continue;
        }
        let rel = path.strip_prefix(suites_dir).unwrap_or(&path);
        let parts: Vec<_> = rel.components().collect();
        if parts.len() == 2 || parts.len() == 3 {
            out.push(path);
        }
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
