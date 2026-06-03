use std::fs;

use specify_diagnostics::Diagnostic;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::{relative_display, resolve_markdown_asset, walk_markdown_files};

const RULE_MISSING_DIAGRAM: &str = "docs.missing-diagram-asset";
const RULE_TEXT_PIPELINE: &str = "docs.text-pipeline-diagram";

const TEXT_DIAGRAM_ROOTS: &[&str] =
    &["docs/explanation", "docs/orientation", "docs/tutorials", "docs/how-to"];

const TEXT_FENCE_ALLOWLIST: &[&str] = &[];

/// Ensure markdown SVG image references resolve to committed assets.
pub struct MissingDiagramAsset;

impl Check for MissingDiagramAsset {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        let root = ctx.framework_root().join("docs");
        if !root.is_dir() {
            return Vec::new();
        }

        let mut findings = Vec::new();
        let files = walk_markdown_files(ctx.framework_root(), &root).unwrap_or_default();

        for path in files {
            let rel = relative_display(ctx.framework_root(), &path);
            if rel.starts_with("docs/book/") {
                continue;
            }
            if rel == "docs/assets/diagrams/_STYLE.md" || rel == "docs/standards/doc-authoring.md" {
                continue;
            }

            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(_) => continue,
            };

            for target in find_svg_image_refs(&content) {
                if target.starts_with("http://") || target.starts_with("https://") {
                    continue;
                }
                let abs = resolve_markdown_asset(&path, &target);
                if abs.is_file() {
                    continue;
                }
                let resolved = relative_display(ctx.framework_root(), &abs);
                findings.push(framework_finding(
                    RULE_MISSING_DIAGRAM,
                    format!("{rel} references missing SVG {target} (resolved {resolved})"),
                    Some(loc(path.clone(), 1, None)),
                ));
            }
        }

        findings
    }
}

/// Ban ```text flow diagrams under explanation/orientation/tutorials/how-to.
pub struct TextPipelineDiagram;

impl Check for TextPipelineDiagram {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        let mut findings = Vec::new();

        for rel_root in TEXT_DIAGRAM_ROOTS {
            let root = ctx.framework_root().join(rel_root);
            if !root.is_dir() {
                continue;
            }

            let files = walk_markdown_files(ctx.framework_root(), &root).unwrap_or_default();
            for path in files {
                let rel = relative_display(ctx.framework_root(), &path);
                if TEXT_FENCE_ALLOWLIST.contains(&rel.as_str()) {
                    continue;
                }

                let content = match fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(_) => continue,
                };

                for block in find_text_fence_blocks(&content) {
                    if has_text_diagram_arrow(&block) {
                        findings.push(framework_finding(
                            RULE_TEXT_PIPELINE,
                            format!(
                                "{rel} uses a ```text flow diagram — replace with SVG under docs/assets/diagrams/ (see docs/assets/diagrams/_STYLE.md)"
                            ),
                            Some(loc(path.clone(), 1, None)),
                        ));
                    }
                }
            }
        }

        findings
    }
}

fn find_svg_image_refs(content: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut search = content;
    while let Some(start) = search.find("![") {
        let after = &search[start + 2..];
        let Some(bracket_end) = after.find(']') else {
            search = &search[start + 2..];
            continue;
        };
        let after_bracket = &after[bracket_end + 1..];
        let Some(target) =
            after_bracket.strip_prefix('(').and_then(|inner| inner.split(')').next())
        else {
            search = &search[start + 2..];
            continue;
        };
        if target.ends_with(".svg") {
            refs.push(target.to_string());
        }
        search = &after_bracket[target.len() + 2..];
    }
    refs
}

fn find_text_fence_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("```text") {
        let after_open = &rest[start + "```text".len()..];
        let Some(end) = after_open.find("```") else {
            break;
        };
        blocks.push(after_open[..end].to_string());
        rest = &after_open[end + "```".len()..];
    }
    blocks
}

fn has_text_diagram_arrow(block: &str) -> bool {
    block.contains("->") || block.contains('→')
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn find_fence_blocks_collects_content() {
        let content = "Intro\n\n```text\nA -> B\n```\n\nOutro";
        let blocks = find_text_fence_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert!(has_text_diagram_arrow(&blocks[0]));
    }
}
