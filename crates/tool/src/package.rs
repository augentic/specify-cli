//! wasm-pkg package resolution for declared tools.

use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use serde::Deserialize;
use sha2::Digest;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;
use wasm_pkg_client::metadata::RegistryMetadataExt;
use wasm_pkg_client::{
    Client, Config, PackageRef, Registry, RegistryMapping, RegistryMetadata, Version,
};

use crate::error::ToolError;
use crate::manifest::PackageRequest;

const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
const FIRST_PARTY_NAMESPACE: &str = "specify";
const FIRST_PARTY_REGISTRY: &str = "augentic.io";
/// Filename of the project-local wasm-pkg config inside `.specify/`.
///
/// Paired with [`Layout::specify_dir`](specify-domain) at the init
/// site so the helper does not have to re-derive the relative path.
pub const WASM_PKG_CONFIG_FILENAME: &str = "wasm-pkg.toml";

/// Project-rooted relative path to the project-local wasm-pkg config.
///
/// Merged in between the global wasm-pkg defaults and the `WKG_CONFIG`
/// override. Operators edit this file to add namespace mappings
/// (private mirrors, internal registries) without setting an env var.
pub const WASM_PKG_CONFIG_PATH: &str = ".specify/wasm-pkg.toml";

/// Canonical contents `specify init` writes for a fresh project.
///
/// Mirrors RFC-17 §"Distribution Model" so
/// `wkg --config .specify/wasm-pkg.toml` and `specify tool fetch`
/// agree on namespace routing.
pub const DEFAULT_WASM_PKG_CONFIG: &str = "default_registry = \"augentic.io\"\n\
                                           \n\
                                           [namespace_registries]\n\
                                           specify = \"augentic.io\"\n";

/// Informational package metadata recorded in `meta.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageMetadata {
    /// Package name without the version suffix.
    pub name: String,
    /// Exact package version.
    pub version: String,
    /// Registry host used for resolution.
    pub registry: String,
    /// Best-effort OCI reference derived from the resolved registry's
    /// well-known wasm-pkg metadata. `None` when metadata is absent or
    /// the registry uses a non-OCI protocol.
    pub oci_reference: Option<String>,
}

/// Bytes acquired from a tool source, ready for digest validation and
/// installation into the cache. Every source streams to a sibling
/// `NamedTempFile` so the install step is a uniform `persist` regardless of
/// whether the bytes came from a local file, an HTTPS download, or a
/// package registry.
#[derive(Debug)]
pub struct AcquiredBytes {
    pub temp: NamedTempFile,
    pub sha256: String,
    pub package_metadata: Option<PackageMetadata>,
}

impl AcquiredBytes {
    pub fn len(&self) -> Result<u64, ToolError> {
        self.temp
            .as_file()
            .metadata()
            .map(|m| m.len())
            .map_err(|err| ToolError::cache_io("stat staged tool body", self.temp.path(), err))
    }
}

/// Atomically move `temp` over `dest`, mapping the typed `tempfile`
/// persist error into the cache-error vocabulary the resolver speaks.
/// Free function (rather than a method on [`AcquiredBytes`]) so the
/// resolver can destructure the bytes once and move `temp` and
/// `package_metadata` independently without forcing a clone.
pub fn persist_temp(temp: NamedTempFile, dest: &Path) -> Result<(), ToolError> {
    temp.persist(dest).map(|_| ()).map_err(|err| {
        ToolError::atomic_move_failed(err.file.path().to_path_buf(), dest.to_path_buf(), err.error)
    })
}

/// Pulls wasm-pkg package bytes for a package request.
pub trait PackageClient {
    /// Fetch package content into a sibling tempfile below `dest_hint`.
    ///
    /// # Errors
    ///
    /// Returns package resolution, registry, stream, or cache staging errors.
    fn fetch(&self, request: &PackageRequest, dest_hint: &Path)
    -> Result<AcquiredBytes, ToolError>;
}

/// Default wasm-pkg package client.
///
/// Constructed by [`crate::resolver::resolve`] with the active project
/// root so [`load_config`] can merge the project-local
/// `.specify/wasm-pkg.toml` (when present) into the wasm-pkg config
/// chain. Tests inject their own [`PackageClient`] via
/// `resolver::resolve_with` and bypass this entirely.
#[derive(Debug, Default, Clone)]
pub struct WasmPkgClient {
    project_dir: Option<PathBuf>,
}

impl WasmPkgClient {
    /// Build a client anchored at `project_dir`. Pass `None` when no
    /// project context is available (e.g. ad-hoc invocations); the
    /// project-local config layer is then skipped.
    #[must_use]
    pub const fn new(project_dir: Option<PathBuf>) -> Self {
        Self { project_dir }
    }
}

