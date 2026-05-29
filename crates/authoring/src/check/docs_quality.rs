use std::fs;

use crate::context::Context;
use crate::finding::{Check, Finding, Location};
use crate::helpers::{relative_display, resolve_markdown_asset, walk_markdown_files};

const RULE_SPECIFY_HISTORY_CITATION: &str = "docs.specify-history-citation-in-docs";
const RULE_MISSING_DIAGRAM: &str = "docs.missing-diagram-asset";
const RULE_TEXT_PIPELINE: &str = "docs.text-pipeline-diagram";

const TEXT_DIAGRAM_ROOTS: &[&str] =
    &["docs/explanation", "docs/orientation", "docs/tutorials", "docs/how-to"];

const TEXT_FENCE_ALLOWLIST: &[&str] = &[];

/// Flag retired Specify design-history citations in user-facing docs.
pub struct HistoryCitation;

impl Check for HistoryCitation {
    fn run(&self, ctx: &Context) -> Vec<Finding> {
        let mut findings = Vec::new();
        let markdown_roots = ["docs", "adapters", "plugins"];
        let root_files = ["AGENTS.md", "REVIEW.md"];

        for root_name in markdown_roots {
            let root = ctx.framework_root().join(root_name);
            if !root.is_dir() {
                continue;
            }

            let files = walk_markdown_files(ctx.framework_root(), &root).unwrap_or_default();

            for path in files {
                collect_specify_history_citations(ctx, &path, &mut findings);
            }
        }

        for file_name in root_files {
            let path = ctx.framework_root().join(file_name);
            if path.is_file() {
                collect_specify_history_citations(ctx, &path, &mut findings);
            }
        }

        findings
    }
}

fn collect_specify_history_citations(
    ctx: &Context, path: &std::path::Path, findings: &mut Vec<Finding>,
) {
    let rel = relative_display(ctx.framework_root(), path);
    if rel.starts_with("docs/assets/") {
        return;
    }

    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return,
    };

    for (line_idx, line) in content.lines().enumerate() {
        if has_specify_history_citation(line) {
            findings.push(Finding {
                rule_id: RULE_SPECIFY_HISTORY_CITATION,
                message: format!(
                    "Specify design-history citation in user-facing docs at {rel}:{} -- {} -- cite the live decision topic or strip",
                    line_idx + 1,
                    line.trim()
                ),
                location: Some(Location {
                    path: path.to_path_buf(),
                    line: line_idx + 1,
                    column: None,
                }),
            });
        }
    }
}

/// Ensure markdown SVG image references resolve to committed assets.
pub struct MissingDiagramAsset;

impl Check for MissingDiagramAsset {
    fn run(&self, ctx: &Context) -> Vec<Finding> {
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
                findings.push(Finding {
                    rule_id: RULE_MISSING_DIAGRAM,
                    message: format!("{rel} references missing SVG {target} (resolved {resolved})"),
                    location: Some(Location {
                        path: path.clone(),
                        line: 1,
                        column: None,
                    }),
                });
            }
        }

        findings
    }
}

/// Ban ```text flow diagrams under explanation/orientation/tutorials/how-to.
pub struct TextPipelineDiagram;

impl Check for TextPipelineDiagram {
    fn run(&self, ctx: &Context) -> Vec<Finding> {
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
                        findings.push(Finding {
                            rule_id: RULE_TEXT_PIPELINE,
                            message: format!(
                                "{rel} uses a ```text flow diagram — replace with SVG under docs/assets/diagrams/ (see docs/assets/diagrams/_STYLE.md)"
                            ),
                            location: Some(Location {
                                path: path.clone(),
                                line: 1,
                                column: None,
                            }),
                        });
                    }
                }
            }
        }

        findings
    }
}

fn has_specify_history_citation(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let retired_tree = ["rfc", "s/"].concat();
    let retired_token = ["rfc", "-"].concat();
    if lower.contains(&retired_tree) || contains_numbered_token(&lower, &retired_token) {
        return true;
    }

    let mut search = text;
    let retired_upper = ["R", "FC"].concat();
    while let Some(idx) = search.find(&retired_upper) {
        let rest = &search[idx + retired_upper.len()..];
        if let Some(number) =
            parse_design_history_number(rest.strip_prefix('-').or_else(|| rest.strip_prefix(' ')))
            && number < 100
        {
            return true;
        }
        search = advance_one(rest);
    }

    false
}

fn contains_numbered_token(text: &str, token: &str) -> bool {
    let mut search = text;
    while let Some(idx) = search.find(token) {
        let rest = &search[idx + token.len()..];
        if parse_design_history_number(Some(rest)).is_some_and(|number| number < 100) {
            return true;
        }
        search = advance_one(rest);
    }
    false
}

fn advance_one(text: &str) -> &str {
    text.char_indices().nth(1).map_or("", |(idx, _)| &text[idx..])
}

fn parse_design_history_number(rest: Option<&str>) -> Option<u32> {
    let rest = rest?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() { None } else { digits.parse().ok() }
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
    fn history_citation_detects_links() {
        let line = format!(
            "See [details]({}s/done/{}-5.md) for more.",
            "r".to_owned() + "fc",
            "r".to_owned() + "fc"
        );
        assert!(has_specify_history_citation(&line));
    }

    #[test]
    fn history_citation_detects_numbered() {
        assert!(has_specify_history_citation(&format!(
            "See {}-5 for details",
            "R".to_owned() + "FC"
        )));
        assert!(has_specify_history_citation(&format!(
            "See {} 5 for details",
            "R".to_owned() + "FC"
        )));
        assert!(!has_specify_history_citation(&format!(
            "Use {} 3339 timestamps and {} 5322 email syntax",
            "R".to_owned() + "FC",
            "R".to_owned() + "FC"
        )));
    }

    #[test]
    fn find_fence_blocks_collects_content() {
        let content = "Intro\n\n```text\nA -> B\n```\n\nOutro";
        let blocks = find_text_fence_blocks(content);
        assert_eq!(blocks.len(), 1);
        assert!(has_text_diagram_arrow(&blocks[0]));
    }
}
