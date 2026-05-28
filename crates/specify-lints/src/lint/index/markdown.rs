//! Markdown structure + link scan per the `WorkspaceModel` entity families.
//!
//! Two byte-stable passes over each markdown file's text:
//!
//! - [`extract_sections`] records ATX-style headings (`#` … `######`),
//!   tracks each heading's `(line_start, line_end, body_line_count)`,
//!   and closes a section when the next same-or-shallower heading
//!   begins (or at EOF).
//! - [`extract_links`] scans for `[label](target)` markdown links.
//!
//! Both passes share the same fence and HTML-comment state machine:
//! a heading or a link that lands inside a triple-backtick or `~~~`
//! fence or inside a `<!-- … -->` block is skipped. The §"Stability"
//! rule requires byte-stable output across runs, not perfect markdown
//! semantics — the scanner trades `CommonMark` edge cases for a
//! single flat pass with no parser dependency.

use std::borrow::Cow;

use super::files::DiscoveredFile;
use crate::lint::{MarkdownLink, MarkdownSection};

/// Extract ATX-style section facts from a markdown file. Non-markdown
/// files return an empty vector.
#[must_use]
pub fn extract_sections(file: &DiscoveredFile) -> Vec<MarkdownSection> {
    if file.language.as_deref() != Some("markdown") {
        return Vec::new();
    }
    let text = file.text();
    let lines: Vec<&str> = text.split('\n').collect();
    let total_lines = u32::try_from(lines.len()).unwrap_or(u32::MAX);

    let mut state = ScanState::default();
    let mut headings: Vec<(u8, String, u32)> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        let Some(scanned) = state.process(line) else {
            continue;
        };
        if let Some((level, title)) = parse_heading(scanned.as_ref()) {
            headings.push((level, title, line_no));
        }
    }

    let mut sections: Vec<MarkdownSection> = Vec::with_capacity(headings.len());
    for (i, (level, title, line_start)) in headings.iter().enumerate() {
        let line_end = headings
            .iter()
            .skip(i + 1)
            .find(|(next_level, _, _)| next_level <= level)
            .map_or(total_lines, |(_, _, next_start)| next_start.saturating_sub(1));
        let line_end = line_end.max(*line_start);
        let body_line_count = line_end.saturating_sub(*line_start);
        sections.push(MarkdownSection {
            path: file.relative.clone(),
            level: *level,
            title: title.clone(),
            line_start: *line_start,
            line_end,
            body_line_count,
        });
    }
    sections
}

/// Extract `[label](target)` link facts from a markdown file.
/// `resolves` is left at `None`; the umbrella's sequential pass fills
/// it in by checking each `to_raw` against the discovered file set.
#[must_use]
pub fn extract_links(file: &DiscoveredFile) -> Vec<MarkdownLink> {
    if file.language.as_deref() != Some("markdown") {
        return Vec::new();
    }
    let text = file.text();
    let mut state = ScanState::default();
    let mut links: Vec<MarkdownLink> = Vec::new();
    for (idx, line) in text.split('\n').enumerate() {
        let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        let Some(scanned) = state.process(line) else {
            continue;
        };
        scan_line_for_links(scanned.as_ref(), &file.relative, line_no, &mut links);
    }
    links
}

#[derive(Default)]
struct ScanState {
    in_fence: bool,
    fence_marker: Option<String>,
    in_comment: bool,
}

impl ScanState {
    /// Advance the state with `line` and return the bytes that
    /// section / link scanners should consume:
    ///
    /// - `None` when the line is entirely inside a fence, entirely
    ///   inside an HTML comment, or is itself the opening/closing
    ///   delimiter of a fence or multi-line comment.
    /// - `Some(Cow::Borrowed(line))` when the line is plain markdown
    ///   with no inline HTML comment to strip.
    /// - `Some(Cow::Owned(cleaned))` when the line carries an inline
    ///   `<!-- … -->` region that was stripped before scanning.
    fn process<'a>(&mut self, line: &'a str) -> Option<Cow<'a, str>> {
        let trimmed_start = line.trim_start();
        if self.in_fence {
            if let Some(marker) = self.fence_marker.as_deref()
                && trimmed_start.starts_with(marker)
                && trimmed_start.trim_end().eq(marker)
            {
                self.in_fence = false;
                self.fence_marker = None;
            }
            return None;
        }
        if self.in_comment {
            if let Some(idx) = line.find("-->") {
                self.in_comment = false;
                let after = &line[idx + 3..];
                if let Some(marker) = detect_fence_open(after.trim_start()) {
                    self.in_fence = true;
                    self.fence_marker = Some(marker);
                }
            }
            return None;
        }
        if let Some(marker) = detect_fence_open(trimmed_start) {
            self.in_fence = true;
            self.fence_marker = Some(marker);
            return None;
        }

        // Strip inline `<!-- … -->` regions; only an open without a
        // matching close on the same line flips us into multi-line
        // comment state.
        if !line.contains("<!--") {
            return Some(Cow::Borrowed(line));
        }
        let mut buf = String::with_capacity(line.len());
        let mut rest = line;
        loop {
            let Some(open) = rest.find("<!--") else {
                buf.push_str(rest);
                break;
            };
            buf.push_str(&rest[..open]);
            let after_open = &rest[open + 4..];
            if let Some(close_rel) = after_open.find("-->") {
                rest = &after_open[close_rel + 3..];
            } else {
                self.in_comment = true;
                break;
            }
        }
        Some(Cow::Owned(buf))
    }
}