impl PackageClient for WasmPkgClient {
    fn fetch(
        &self, request: &PackageRequest, dest_hint: &Path,
    ) -> Result<AcquiredBytes, ToolError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| ToolError::package(request, format!("create tokio runtime: {err}")))?;
        runtime.block_on(fetch(request, dest_hint, self.project_dir.as_deref()))
    }
}

async fn fetch(
    request: &PackageRequest, dest_hint: &Path, project_dir: Option<&Path>,
) -> Result<AcquiredBytes, ToolError> {
    let temp_parent = dest_hint.parent().ok_or_else(|| {
        ToolError::cache_root(format!(
            "tool package destination has no parent: {}",
            dest_hint.display()
        ))
    })?;
    std::fs::create_dir_all(temp_parent).map_err(|err| {
        ToolError::cache_io("create package download staging parent", temp_parent, err)
    })?;
    let temp = NamedTempFile::new_in(temp_parent)
        .map_err(|err| ToolError::cache_io("create package download tempfile", temp_parent, err))?;

    let package: PackageRef = request
        .name_ref()
        .parse()
        .map_err(|err| ToolError::package(request, format!("parse package name: {err}")))?;
    let version = Version::parse(&request.version)
        .map_err(|err| ToolError::package(request, format!("parse package version: {err}")))?;
    let config = load_config(&package, project_dir).await?;
    let resolved_registry: Registry =
        config.resolve_registry(&package).cloned().unwrap_or_else(|| {
            FIRST_PARTY_REGISTRY.parse().expect("FIRST_PARTY_REGISTRY parses as a Registry")
        });
    let registry_string = resolved_registry.to_string();
    let oci_reference = derive_oci_reference(
        &resolved_registry,
        &request.namespace,
        &request.name,
        &request.version,
    )
    .await;
    let client = Client::new(config);
    let release = client
        .get_release(&package, &version)
        .await
        .map_err(|err| ToolError::package(request, format!("resolve package release: {err}")))?;
    let mut stream = client.stream_content(&package, &release).await.map_err(|err| {
        ToolError::package(request, format!("open package content stream: {err}"))
    })?;
    let mut file =
        tokio::fs::File::from_std(temp.reopen().map_err(|err| {
            ToolError::cache_io("open package download tempfile", temp.path(), err)
        })?);
    let mut hasher = sha2::Sha256::new();
    let mut total = 0_u64;
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
    file.flush()
        .await
        .map_err(|err| ToolError::cache_io("flush package download tempfile", temp.path(), err))?;
    file.sync_all()
        .await
        .map_err(|err| ToolError::cache_io("sync package download tempfile", temp.path(), err))?;
    drop(file);

    Ok(AcquiredBytes {
        temp,
        sha256: format!("{:x}", hasher.finalize()),
        package_metadata: Some(PackageMetadata {
            name: request.name_ref(),
            version: request.version.clone(),
            registry: registry_string,
            oci_reference,
        }),
    })
}

/// Build the wasm-pkg [`Config`] used to resolve `package`, layering
/// (in order, last write wins per key):
///
/// 1. The wasm-pkg global defaults (`Config::global_defaults`).
/// 2. The project-local `.specify/wasm-pkg.toml` when `project_dir` is
///    `Some` and the file exists.
/// 3. The `WKG_CONFIG` override file when the env var is set.
/// 4. An embedded `specify -> augentic.io` namespace mapping when no
///    explicit `specify` mapping was supplied by any layer above.
async fn load_config(
    package: &PackageRef, project_dir: Option<&Path>,
) -> Result<Config, ToolError> {
    let mut config = Config::global_defaults().await.map_err(|err| {
        ToolError::package_label(package.to_string(), format!("load wasm-pkg config: {err}"))
    })?;

    if let Some(dir) = project_dir {
        let project_config_path = dir.join(WASM_PKG_CONFIG_PATH);
        if project_config_path.is_file() {
            let project_config = Config::from_file(&project_config_path).await.map_err(|err| {
                ToolError::package_label(
                    package.to_string(),
                    format!(
                        "load project wasm-pkg config {}: {err}",
                        project_config_path.display()
                    ),
                )
            })?;
            config.merge(project_config);
        }
    }

    if let Some(path) = std::env::var_os("WKG_CONFIG") {
        let override_config = Config::from_file(&path).await.map_err(|err| {
            ToolError::package_label(
                package.to_string(),
                format!("load WKG_CONFIG {}: {err}", Path::new(&path).display()),
            )
        })?;
        config.merge(override_config);
    }

    if package.namespace().to_string() == FIRST_PARTY_NAMESPACE
        && config.namespace_registry(package.namespace()).is_none()
    {
        let registry: Registry = FIRST_PARTY_REGISTRY.parse().map_err(|err| {
            ToolError::package_label(
                package.to_string(),
                format!("parse first-party registry default: {err}"),
            )
        })?;
        config.set_namespace_registry(
            package.namespace().clone(),
            RegistryMapping::Registry(registry),
        );
    }
    Ok(config)
}

