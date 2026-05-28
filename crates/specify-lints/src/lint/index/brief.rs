//! `adapters/**/briefs/*.md` extractor per the standards-layer
//! contract §"Module additions".
//!
//! Emits one [`Brief`] fact per markdown file whose project-relative
//! path matches `adapters/{sources,targets}/<adapter>/briefs/<op>.md`.
//! The `sections` array captures `##` heading titles in document
//! order, sharing the fence and HTML-comment state machine from the
//! [`super::markdown`] extractor so fenced "## hidden" headings are
//! not mistaken for body sections. `body_line_count` is the count of
//! non-empty body lines after a leading frontmatter block is stripped.

use super::files::DiscoveredFile;
use super::markdown::extract_sections;
use crate::lint::{AdapterAxis, Brief};

/// Extract a [`Brief`] fact from a discovered file.
///
/// Returns `None` when the file does not live under
/// `adapters/{sources,targets}/<adapter>/briefs/<op>.md` or when the
/// file's inferred language is not markdown.
#[must_use]
pub fn extract(file: &DiscoveredFile) -> Option<Brief> {
    if file.language.as_deref() != Some("markdown") {
        return None;
    }
    let (axis, adapter, operation) = parse_brief_path(&file.relative)?;
    let sections: Vec<String> = extract_sections(file)
        .into_iter()
        .filter(|section| section.level == 2)
        .map(|section| section.title)
        .collect();
    let body_line_count = count_body_lines(file);
    Some(Brief {
        path: file.relative.clone(),
        axis,
        adapter: adapter.to_owned(),
        operation: operation.to_owned(),
        sections,
        body_line_count,
    })
}

/// Split `adapters/{sources,targets}/<adapter>/briefs/<op>.md` into
/// the `(axis, adapter, operation)` tuple.
fn parse_brief_path(relative: &str) -> Option<(AdapterAxis, &str, &str)> {
    let rest = relative.strip_prefix("adapters/")?;
    let (axis_str, rest) = rest.split_once('/')?;
    let axis = match axis_str {
        "sources" => AdapterAxis::Sources,
        "targets" => AdapterAxis::Targets,
        _ => return None,
    };
    let (adapter, rest) = rest.split_once('/')?;
    if adapter.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix("briefs/")?;
    let operation = rest.strip_suffix(".md")?;
    if operation.is_empty() || operation.contains('/') {
        return None;
    }
    Some((axis, adapter, operation))
}

fn count_body_lines(file: &DiscoveredFile) -> u32 {
    let text = file.text();
    let body = strip_frontmatter(&text);
    let count = body.lines().filter(|line| !line.trim().is_empty()).count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else {
        return text;
    };
    let mut search_from = 0;
    while let Some(rel) = rest[search_from..].find("\n---") {
        let pos = search_from + rel;
        let after = pos + "\n---".len();
        let tail = &rest[after..];
        if tail.is_empty() {
            return "";
        }
        if let Some(stripped) = tail.strip_prefix('\n') {
            return stripped;
        }
        if let Some(stripped) = tail.strip_prefix("\r\n") {
            return stripped;
        }
        search_from = after;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::FileKind;

    fn brief(relative: &str, body: &str) -> DiscoveredFile {
        DiscoveredFile {
            relative: relative.into(),
            kind: FileKind::Text,
            language: Some("markdown".into()),
            bytes: Some(body.as_bytes().to_vec()),
        }
    }

    #[test]
    fn captures_h2_sections_in_order() {
        let file = brief(
            "adapters/sources/intent/briefs/enumerate.md",
            "# Title\n\n## Inputs\n\nbody\n\n## Output contract\n\nmore body\n",
        );
        let brief = extract(&file).expect("brief extracted");
        assert_eq!(brief.axis, AdapterAxis::Sources);
        assert_eq!(brief.adapter, "intent");
        assert_eq!(brief.operation, "enumerate");
        assert_eq!(brief.sections, vec!["Inputs", "Output contract"]);
        assert!(brief.body_line_count >= 3);
    }

    #[test]
    fn rejects_paths_outside_briefs_dir() {
        let file = brief("adapters/sources/intent/references/foo.md", "## Heading\n");
        assert!(extract(&file).is_none());
    }
}
