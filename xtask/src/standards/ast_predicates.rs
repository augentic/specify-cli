//! AST-based predicates. Currently a single predicate (`inline-dtos`)
//! implemented via a [`syn::visit::Visit`] walker that catches DTOs
//! declared inside `match` arms and other nested `Block`s.

use syn::visit::Visit;

/// `inline-dtos`: count `#[derive(Serialize)]` structs / enums declared
/// inside any [`syn::Block`] (function bodies, match arms, closures, …).
/// Returns 0 when the file cannot be parsed.
pub(super) fn count_inline_dtos(source: &str) -> u32 {
    let Ok(file) = syn::parse_file(source) else {
        return 0;
    };
    let mut visitor = InlineDtoVisitor { hits: 0, depth: 0 };
    visitor.visit_file(&file);
    visitor.hits
}

struct InlineDtoVisitor {
    hits: u32,
    depth: u32,
}

impl InlineDtoVisitor {
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
}

impl<'ast> Visit<'ast> for InlineDtoVisitor {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        self.depth += 1;
        syn::visit::visit_block(self, node);
        self.depth -= 1;
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.depth > 0 && Self::has_serialize(&node.attrs) {
            self.hits += 1;
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if self.depth > 0 && Self::has_serialize(&node.attrs) {
            self.hits += 1;
        }
        syn::visit::visit_item_enum(self, node);
    }
}
