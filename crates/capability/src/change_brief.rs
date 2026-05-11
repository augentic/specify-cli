//! Change brief parser — operator-authored brief.
//!
//! `change.md` (at the repo root) is a markdown document with a
//! `---`-delimited YAML frontmatter block at the top. The frontmatter
//! shape is enforced here (`#[serde(deny_unknown_fields)]` +
//! [`ChangeBrief::parse_str`] invariants); the body is captured
//! verbatim and **not** parsed in v1. A future RFC may land structured
//! body parsing, but today's consumers treat the body as prose.
//!
//! No JSON schema file ships for v1 per the RFC — the shape is enforced
//! directly in code.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::{Error, is_kebab};

use crate::brief::split_on_closing_delimiter;

/// Filename of the operator brief at the repo root.
///
/// Written by `specify change create <name>`.
pub const FILENAME: &str = "change.md";

/// In-memory representation of `change.md` (at the repo root).
///
/// Structured frontmatter (YAML) + free-form body (markdown). The
/// body is preserved byte-for-byte so round-tripping is faithful;
/// structured body interpretation is explicitly deferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeBrief {
    /// Parsed YAML frontmatter.
    pub frontmatter: ChangeFrontmatter,
    /// Markdown body captured verbatim. Not parsed further in v1.
    pub body: String,
}

/// Parsed frontmatter of `change.md`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeFrontmatter {
    /// Kebab-case change name.
    pub name: String,
    /// Seed inputs. Optional — may be absent or empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<ChangeInput>,
}

/// One entry in [`ChangeFrontmatter::inputs`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeInput {
    /// Relative or absolute path. Stored verbatim; resolution happens
    /// downstream in `/spec:analyze` (not in this crate).
    pub path: String,
    /// Closed enum — see [`InputKind`].
    pub kind: InputKind,
}

/// Closed enum over the kinds of seed input a brief can declare.
///
/// Unknown values are a hard parse error (serde-driven) so typos
/// like `kind: documenttation` fail fast.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputKind {
    /// Existing source tree that will be migrated/extracted.
    LegacyCode,
    /// Human-authored references (runbooks, PDFs, design docs).
    Documentation,
}

impl ChangeBrief {
    /// Absolute path to `<project_dir>/change.md`. The operator brief
    /// lives at the repo root.
    #[must_use]
    pub fn path(project_dir: &Path) -> PathBuf {
        project_dir.join(FILENAME)
    }

    /// Load + shape-validate the change brief.
    ///
    /// - `Ok(None)` — the file is absent. The brief is optional and a
    ///   missing file is *not* an error.
    /// - `Ok(Some(_))` — parsed and shape-validated.
    /// - `Err(_)` — malformed YAML, unknown keys, kebab-case / required-
    ///   field / empty-path violations.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(project_dir: &Path) -> Result<Option<Self>, Error> {
        let path = Self::path(project_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path).map_err(|err| Error::Diag {
            code: "change-brief-read-failed",
            detail: format!("failed to read {}: {err}", path.display()),
        })?;
        Self::parse_str(&content).map(Some)
    }

    /// Parse an in-memory brief: YAML frontmatter between `---`
    /// delimiters followed by a verbatim markdown body.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn parse_str(content: &str) -> Result<Self, Error> {
        let after_open = content
            .strip_prefix("---\n")
            .or_else(|| content.strip_prefix("---\r\n"))
            .ok_or_else(|| Error::Diag {
                code: "change-brief-frontmatter-missing",
                detail: format!("{FILENAME}: missing YAML frontmatter"),
            })?;
        let (frontmatter_text, body) =
            split_on_closing_delimiter(after_open).ok_or_else(|| Error::Diag {
                code: "change-brief-frontmatter-unclosed",
                detail: format!("{FILENAME}: opening `---` has no closing `---` delimiter"),
            })?;

        let frontmatter: ChangeFrontmatter =
            serde_saphyr::from_str(frontmatter_text).map_err(|err| Error::Diag {
                code: "change-brief-frontmatter-malformed",
                detail: format!("{FILENAME}: invalid frontmatter: {err}"),
            })?;

        let brief = Self {
            frontmatter,
            body: body.to_string(),
        };
        brief.validate_shape()?;
        Ok(brief)
    }

    /// Invariants serde cannot express: kebab-case name, non-empty
    /// input paths.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn validate_shape(&self) -> Result<(), Error> {
        if self.frontmatter.name.is_empty() {
            return Err(Error::Diag {
                code: "change-brief-name-empty",
                detail: format!("{FILENAME}: name is empty"),
            });
        }
        if !is_kebab(&self.frontmatter.name) {
            return Err(Error::Diag {
                code: "change-brief-name-not-kebab",
                detail: format!(
                    "{FILENAME}: name `{}` must be kebab-case \
                     (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)",
                    self.frontmatter.name
                ),
            });
        }
        for (idx, input) in self.frontmatter.inputs.iter().enumerate() {
            if input.path.is_empty() {
                return Err(Error::Diag {
                    code: "change-brief-input-path-empty",
                    detail: format!("{FILENAME}: inputs[{idx}].path is empty"),
                });
            }
        }
        Ok(())
    }

    /// Render the canonical `change.md` template for the given
    /// kebab-case change name. Byte-stable — the `change create` CLI
    /// verb compares against a golden fixture.
    #[must_use]
    #[expect(
        clippy::literal_string_with_formatting_args,
        reason = "Template uses {name}/{title} as plain-text placeholders, not format args."
    )]
    pub fn template(name: &str) -> String {
        CHANGE_TEMPLATE.replace("{name}", name).replace("{title}", &title_case(name))
    }
}

/// Canonical template shipped by `specify change create`. The
/// golden-fixture test pins this byte-for-byte; any edit here must be
/// mirrored in the test constant.
const CHANGE_TEMPLATE: &str = "\
---
name: {name}
inputs: []
---

# {title}

<!-- One-paragraph framing of what this change is trying to
     achieve. Plans reference this brief via `change.md`. -->
";

/// Title-case transform used by [`ChangeBrief::template`]:
/// `traffic-modernisation` → `Traffic modernisation`. Dashes become
/// spaces, the very first ASCII character is uppercased, everything
/// else is left as-is.
fn title_case(name: &str) -> String {
    let with_spaces: String = name.chars().map(|c| if c == '-' { ' ' } else { c }).collect();
    let mut chars = with_spaces.chars();
    chars
        .next()
        .map_or_else(String::new, |first| first.to_ascii_uppercase().to_string() + chars.as_str())
}
