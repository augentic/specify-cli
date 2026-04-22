//! Brief frontmatter + body parsing.
//!
//! Briefs are markdown files with a `---`-delimited YAML frontmatter
//! block at the top. The frontmatter shape is enforced here;
//! cross-brief invariants (ids matching the pipeline, `needs`/`tracks`
//! dependencies) live in `pipeline.rs`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

/// Parsed frontmatter of a brief markdown file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BriefFrontmatter {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub generates: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub tracks: Option<String>,
}

/// A parsed brief: the path it was loaded from, its frontmatter, and the
/// remaining markdown body.
#[derive(Debug, Clone)]
pub struct Brief {
    pub path: PathBuf,
    pub frontmatter: BriefFrontmatter,
    pub body: String,
}

impl Brief {
    /// Read `path` and parse it via [`Brief::parse`].
    pub fn load(path: &Path) -> Result<Self, Error> {
        let contents = std::fs::read_to_string(path).map_err(|err| {
            Error::Config(format!("failed to read brief {}: {err}", path.display()))
        })?;
        Self::parse(path, &contents)
    }

    /// Parse an in-memory brief. The file must begin with `---\n`,
    /// followed by YAML frontmatter, a closing `---` on its own line,
    /// and then the markdown body.
    pub fn parse(path: &Path, contents: &str) -> Result<Self, Error> {
        let stripped = contents
            .strip_prefix("---\n")
            .or_else(|| contents.strip_prefix("---\r\n"))
            .ok_or_else(|| {
                Error::Config(format!(
                    "brief {} is missing a leading `---` frontmatter delimiter",
                    path.display()
                ))
            })?;

        let (frontmatter_text, body) = split_on_closing_delimiter(stripped).ok_or_else(|| {
            Error::Config(format!(
                "brief {} has an opening `---` but no closing `---` delimiter",
                path.display()
            ))
        })?;

        let frontmatter: BriefFrontmatter =
            serde_yaml::from_str(frontmatter_text).map_err(|err| {
                Error::Config(format!(
                    "brief {} has invalid frontmatter YAML: {err}",
                    path.display()
                ))
            })?;

        Ok(Brief {
            path: path.to_path_buf(),
            frontmatter,
            body: body.to_string(),
        })
    }
}

/// Given the text *after* the leading `---\n`, split it into
/// `(frontmatter, body)` at the first closing `---` on its own line.
pub(crate) fn split_on_closing_delimiter(after_open: &str) -> Option<(&str, &str)> {
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
