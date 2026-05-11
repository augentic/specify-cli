//! Manifest data types for declared Specify WASI tools.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde::de::{self, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One declared WASI tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// Tool name used by `specify tool run <name>`.
    pub name: String,
    /// Exact SemVer version string. Parsed during structural validation.
    pub version: String,
    /// Source of the WASI component bytes.
    pub source: ToolSource,
    /// Optional lower-case hex SHA-256 digest over the component bytes.
    pub sha256: Option<String>,
    /// Filesystem preopen requests.
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
    /// Exact wasm-pkg package request.
    Package(PackageRequest),
}

/// Exact wasm-pkg package request used by first-party tool declarations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageRequest {
    /// Package namespace before `:`.
    pub namespace: String,
    /// Package name after `:` and before `@`.
    pub name: String,
    /// Exact version after `@`.
    pub version: String,
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
        } else if looks_like_package_request(value) {
            Ok(Self::Package(PackageRequest::parse(value)))
        } else {
            Err(format!(
                "unsupported tool source `{value}`; expected an absolute path, file:// URI, https:// URI, or wasm package request"
            ))
        }
    }

    /// Return the manifest string form for this source.
    #[must_use]
    pub fn to_wire_string(&self) -> Cow<'_, str> {
        match self {
            Self::LocalPath(path) => path.to_string_lossy(),
            Self::FileUri(uri) | Self::HttpsUri(uri) => Cow::Borrowed(uri),
            Self::Package(package) => Cow::Owned(package.to_wire_string()),
        }
    }
}

impl PackageRequest {
    /// Parse a package request string.
    ///
    /// Parsing is intentionally permissive so structural validation can emit
    /// stable rule ids for unsupported namespaces and non-SemVer versions.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        let (package, version) = value.split_once('@').unwrap_or((value, ""));
        let (namespace, name) = package.split_once(':').unwrap_or(("", package));
        Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
            version: version.to_string(),
        }
    }

    /// Return the package name without the version suffix.
    #[must_use]
    pub fn name_ref(&self) -> String {
        format!("{}:{}", self.namespace, self.name)
    }

    /// Return the manifest string form.
    #[must_use]
    pub fn to_wire_string(&self) -> String {
        format!("{}@{}", self.name_ref(), self.version)
    }
}

impl Tool {
    fn from_package(value: &str) -> Self {
        let package = PackageRequest::parse(value);
        let permissions = first_party_permissions(&package).unwrap_or_default();
        Self {
            name: package.name.clone(),
            version: package.version.clone(),
            source: ToolSource::Package(package),
            sha256: None,
            permissions,
        }
    }

    fn is_scalar_package_entry(&self) -> bool {
        let ToolSource::Package(package) = &self.source else {
            return false;
        };
        self.name == package.name
            && self.version == package.version
            && self.sha256.is_none()
            && first_party_permissions(package)
                .is_some_and(|permissions| self.permissions == permissions)
    }
}

impl Serialize for Tool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.is_scalar_package_entry() {
            return serializer.serialize_str(self.source.to_wire_string().as_ref());
        }

        let mut state = serializer.serialize_struct("Tool", 5)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("source", &self.source)?;
        if let Some(sha256) = &self.sha256 {
            state.serialize_field("sha256", sha256)?;
        }
        if self.permissions != ToolPermissions::default() {
            state.serialize_field("permissions", &self.permissions)?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for Tool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ToolObject {
            name: String,
            version: String,
            source: ToolSource,
            #[serde(default)]
            sha256: Option<String>,
            #[serde(default)]
            permissions: ToolPermissions,
        }

        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(package) => Ok(Self::from_package(&package)),
            serde_json::Value::Object(_) => {
                let tool: ToolObject = serde_json::from_value(value).map_err(de::Error::custom)?;
                Ok(Self {
                    name: tool.name,
                    version: tool.version,
                    source: tool.source,
                    sha256: tool.sha256,
                    permissions: tool.permissions,
                })
            }
            other => Err(de::Error::custom(format!(
                "expected a package request string or tool object, got {other}"
            ))),
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

/// Return embedded permissions for first-party scalar package declarations.
#[must_use]
pub fn first_party_permissions(package: &PackageRequest) -> Option<ToolPermissions> {
    if package.namespace != "specify" {
        return None;
    }
    match package.name.as_str() {
        "contract" => Some(ToolPermissions {
            read: vec!["$PROJECT_DIR/contracts".to_string()],
            write: Vec::new(),
        }),
        "vectis" => Some(ToolPermissions {
            read: vec!["$PROJECT_DIR".to_string(), "$CAPABILITY_DIR".to_string()],
            write: vec!["$PROJECT_DIR".to_string()],
        }),
        _ => None,
    }
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

fn looks_like_package_request(value: &str) -> bool {
    value.contains(':') || value.starts_with("specify:")
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

    #[test]
    fn scalar_package_entry_derives_tool_fields_and_permissions() {
        let manifest: ToolManifest =
            serde_saphyr::from_str("tools:\n  - \"specify:contract@1.2.3\"\n")
                .expect("parse package manifest");
        assert_eq!(manifest.tools.len(), 1);
        let tool = &manifest.tools[0];
        assert_eq!(tool.name, "contract");
        assert_eq!(tool.version, "1.2.3");
        assert!(matches!(
            &tool.source,
            ToolSource::Package(package)
                if package.namespace == "specify"
                    && package.name == "contract"
                    && package.version == "1.2.3"
        ));
        assert_eq!(
            tool.permissions,
            ToolPermissions {
                read: vec!["$PROJECT_DIR/contracts".to_string()],
                write: Vec::new(),
            }
        );

        let yaml = serde_saphyr::to_string(&manifest).expect("serialize package manifest");
        assert!(yaml.contains("specify:contract@1.2.3"), "{yaml}");
    }

    #[test]
    fn unknown_package_entry_keeps_empty_permissions_for_validation() {
        let manifest: ToolManifest =
            serde_saphyr::from_str("tools:\n  - \"other:helper@latest\"\n")
                .expect("parse package manifest");
        let tool = &manifest.tools[0];
        assert_eq!(tool.name, "helper");
        assert_eq!(tool.version, "latest");
        assert_eq!(tool.permissions, ToolPermissions::default());
    }
}
