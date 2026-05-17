//! DTO and parser for the `--sources` YAML batch file consumed by
//! `specify change survey --sources <file>`. Shape: `version: 1`,
//! `sources[].{key, path}`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use specify_error::Error;

/// Parsed representation of a `--sources` YAML file.
#[derive(Debug, Clone, Deserialize)]
pub struct SourcesFile {
    /// Schema version; must be `1`.
    pub version: u8,
    /// One row per legacy-code source.
    pub sources: Vec<SourceRow>,
}

/// Single entry inside `sources[].{key, path}`.
#[derive(Debug, Clone, Deserialize)]
pub struct SourceRow {
    /// Kebab-case identifier for the source.
    pub key: String,
    /// Path to the source root.
    pub path: PathBuf,
}

impl SourcesFile {
    /// Read and validate a `--sources` YAML file.
    ///
    /// # Errors
    ///
    /// - `sources-file-missing`: file does not exist.
    /// - `sources-file-malformed`: parse failure, wrong version, empty
    ///   sources list, or duplicate key.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let content = std::fs::read_to_string(path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Error::Diag {
                    code: "sources-file-missing",
                    detail: format!("sources file not found: {}", path.display()),
                }
            } else {
                Error::Io(err)
            }
        })?;
        Self::parse(&content)
    }

    /// Parse and validate the YAML body.
    ///
    /// # Errors
    ///
    /// Returns `sources-file-malformed` on any schema violation.
    pub fn parse(yaml: &str) -> Result<Self, Error> {
        let file: Self = serde_saphyr::from_str(yaml).map_err(|err| Error::Diag {
            code: "sources-file-malformed",
            detail: format!("sources file malformed: {err}"),
        })?;

        if file.version != 1 {
            return Err(Error::Diag {
                code: "sources-file-malformed",
                detail: format!(
                    "sources file malformed: unsupported version {} (expected 1)",
                    file.version
                ),
            });
        }

        if file.sources.is_empty() {
            return Err(Error::Diag {
                code: "sources-file-malformed",
                detail: "sources file malformed: sources list is empty".to_string(),
            });
        }

        let mut seen = HashSet::new();
        for row in &file.sources {
            if !seen.insert(&row.key) {
                return Err(Error::Diag {
                    code: "sources-file-malformed",
                    detail: format!("sources file malformed: duplicate key `{}`", row.key),
                });
            }
        }

        Ok(file)
    }
}
