//! `unit-test-serde-roundtrip`: a `#[test]` function that exercises a
//! serde round-trip (`to_string` + `from_str`) on the same crate's
//! types.
//!
//! Round-trip tests usually belong in `tests/` integration tests where
//! the round-trip is driven end-to-end through a CLI command. Recurring
//! the pattern as a unit test in the same crate as the type is a soft
//! smell — the test asserts a property of `serde_*` rather than of the
//! domain. One hit per offending test function; allowlist when the
//! round-trip genuinely lives next to a custom Visitor or similar.

use syn::visit::Visit;

const SERDE_CRATES: &[&str] = &["serde_json", "serde_saphyr"];

/// Walk every `#[test] fn ...` (free function, or inside any nested
/// module — typically `#[cfg(test)] mod tests`) and count the ones
/// whose body contains a matching pair of `<crate>::to_string` and
/// `<crate>::from_str` calls for one of [`SERDE_CRATES`]. Returns 0
/// when the file cannot be parsed.
pub(super) fn count(source: &str) -> u32 {
    let Ok(file) = syn::parse_file(source) else {
        return 0;
    };
    let mut visitor = TestVisitor { hits: 0 };
    visitor.visit_file(&file);
    visitor.hits
}

struct TestVisitor {
    hits: u32,
}

impl<'ast> Visit<'ast> for TestVisitor {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if has_test_attr(&node.attrs) {
            let mut probe = SerdeCallProbe {
                has_to: false,
                has_from: false,
            };
            probe.visit_block(&node.block);
            if probe.has_to && probe.has_from {
                self.hits = self.hits.saturating_add(1);
            }
        }
        syn::visit::visit_item_fn(self, node);
    }
}

fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("test"))
}

struct SerdeCallProbe {
    has_to: bool,
    has_from: bool,
}

impl<'ast> Visit<'ast> for SerdeCallProbe {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = node.func.as_ref()
            && let Some(kind) = serde_path_kind(&p.path)
        {
            match kind {
                CallKind::ToString => self.has_to = true,
                CallKind::FromStr => self.has_from = true,
            }
        }
        syn::visit::visit_expr_call(self, node);
    }
}

#[derive(Copy, Clone)]
enum CallKind {
    ToString,
    FromStr,
}

/// Recognise `serde_json::to_string`, `serde_saphyr::from_str`, and the
/// `_pretty` variants. Single-segment idents are ignored; we only
/// match qualified paths so a local helper named `to_string` is not a
/// hit.
fn serde_path_kind(path: &syn::Path) -> Option<CallKind> {
    if path.segments.len() < 2 {
        return None;
    }
    let crate_seg = &path.segments[path.segments.len() - 2].ident;
    if !SERDE_CRATES.iter().any(|c| crate_seg == c) {
        return None;
    }
    let last = path.segments.last()?.ident.to_string();
    match last.as_str() {
        "to_string" | "to_string_pretty" => Some(CallKind::ToString),
        "from_str" => Some(CallKind::FromStr),
        _ => None,
    }
}
