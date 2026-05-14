//! Manifest data types for declared Specify WASI tools.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One declared WASI tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "ToolForm")]
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
    #[serde(default, skip_serializing_if = "ToolPermissions::is_default")]
    pub permissions: ToolPermissions,
}

/// Supported source locations for WASI component bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
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
        } else if Path::new(value).is_absolute() || looks_like_windows_absolute(value) {
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

impl From<ToolSource> for String {
    fn from(value: ToolSource) -> Self {
        value.to_wire_string().into_owned()
    }
}

impl TryFrom<String> for ToolSource {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse_wire(&value)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ToolForm {
    Scalar(String),
    Object(ToolObject),
}

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

impl From<ToolForm> for Tool {
    fn from(form: ToolForm) -> Self {
        match form {
            ToolForm::Scalar(value) => {
                let package = PackageRequest::parse(&value);
                let permissions = first_party_permissions(&package).unwrap_or_default();
                Self {
                    name: package.name.clone(),
                    version: package.version.clone(),
                    source: ToolSource::Package(package),
                    sha256: None,
                    permissions,
                }
            }
            ToolForm::Object(ToolObject {
                name,
                version,
                source,
                sha256,
                permissions,
            }) => Self {
                name,
                version,
                source,
                sha256,
                permissions,
            },
        }
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

impl ToolPermissions {
    const fn is_default(&self) -> bool {
        self.read.is_empty() && self.write.is_empty()
    }
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, strum::EnumDiscriminants)]
#[strum_discriminants(name(ToolScopeKind))]
#[strum_discriminants(derive(Hash, serde::Serialize, serde::Deserialize, strum::Display))]
#[strum_discriminants(serde(rename_all = "kebab-case"))]
#[strum_discriminants(strum(serialize_all = "kebab-case"))]
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

pub(crate) fn looks_like_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// True when `value` is a 64-character lowercase hexadecimal SHA-256 digest.
pub(crate) fn looks_like_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
        serde_saphyr::from_str::<ToolManifest>(
            "tools:\n  - name: bad\n    version: 1.0.0\n    source: relative.wasm\n",
        )
        .expect_err("relative source must fail");
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
