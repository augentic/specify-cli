//! `display-serde-mirror`: hand-rolled `Display` impls that mirror the
//! serde wire shape via `match self { Self::Variant => "literal" }`.
//! The `kebab_enum!` macro is the supported replacement.

use std::collections::BTreeSet;

/// Count `impl Display for T` blocks where `T` derives `Serialize` in
/// the same file and the body is a `match self { ... }` whose arms map
/// unit variants to bare string literals (directly, via `f.write_str`,
/// or via `write!(f, "lit")`). Returns 0 when the file cannot be
/// parsed.
pub(super) fn count(source: &str) -> u32 {
    let Ok(file) = syn::parse_file(source) else {
        return 0;
    };
    let serializable = collect_serializable_types(&file);
    if serializable.is_empty() {
        return 0;
    }
    let mut hits = 0u32;
    for item in &file.items {
        if let syn::Item::Impl(item_impl) = item
            && let Some(name) = display_impl_target(item_impl)
            && serializable.contains(&name)
            && impl_body_is_kebab_mirror(item_impl)
        {
            hits = hits.saturating_add(1);
        }
    }
    hits
}

fn collect_serializable_types(file: &syn::File) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in &file.items {
        match item {
            syn::Item::Enum(it) if has_serialize(&it.attrs) => {
                out.insert(it.ident.to_string());
            }
            syn::Item::Struct(it) if has_serialize(&it.attrs) => {
                out.insert(it.ident.to_string());
            }
            _ => {}
        }
    }
    out
}

fn has_serialize(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("derive") {
            return false;
        }
        let mut found = false;
        let _ = a.parse_nested_meta(|meta| {
            if meta.path.is_ident("Serialize") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

/// If `it` is `impl [...] Display for T` (any path ending in
/// `Display`), return `T`'s identifier. We accept `Display`,
/// `fmt::Display`, `std::fmt::Display`, and `::core::fmt::Display`.
fn display_impl_target(it: &syn::ItemImpl) -> Option<String> {
    let (_, trait_path, _) = it.trait_.as_ref()?;
    let last = trait_path.segments.last()?;
    if last.ident != "Display" {
        return None;
    }
    if let syn::Type::Path(tp) = it.self_ty.as_ref()
        && let Some(seg) = tp.path.segments.last()
    {
        return Some(seg.ident.to_string());
    }
    None
}

fn impl_body_is_kebab_mirror(it: &syn::ItemImpl) -> bool {
    for impl_item in &it.items {
        if let syn::ImplItem::Fn(f) = impl_item
            && f.sig.ident == "fmt"
        {
            return block_has_kebab_match(&f.block);
        }
    }
    false
}

fn block_has_kebab_match(block: &syn::Block) -> bool {
    let mut finder = MatchFinder { hit: false };
    syn::visit::Visit::visit_block(&mut finder, block);
    finder.hit
}

struct MatchFinder {
    hit: bool,
}

impl<'ast> syn::visit::Visit<'ast> for MatchFinder {
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if !self.hit && is_match_on_self(&node.expr) && all_arms_unit_to_str_literal(&node.arms) {
            self.hit = true;
        }
        syn::visit::visit_expr_match(self, node);
    }
}

fn is_match_on_self(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Path(p) => p.path.is_ident("self"),
        syn::Expr::Reference(r) => is_match_on_self(&r.expr),
        syn::Expr::Unary(u) => matches!(u.op, syn::UnOp::Deref(_)) && is_match_on_self(&u.expr),
        _ => false,
    }
}

fn all_arms_unit_to_str_literal(arms: &[syn::Arm]) -> bool {
    if arms.is_empty() {
        return false;
    }
    arms.iter().all(|arm| is_unit_self_variant(&arm.pat) && expr_yields_only_literal(&arm.body))
}

fn is_unit_self_variant(pat: &syn::Pat) -> bool {
    if let syn::Pat::Path(p) = pat {
        let segs = &p.path.segments;
        segs.len() == 2 && segs[0].ident == "Self"
    } else {
        false
    }
}

/// True when `expr` is a bare string literal, `f.write_str("lit")`,
/// or `write!(f, "lit")` with no format arguments.
fn expr_yields_only_literal(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Lit(lit) => matches!(lit.lit, syn::Lit::Str(_)),
        syn::Expr::MethodCall(mc) => {
            mc.method == "write_str"
                && mc.args.len() == 1
                && matches!(
                    &mc.args[0],
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(_),
                        ..
                    })
                )
        }
        syn::Expr::Macro(m) => is_write_literal_macro(&m.mac),
        syn::Expr::Block(b) => {
            b.block.stmts.len() == 1
                && match &b.block.stmts[0] {
                    syn::Stmt::Expr(e, _) => expr_yields_only_literal(e),
                    _ => false,
                }
        }
        _ => false,
    }
}

fn is_write_literal_macro(mac: &syn::Macro) -> bool {
    let last = mac.path.segments.last().map(|s| s.ident.to_string());
    if !matches!(last.as_deref(), Some("write" | "writeln")) {
        return false;
    }
    // `write!(f, "lit")` parses as two args: an expression and a literal.
    // Refuse anything with format arguments (`write!(f, "{x}")` etc.).
    let Ok(args) = mac.parse_body_with(
        syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
    ) else {
        return false;
    };
    if args.len() != 2 {
        return false;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &args[1]
    else {
        return false;
    };
    !s.value().contains('{')
}
