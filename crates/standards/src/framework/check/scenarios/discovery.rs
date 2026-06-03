//! Scenario-file discovery across the test, target, and plugin trees.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::framework::context::Context;
use crate::framework::helpers::under_symlink;

pub(super) fn discover_scenario_candidates(ctx: &Context) -> Vec<PathBuf> {
    let root = ctx.framework_root();
    let mut candidates = Vec::new();

    collect_tests_suite_scenarios(&root.join("tests"), 2, &mut candidates);
    collect_tests_suite_scenarios(&root.join("tests").join("suites"), 2, &mut candidates);
    collect_plan_scenarios(&root.join("tests").join("plan"), &mut candidates);
    collect_target_scenarios(&ctx.targets_dir(), &mut candidates);
    collect_plugin_fixture_scenarios(&root.join("plugins"), root, &mut candidates);

    candidates.sort();
    candidates.dedup();
    candidates
}

fn collect_tests_suite_scenarios(dir: &Path, max_depth: usize, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    for entry in WalkDir::new(dir).max_depth(max_depth).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.file_name().and_then(|name| name.to_str()) != Some("scenario.md") {
            continue;
        }
        let rel = path.strip_prefix(dir).unwrap_or(&path);
        let parts: Vec<_> = rel.components().collect();
        if parts.len() == 2 {
            out.push(path);
        }
    }
}

fn collect_plan_scenarios(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    for entry in WalkDir::new(dir).max_depth(1).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let rel = path.strip_prefix(dir).unwrap_or(&path);
        if rel.components().count() == 1 {
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