/// Detect a fence open marker (` ``` ` or `~~~`, optionally followed
/// by an info string) at the start of `line`. Returns the marker
/// string (`"```"` or `"~~~"`) so the matching close can be detected.
fn detect_fence_open(line: &str) -> Option<String> {
    for marker in ["```", "~~~"] {
        if line.starts_with(marker) {
            return Some(marker.to_owned());
        }
    }
    None
}

/// Parse an ATX heading `#{1,6} title`. Trailing `#`s and surrounding
/// whitespace are stripped per `CommonMark` §"ATX headings".
fn parse_heading(line: &str) -> Option<(u8, String)> {
    let mut chars = line.chars();
    let mut hashes: u8 = 0;
    for ch in chars.by_ref() {
        if ch == '#' {
            hashes += 1;
            if hashes > 6 {
                return None;
            }
        } else if ch == ' ' || ch == '\t' {
            break;
        } else {
            return None;
        }
    }
    if hashes == 0 {
        return None;
    }
    let remainder: String = chars.collect();
    let mut title = remainder.trim().to_owned();
    while title.ends_with('#') {
        title.pop();
    }
    let title = title.trim().to_owned();
    if title.is_empty() { None } else { Some((hashes, title)) }
}

/// Scan a single line for `[label](target)` markdown links. The
/// scanner is intentionally simple: it ignores backslash escapes,
/// reference-style links, and image links (which already mismatch the
/// `[label]` shape via the leading `!`).
fn scan_line_for_links(line: &str, from_path: &str, line_no: u32, out: &mut Vec<MarkdownLink>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        // Image links (`![alt](src)`) share the `[…](…)` shape but
        // are not link facts; skip when preceded by `!`.
        if i > 0 && bytes[i - 1] == b'!' {
            i += 1;
            continue;
        }
        let Some(close_bracket) = find_unescaped(bytes, i + 1, b']') else {
            break;
        };
        if close_bracket + 1 >= bytes.len() || bytes[close_bracket + 1] != b'(' {
            i = close_bracket + 1;
            continue;
        }
        let Some(close_paren) = find_unescaped(bytes, close_bracket + 2, b')') else {
            break;
        };
        let target = &line[close_bracket + 2..close_paren];
        let target = target.trim();
        if !target.is_empty() {
            out.push(MarkdownLink {
                from_path: from_path.to_owned(),
                to_raw: target.to_owned(),
                line: line_no,
                resolves: None,
            });
        }
        i = close_paren + 1;
    }
}

fn find_unescaped(bytes: &[u8], start: usize, needle: u8) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::FileKind;

    fn markdown(relative: &str, body: &str) -> DiscoveredFile {
        DiscoveredFile {
            relative: relative.into(),
            kind: FileKind::Text,
            language: Some("markdown".into()),
            bytes: Some(body.as_bytes().to_vec()),
        }
    }

    #[test]
    fn sections_capture_atx_headings() {
        let f = markdown("doc.md", "# Top\nintro line\n\n## Sub\nbody one\nbody two\n## Sibling\n");
        let sections = extract_sections(&f);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].title, "Top");
        assert_eq!(sections[0].level, 1);
        assert_eq!(sections[0].line_start, 1);
        // "Top" closes when the next h1-or-shallower heading appears —
        // there is no other h1, so it absorbs everything through EOF.
        assert_eq!(sections[1].title, "Sub");
        assert_eq!(sections[1].line_start, 4);
        assert_eq!(sections[1].line_end, 6);
        assert_eq!(sections[1].body_line_count, 2);
    }

    #[test]
    fn sections_skip_fenced_headings() {
        let f = markdown("doc.md", "# Real\n```rust\n# not a heading\n```\n## Real Sub\n");
        let sections = extract_sections(&f);
        let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Real", "Real Sub"]);
    }

    #[test]
    fn sections_skip_html_comments() {
        let f = markdown("doc.md", "# Visible\n<!--\n# hidden\n-->\n## Sub\n");
        let sections = extract_sections(&f);
        let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Visible", "Sub"]);
    }

    #[test]
    fn links_record_relative_targets() {
        let f = markdown("doc.md", "intro [first](./a.md) and [second](https://example.com)\n");
        let links = extract_links(&f);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].to_raw, "./a.md");
        assert_eq!(links[0].line, 1);
        assert_eq!(links[1].to_raw, "https://example.com");
    }

    #[test]
    fn links_skip_fences_and_comments() {
        let f = markdown(
            "doc.md",
            "real [one](./a.md)\n```\n[fake](nope)\n```\n<!-- [also-fake](nope) -->\n",
        );
        let links = extract_links(&f);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].to_raw, "./a.md");
    }

    #[test]
    fn image_links_are_ignored() {
        let f = markdown("doc.md", "logo: ![alt](./logo.png) and [real](./a.md)\n");
        let links = extract_links(&f);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].to_raw, "./a.md");
    }
}
