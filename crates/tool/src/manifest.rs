//! Manifest data types for declared Specify WASI tools.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One declared WASI tool.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// Tool name used by `specify tool run <name>`.
    pub name: String,
    /// Exact SemVer version string. Parsed during structural validation.
    pub version: String,
    /// Source of the WASI component bytes.
    pub source: ToolSource,
    /// Optional lower-case hex SHA-256 digest over the component bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Filesystem preopen requests.
    #[serde(default)]
    pub permissions: ToolPermissions,
}

/// Supported source locations for WASI component bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// Absolute local filesystem path.
    LocalPath(PathBuf),
    /// `file://` URI.
    FileUri(String),
    /// `https://` URI.
    HttpsUri(String),
}

impl ToolSource {
    /// Classify a manifest `source:` string into a supported source variant.
    ///
    /// # Errors
    ///
    /// Returns an error message when the source uses a relative path or an
    /// unsupported URI scheme.
    pub fn parse_wire(value: &str) -> Result<Self, String> {
        if value.starts_with("https://") {
            Ok(Self::HttpsUri(value.to_string()))
        } else if value.starts_with("file://") {
            Ok(Self::FileUri(value.to_string()))
        } else if Path::new(value).is_absolute() || looks_like_windows_absolute_path(value) {
            Ok(Self::LocalPath(PathBuf::from(value)))
        } else {
            Err(format!(
                "unsupported tool source `{value}`; expected an absolute path, file:// URI, or https:// URI"
            ))
        }
    }

    /// Return the manifest string form for this source.
    #[must_use]
    pub fn to_wire_string(&self) -> Cow<'_, str> {
        match self {
            Self::LocalPath(path) => path.to_string_lossy(),
            Self::FileUri(uri) | Self::HttpsUri(uri) => Cow::Borrowed(uri),
        }
    }
}

impl Serialize for ToolSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.to_wire_string().as_ref())
    }
}

impl<'de> Deserialize<'de> for ToolSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SourceVisitor;

        impl Visitor<'_> for SourceVisitor {
            type Value = ToolSource;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an absolute path, file:// URI, or https:// URI string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                ToolSource::parse_wire(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(SourceVisitor)
    }
}

/// Filesystem permissions requested by a tool.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolPermissions {
    /// Read-only preopen path templates.
    #[serde(default)]
    pub read: Vec<String>,
    /// Read-write preopen path templates.
    #[serde(default)]
    pub write: Vec<String>,
}

/// A `tools:` array as it appears in either declaration site.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolManifest {
    /// Declared tools.
    #[serde(default)]
    pub tools: Vec<Tool>,
}

/// Identifies which declaration site a tool came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolScope {
    /// Tool declared in `.specify/project.yaml`.
    Project {
        /// Project name from `project.yaml`.
        project_name: String,
    },
    /// Tool declared in a resolved capability's sidecar `tools.yaml`.
    Capability {
        /// Capability slug from `capability.yaml`.
        capability_slug: String,
        /// Resolved capability directory.
        capability_dir: PathBuf,
    },
}

fn looks_like_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_manifest_yaml_round_trips_all_source_variants() {
        let manifest = ToolManifest {
            tools: vec![
                Tool {
                    name: "local-tool".to_string(),
                    version: "1.0.0".to_string(),
                    source: ToolSource::LocalPath(PathBuf::from("/opt/specify/local.wasm")),
                    sha256: None,
                    permissions: ToolPermissions::default(),
                },
                Tool {
                    name: "file-tool".to_string(),
                    version: "1.0.1".to_string(),
                    source: ToolSource::FileUri("file:///opt/specify/file.wasm".to_string()),
                    sha256: None,
                    permissions: ToolPermissions {
                        read: vec!["$PROJECT_DIR/contracts".to_string()],
                        write: Vec::new(),
                    },
                },
                Tool {
                    name: "https-tool".to_string(),
                    version: "1.0.2".to_string(),
                    source: ToolSource::HttpsUri(
                        "https://example.com/specify/https.wasm".to_string(),
                    ),
                    sha256: Some(
                        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                            .to_string(),
                    ),
                    permissions: ToolPermissions {
                        read: Vec::new(),
                        write: vec!["$PROJECT_DIR/generated".to_string()],
                    },
                },
            ],
        };

        let yaml = serde_saphyr::to_string(&manifest).expect("serialize manifest");
        assert!(yaml.contains("source: /opt/specify/local.wasm"));
        assert!(yaml.contains("source: file:///opt/specify/file.wasm"));
        assert!(yaml.contains("source: https://example.com/specify/https.wasm"));

        let parsed: ToolManifest = serde_saphyr::from_str(&yaml).expect("parse manifest");
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn unsupported_source_fails_during_deserialize() {
        let err = serde_saphyr::from_str::<ToolManifest>(
            "tools:\n  - name: bad\n    version: 1.0.0\n    source: relative.wasm\n",
        )
        .expect_err("relative source must fail");
        assert!(err.to_string().contains("unsupported tool source"), "{err}");
    }
}
