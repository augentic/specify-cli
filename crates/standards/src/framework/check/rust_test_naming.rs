//! Flag sentence-length `#[test]` function names in the specify-cli workspace.

use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::relative_display;

const RULE: &str = "rust.test-fn-name-too-long";
const MAX_TEST_FN_LEN: usize = 40;

/// When the framework root is the specify-cli repo, reject test fns whose
/// names read like sentences (see `docs/standards/testing.md`).
pub struct RustTestNaming;

impl Check for RustTestNaming {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        let root = ctx.framework_root();
        if !root.join("crates/workflow").is_dir() || !root.join("src/runtime").is_dir() {
            return Vec::new();
        }

        let mut findings = Vec::new();
        walk_rust_tests(root, root, &mut findings);
        findings
    }
}

fn walk_rust_tests(root: &Path, dir: &Path, findings: &mut Vec<Diagnostic>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target" || n == "wasi-tools") {
                continue;
            }
            walk_rust_tests(root, &path, findings);
            continue;
        }
        if path.extension().is_some_and(|e| e == "rs") && is_test_rust_file(&path) {
            check_test_fn_names(root, &path, findings);
        }
    }
}

fn is_test_rust_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.ends_with("tests.rs") || path.components().any(|c| c.as_os_str() == "tests")
}

fn check_test_fn_names(root: &Path, path: &Path, findings: &mut Vec<Diagnostic>) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let rel = relative_display(root, path);
    let lines: Vec<&str> = content.lines().collect();
    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("fn ").or_else(|| trimmed.strip_prefix("async fn "))
        else {
            continue;
        };
        let Some((name, _)) = rest.split_once('(') else {
            continue;
        };
        if name.len() <= MAX_TEST_FN_LEN {
            continue;
        }
        if !preceded_by_test_attr(&lines, line_idx) {
            continue;
        }
        findings.push(framework_finding(
            RULE,
            format!(
                "test fn `{name}` is {} chars; shorten per docs/standards/testing.md (got {rel}:{})",
                name.len(),
                line_idx + 1
            ),
            Some(loc(path, line_idx + 1, None)),
        ));
    }
}

/// Walk upward over the attribute window above a `fn`, skipping blank lines and
/// other attributes (`#[ignore]`, `#[case(..)]`, …), and report whether a
/// `#[test]` / `#[tokio::test]` attribute introduces it.
fn preceded_by_test_attr(lines: &[&str], fn_idx: usize) -> bool {
    for prev in lines[..fn_idx].iter().rev() {
        let trimmed = prev.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.starts_with("#[") {
            return false;
        }
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::framework::context::Context;

    #[test]
    fn flags_long_test_fn_name() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("crates/workflow/src/foo/tests.rs");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "#[test]\nfn this_test_function_name_is_way_too_long_for_policy() {}\n")
            .expect("write");

        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("src/runtime")).expect("runtime dir");
        let ctx = Context::from_specify_cli_root(&root).expect("cli root");
        let findings = RustTestNaming.run(&ctx);
        assert!(
            findings.iter().any(|f| f.title.contains(RULE)),
            "expected long-name finding, got: {findings:?}"
        );
    }

    #[test]
    fn flags_async_tokio_test_behind_attributes() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("crates/workflow/src/foo/tests.rs");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &path,
            "#[tokio::test]\n#[ignore]\nasync fn this_async_test_function_name_is_clearly_too_long() {}\n",
        )
        .expect("write");

        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("src/runtime")).expect("runtime dir");
        let ctx = Context::from_specify_cli_root(&root).expect("cli root");
        let findings = RustTestNaming.run(&ctx);
        assert!(
            findings.iter().any(|f| f.title.contains(RULE)),
            "tokio::test behind an intervening attribute must still be flagged, got: {findings:?}"
        );
    }

    #[test]
    fn ignores_long_non_test_fn() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("crates/workflow/src/foo/tests.rs");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "fn this_helper_function_name_is_long_but_not_a_test_case() {}\n")
            .expect("write");

        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("src/runtime")).expect("runtime dir");
        let ctx = Context::from_specify_cli_root(&root).expect("cli root");
        assert!(RustTestNaming.run(&ctx).is_empty(), "non-test fns must not be flagged");
    }

    #[test]
    fn skips_plugin_framework_roots() {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("plugins")).expect("plugins");
        fs::create_dir_all(dir.path().join("adapters")).expect("adapters");
        let ctx = Context::from_framework_root(dir.path()).expect("framework root");
        assert!(RustTestNaming.run(&ctx).is_empty());
    }
}
