//! Brief frontmatter + body parsing for `---`-delimited markdown.
//!
//! RFC-25 briefs carry only `id` and `description` — the legacy
//! 1.x cross-brief contract (`generates`, `needs`, `tracks`) was
//! retired with the move to operation-keyed `briefs.<op>` (see
//! [`SourceAdapter`] / [`TargetAdapter`] in
//! `crates/domain/src/adapter/core.rs`).
//!
//! [`SourceAdapter`]: crate::adapter::SourceAdapter
//! [`TargetAdapter`]: crate::adapter::TargetAdapter

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

/// Parsed frontmatter of a brief markdown file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BriefFrontmatter {
    /// Brief identifier matching the adapter manifest's `briefs.<id>` key.
    pub id: String,
    /// Human-readable description of this brief's purpose.
    pub description: String,
}

/// A parsed brief: the path it was loaded from, its frontmatter, and the
/// remaining markdown body.
#[derive(Debug, Clone)]
pub struct Brief {
    /// Filesystem path the brief was loaded from.
    pub path: PathBuf,
    /// Parsed YAML frontmatter.
    pub frontmatter: BriefFrontmatter,
    /// Markdown body after the closing `---` delimiter.
    pub body: String,
}

impl Brief {
    /// Read `path` and parse it via [`Brief::parse`].
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let contents = std::fs::read_to_string(path).map_err(|err| Error::Diag {
            code: "brief-read-failed",
            detail: format!("failed to read brief {}: {err}", path.display()),
        })?;
        Self::parse(path, &contents)
    }

    /// Parse an in-memory brief. The file must begin with `---\n`,
    /// followed by YAML frontmatter, a closing `---` on its own line,
    /// and then the markdown body.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn parse(path: &Path, contents: &str) -> Result<Self, Error> {
        let stripped = contents
            .strip_prefix("---\n")
            .or_else(|| contents.strip_prefix("---\r\n"))
            .ok_or_else(|| Error::Diag {
                code: "brief-frontmatter-missing",
                detail: format!(
                    "brief {} is missing a leading `---` frontmatter delimiter",
                    path.display()
                ),
            })?;

        let (frontmatter_text, body) =
            split_on_closing_delimiter(stripped).ok_or_else(|| Error::Diag {
                code: "brief-frontmatter-unclosed",
                detail: format!(
                    "brief {} has an opening `---` but no closing `---` delimiter",
                    path.display()
                ),
            })?;

        let frontmatter: BriefFrontmatter =
            serde_saphyr::from_str(frontmatter_text).map_err(|err| Error::Diag {
                code: "brief-frontmatter-malformed",
                detail: format!("brief {} has invalid frontmatter YAML: {err}", path.display()),
            })?;

        Ok(Self {
            path: path.to_path_buf(),
            frontmatter,
            body: body.to_string(),
        })
    }
}

/// Given the text *after* the leading `---\n`, split it into
/// `(frontmatter, body)` at the first closing `---` on its own line.
#[must_use]
pub fn split_on_closing_delimiter(after_open: &str) -> Option<(&str, &str)> {
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            let frontmatter = &after_open[..offset];
            let body_start = offset + line.len();
            let body = &after_open[body_start..];
            return Some((frontmatter, body));
        }
        offset += line.len();
    }
    None
}