/// Best-effort OCI reference derivation using the resolved registry's
/// well-known wasm-pkg metadata. Mirrors the OCI backend's
/// `make_reference` shape so the recorded reference matches what the
/// loader actually pulled. Network failure (or non-OCI metadata) yields
/// `None` rather than a synthesised guess.
async fn derive_oci_reference(
    registry: &Registry, namespace: &str, name: &str, version: &str,
) -> Option<String> {
    let metadata = RegistryMetadata::fetch_or_default(registry).await;
    oci_reference_from_metadata(&metadata, registry, namespace, name, version)
}

/// Pure metadata-to-reference projection. Split out so tests can
/// exercise the formatting and the metadata-shape contract without
/// hitting the network.
fn oci_reference_from_metadata(
    metadata: &RegistryMetadata, registry: &Registry, namespace: &str, name: &str, version: &str,
) -> Option<String> {
    let oci = metadata.protocol_config::<OciProtocolMetadata>("oci").ok().flatten()?;
    let oci_registry = oci.registry.unwrap_or_else(|| registry.to_string());
    let prefix = oci.namespace_prefix.unwrap_or_default();
    Some(format!("{oci_registry}/{prefix}{namespace}/{name}:{version}"))
}

/// Local mirror of `wasm_pkg_client::oci::OciRegistryMetadata` (which is
/// `pub(crate)` upstream). Fields match the well-known
/// `/.well-known/wasm-pkg/registry.json` `oci` block.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OciProtocolMetadata {
    registry: Option<String>,
    namespace_prefix: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::{EnvGuard, env_lock, scratch_dir};

    fn package_ref() -> PackageRef {
        "specify:contract".parse().expect("parse first-party package ref")
    }

    fn third_party_ref() -> PackageRef {
        "ba:demo".parse().expect("parse third-party package ref")
    }

    /// Point `HOME` and `XDG_CONFIG_HOME` at a fresh scratch dir so
    /// `Config::global_defaults` cannot pull in a developer's personal
    /// `~/.config/wasm-pkg/config.toml` and surprise the assertions.
    fn isolate_global_config_dir(label: &str) -> (PathBuf, [EnvGuard; 2]) {
        let home = scratch_dir(label);
        let guards = [EnvGuard::set("HOME", &home), EnvGuard::set("XDG_CONFIG_HOME", &home)];
        (home, guards)
    }

    #[test]
    fn embedded_default_injects_first_party_namespace() {
        let _guard = env_lock();
        let (_home, _isolated) = isolate_global_config_dir("package-embedded-default");
        let _wkg = EnvGuard::unset("WKG_CONFIG");

        let package = package_ref();
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let config = runtime.block_on(load_config(&package, None)).expect("load_config ok");
        let resolved = config.resolve_registry(&package).expect("specify namespace mapped");
        assert_eq!(resolved.to_string(), FIRST_PARTY_REGISTRY);
    }

    #[test]
    fn embedded_default_skipped_for_other_namespaces() {
        let _guard = env_lock();
        let (_home, _isolated) = isolate_global_config_dir("package-embedded-other");
        let _wkg = EnvGuard::unset("WKG_CONFIG");

        let package = third_party_ref();
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let config = runtime.block_on(load_config(&package, None)).expect("load_config ok");
        // wasm-pkg ships a `ba -> bytecodealliance.org` hard-coded
        // fallback; the contract here is only that we did NOT inject
        // `augentic.io` for a non-`specify` namespace.
        let resolved = config.resolve_registry(&package).map(ToString::to_string);
        assert_ne!(resolved.as_deref(), Some(FIRST_PARTY_REGISTRY));
    }

    #[test]
    fn project_local_config_overrides_embedded_default() {
        let _guard = env_lock();
        let (_home, _isolated) = isolate_global_config_dir("package-project-config-home");
        let _wkg = EnvGuard::unset("WKG_CONFIG");

        let project = scratch_dir("package-project-config");
        fs::create_dir_all(project.join(".specify")).expect("create .specify");
        fs::write(
            project.join(WASM_PKG_CONFIG_PATH),
            "[namespace_registries]\nspecify = \"mirror.example.com\"\n",
        )
        .expect("write project wasm-pkg.toml");

        let package = package_ref();
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let config = runtime
            .block_on(load_config(&package, Some(project.as_path())))
            .expect("load_config ok");
        let resolved = config.resolve_registry(&package).expect("namespace mapped");
        assert_eq!(resolved.to_string(), "mirror.example.com");
    }

    #[test]
    fn wkg_config_overrides_project_local() {
        let _guard = env_lock();
        let (_home, _isolated) = isolate_global_config_dir("package-wkg-overrides-home");

        let project = scratch_dir("package-wkg-overrides-project");
        fs::create_dir_all(project.join(".specify")).expect("create .specify");
        fs::write(
            project.join(WASM_PKG_CONFIG_PATH),
            "[namespace_registries]\nspecify = \"mirror.example.com\"\n",
        )
        .expect("write project wasm-pkg.toml");

        let wkg_config = project.join("wkg-override.toml");
        fs::write(&wkg_config, "[namespace_registries]\nspecify = \"override.example.com\"\n")
            .expect("write wkg override");
        let _wkg = EnvGuard::set("WKG_CONFIG", &wkg_config);

        let package = package_ref();
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let config = runtime
            .block_on(load_config(&package, Some(project.as_path())))
            .expect("load_config ok");
        let resolved = config.resolve_registry(&package).expect("namespace mapped");
        assert_eq!(resolved.to_string(), "override.example.com");
    }

    #[test]
    fn missing_project_config_is_silently_skipped() {
        let _guard = env_lock();
        let (_home, _isolated) = isolate_global_config_dir("package-missing-project-home");
        let _wkg = EnvGuard::unset("WKG_CONFIG");

        let project = scratch_dir("package-missing-project-config");
        // Intentionally do not create `.specify/wasm-pkg.toml`.

        let package = package_ref();
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let config = runtime
            .block_on(load_config(&package, Some(project.as_path())))
            .expect("load_config ok");
        let resolved = config.resolve_registry(&package).expect("specify namespace mapped");
        assert_eq!(resolved.to_string(), FIRST_PARTY_REGISTRY);
    }

    #[test]
    fn oci_reference_derived_from_metadata() {
        let raw = serde_json::json!({
            "preferredProtocol": "oci",
            "oci": {
                "registry": "ghcr.io",
                "namespacePrefix": "augentic/"
            }
        });
        let metadata: RegistryMetadata = serde_json::from_value(raw).expect("deserialize metadata");
        let registry: Registry = "augentic.io".parse().expect("parse registry");
        let reference =
            oci_reference_from_metadata(&metadata, &registry, "specify", "contract", "1.0.0")
                .expect("oci reference derived");
        assert_eq!(reference, "ghcr.io/augentic/specify/contract:1.0.0");
    }

    #[test]
    fn metadata_without_oci_yields_none() {
        let metadata = RegistryMetadata::default();
        let registry: Registry = "augentic.io".parse().expect("parse registry");
        assert!(
            oci_reference_from_metadata(&metadata, &registry, "specify", "contract", "1.0.0")
                .is_none(),
            "default metadata must produce no OCI reference"
        );
    }

    #[test]
    fn metadata_without_namespace_prefix_omits_prefix() {
        let raw = serde_json::json!({
            "preferredProtocol": "oci",
            "oci": {
                "registry": "ghcr.io"
            }
        });
        let metadata: RegistryMetadata = serde_json::from_value(raw).expect("deserialize metadata");
        let registry: Registry = "augentic.io".parse().expect("parse registry");
        let reference =
            oci_reference_from_metadata(&metadata, &registry, "specify", "contract", "1.0.0")
                .expect("oci reference derived");
        assert_eq!(reference, "ghcr.io/specify/contract:1.0.0");
    }

    #[test]
    fn metadata_without_oci_registry_falls_back_to_resolved_registry() {
        let raw = serde_json::json!({
            "preferredProtocol": "oci",
            "oci": {
                "namespacePrefix": "augentic/"
            }
        });
        let metadata: RegistryMetadata = serde_json::from_value(raw).expect("deserialize metadata");
        let registry: Registry = "augentic.io".parse().expect("parse registry");
        let reference =
            oci_reference_from_metadata(&metadata, &registry, "specify", "contract", "1.0.0")
                .expect("oci reference derived");
        assert_eq!(reference, "augentic.io/augentic/specify/contract:1.0.0");
    }
}
