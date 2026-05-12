//! wasm-pkg package resolution for declared tools. `PackageClient`
//! compiles unconditionally; the `wasm-pkg-client` backing for
//! [`WasmPkgClient`] is gated behind the `oci` Cargo feature.

use std::path::Path;

use tempfile::NamedTempFile;

use crate::error::ToolError;
use crate::manifest::PackageRequest;

/// Informational package metadata recorded in `meta.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageMetadata {
    /// Package name without the version suffix.
    pub name: String,
    /// Exact package version.
    pub version: String,
    /// Registry host used for resolution.
    pub registry: String,
    /// Best-effort OCI reference for first-party package defaults.
    pub oci_reference: Option<String>,
}

/// Package bytes staged into a temporary file.
#[derive(Debug)]
pub struct FetchedPackage {
    /// Temporary file containing the downloaded package component bytes.
    pub temp: NamedTempFile,
    /// SHA-256 digest computed while streaming.
    pub sha256: String,
    /// Resolution metadata for the sidecar.
    pub metadata: PackageMetadata,
}

/// Pulls wasm-pkg package bytes for a package request.
pub trait PackageClient {
    /// Fetch package content into a sibling tempfile below `dest_hint`.
    ///
    /// # Errors
    ///
    /// Returns package resolution, registry, stream, or cache staging errors.
    /// Without the `oci` Cargo feature, returns [`ToolError::PackageDisabled`].
    fn fetch(
        &self, request: &PackageRequest, dest_hint: &Path,
    ) -> Result<FetchedPackage, ToolError>;
}

/// Default package client.
///
/// With the `oci` Cargo feature, this is backed by `wasm-pkg-client` and a
/// per-call current-thread Tokio runtime. Without the feature, the type still
/// exists so [`crate::resolver`] dispatch compiles, but [`Self::fetch`]
/// returns [`ToolError::PackageDisabled`].
#[derive(Debug, Default)]
pub struct WasmPkgClient;

#[cfg(not(feature = "oci"))]
impl PackageClient for WasmPkgClient {
    fn fetch(
        &self, _request: &PackageRequest, _dest_hint: &Path,
    ) -> Result<FetchedPackage, ToolError> {
        Err(ToolError::PackageDisabled)
    }
}

#[cfg(feature = "oci")]
mod oci_backend {
    use std::path::Path;

    use futures_util::TryStreamExt;
    use sha2::Digest;
    use tempfile::NamedTempFile;
    use tokio::io::AsyncWriteExt;
    use wasm_pkg_client::{Client, Config, PackageRef, Registry, RegistryMapping, Version};

    use super::{FetchedPackage, PackageClient, PackageMetadata, WasmPkgClient};
    use crate::error::ToolError;
    use crate::manifest::PackageRequest;

    const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
    const FIRST_PARTY_REGISTRY: &str = "augentic.io";
    const FIRST_PARTY_OCI_PREFIX: &str = "ghcr.io/augentic/specify";

    impl PackageClient for WasmPkgClient {
        fn fetch(
            &self, request: &PackageRequest, dest_hint: &Path,
        ) -> Result<FetchedPackage, ToolError> {
            let runtime =
                tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(
                    |err| ToolError::package(request, format!("create tokio runtime: {err}")),
                )?;
            runtime.block_on(fetch(request, dest_hint))
        }
    }

    async fn fetch(
        request: &PackageRequest, dest_hint: &Path,
    ) -> Result<FetchedPackage, ToolError> {
        let temp_parent = dest_hint.parent().ok_or_else(|| {
            ToolError::CacheRoot(format!(
                "tool package destination has no parent: {}",
                dest_hint.display()
            ))
        })?;
        std::fs::create_dir_all(temp_parent).map_err(|err| {
            ToolError::cache_io("create package download staging parent", temp_parent, err)
        })?;
        let temp = NamedTempFile::new_in(temp_parent).map_err(|err| {
            ToolError::cache_io("create package download tempfile", temp_parent, err)
        })?;

        let package: PackageRef = request
            .name_ref()
            .parse()
            .map_err(|err| ToolError::package(request, format!("parse package name: {err}")))?;
        let version = Version::parse(&request.version)
            .map_err(|err| ToolError::package(request, format!("parse package version: {err}")))?;
        let config = config_for(&package).await?;
        let registry = config
            .resolve_registry(&package)
            .map_or_else(|| FIRST_PARTY_REGISTRY.to_string(), ToString::to_string);
        let client = Client::new(config);
        let release = client.get_release(&package, &version).await.map_err(|err| {
            ToolError::package(request, format!("resolve package release: {err}"))
        })?;
        let mut stream = client.stream_content(&package, &release).await.map_err(|err| {
            ToolError::package(request, format!("open package content stream: {err}"))
        })?;
        let mut file = tokio::fs::File::from_std(temp.reopen().map_err(|err| {
            ToolError::cache_io("open package download tempfile", temp.path(), err)
        })?);
        let mut hasher = sha2::Sha256::new();
        let mut total = 0u64;
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|err| ToolError::package(request, format!("stream package content: {err}")))?
        {
            total = total.saturating_add(chunk.len() as u64);
            if total > MAX_PACKAGE_BYTES {
                return Err(ToolError::Network {
                    url: request.to_wire_string(),
                    kind: crate::error::NetworkKind::TooLarge {
                        limit: MAX_PACKAGE_BYTES,
                        actual: Some(total),
                    },
                });
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|err| {
                ToolError::cache_io("write package download tempfile", temp.path(), err)
            })?;
        }
        file.flush().await.map_err(|err| {
            ToolError::cache_io("flush package download tempfile", temp.path(), err)
        })?;
        file.sync_all().await.map_err(|err| {
            ToolError::cache_io("sync package download tempfile", temp.path(), err)
        })?;
        drop(file);

        Ok(FetchedPackage {
            temp,
            sha256: format!("{:x}", hasher.finalize()),
            metadata: PackageMetadata {
                name: request.name_ref(),
                version: request.version.clone(),
                registry: registry.clone(),
                oci_reference: first_party_oci_reference(request, &registry),
            },
        })
    }

    async fn config_for(package: &PackageRef) -> Result<Config, ToolError> {
        let mut config = Config::global_defaults().await.map_err(|err| ToolError::Package {
            source_value: package.to_string(),
            reason: format!("load wasm-pkg config: {err}"),
        })?;
        if let Some(path) = std::env::var_os("WKG_CONFIG") {
            let override_config =
                Config::from_file(&path).await.map_err(|err| ToolError::Package {
                    source_value: package.to_string(),
                    reason: format!("load WKG_CONFIG {}: {err}", Path::new(&path).display()),
                })?;
            config.merge(override_config);
        }
        if package.namespace().to_string() == "specify"
            && config.namespace_registry(package.namespace()).is_none()
        {
            let registry: Registry =
                FIRST_PARTY_REGISTRY.parse().map_err(|err| ToolError::Package {
                    source_value: package.to_string(),
                    reason: format!("parse first-party registry default: {err}"),
                })?;
            config.set_namespace_registry(
                package.namespace().clone(),
                RegistryMapping::Registry(registry),
            );
        }
        Ok(config)
    }

    fn first_party_oci_reference(request: &PackageRequest, registry: &str) -> Option<String> {
        (request.namespace == "specify" && registry == FIRST_PARTY_REGISTRY)
            .then(|| format!("{FIRST_PARTY_OCI_PREFIX}/{}:{}", request.name, request.version))
    }
}
