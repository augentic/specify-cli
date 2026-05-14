//! Source resolution for local paths, `file:` URIs, `https:` URIs, and
//! wasm-pkg package requests.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::{Builder, NamedTempFile};

pub mod digest;
pub mod http;
pub mod local;

use crate::cache::{self, MODULE_FILENAME, PermissionsSnapshot, SIDECAR_FILENAME, Sidecar};
use crate::error::ToolError;
use crate::manifest::{Tool, ToolScope, ToolSource};
use crate::package::{FetchedPackage, PackageClient, PackageMetadata, WasmPkgClient};

/// Cached component bytes plus the live declaration data needed to run them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    /// Path to the cached WASI component bytes.
    pub bytes_path: PathBuf,
    /// Declaration site that supplied the tool.
    pub scope: ToolScope,
    /// Live manifest declaration used for argv and permission evaluation.
    pub tool: Tool,
}

/// Resolve a declared tool source into the global immutable cache.
///
/// A cache hit is valid only when the live declaration tuple matches the
/// sidecar and, when `sha256` is pinned, the cached bytes still hash to that
/// digest. Misses and digest refreshes stage `module.wasm` and `meta.yaml`
/// together, then atomically install the complete version directory.
///
/// `now` records the sidecar `fetched_at`; the dispatcher passes
/// `Timestamp::now`, tests pin a deterministic stamp.
///
/// `project_dir` is used to discover the project-local
/// `.specify/wasm-pkg.toml` (when present) so package fetches inherit
/// the project's namespace overrides without an env var.
///
/// # Errors
///
/// Returns cache errors, source read errors, digest mismatches, or typed network
/// resolver errors.
pub fn resolve(
    scope: &ToolScope, tool: &Tool, now: jiff::Timestamp, project_dir: &Path,
) -> Result<ResolvedTool, ToolError> {
    resolve_with(
        scope,
        tool,
        now,
        &WasmPkgClient::new(Some(project_dir.to_path_buf())),
    )
}

/// Resolve a declared tool using an injected package client.
///
/// Tests use this to cover package resolver behavior without depending on a
/// live registry.
///
/// # Errors
///
/// Returns the same cache, source, digest, and resolver errors as [`resolve`].
pub(crate) fn resolve_with(
    scope: &ToolScope, tool: &Tool, now: jiff::Timestamp, package_client: &impl PackageClient,
) -> Result<ResolvedTool, ToolError> {
    let source = tool.source.to_wire_string().into_owned();
    let module = cache::module_path(scope, &tool.name, &tool.version)?;
    if cache::status(scope, &tool.name, &tool.version, &source, tool.sha256.as_deref())?
        == cache::Status::Hit
        && digest::cached_matches(&module, tool.sha256.as_deref())?
    {
        return Ok(resolved(scope, tool, module));
    }

    let dest = cache::tool_dir(scope, &tool.name, &tool.version)?;
    let Some(parent) = dest.parent() else {
        return Err(ToolError::cache_root(format!(
            "tool cache destination has no parent: {}",
            dest.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|err| ToolError::cache_io("create resolver staging parent", parent, err))?;
    let stem = dest.file_name().unwrap_or_else(|| OsStr::new("tool")).to_string_lossy();
    let prefix = format!(".resolver-{stem}.");
    let staged = Builder::new()
        .prefix(&prefix)
        .suffix(".tmp")
        .tempdir_in(parent)
        .map_err(|err| ToolError::cache_io("create resolver staging directory", parent, err))?
        .keep();
    let install_result =
        stage_and_install(scope, tool, &source, &staged, &dest, now, package_client);
    // The atomic install moves `staged/` into `dest/`, so its absence on
    // success is expected. On failure we tear down the staging tree.
    let cleanup_result = if install_result.is_ok() && !staged.exists() {
        Ok(())
    } else {
        fs::remove_dir_all(&staged)
    };
    match (install_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(resolved(scope, tool, dest.join(MODULE_FILENAME))),
        (Ok(()), Err(err)) => {
            Err(ToolError::cache_io("remove resolver staging directory", staged, err))
        }
        (Err(err), _) => Err(err),
    }
}

fn stage_and_install(
    scope: &ToolScope, tool: &Tool, source: &str, staged: &Path, dest: &Path, now: jiff::Timestamp,
    package_client: &impl PackageClient,
) -> Result<(), ToolError> {
    let module_dest = staged.join(MODULE_FILENAME);
    let acquired = acquire_source_bytes(&tool.source, &module_dest, package_client)?;
    digest::validate(source, &acquired, tool.sha256.as_deref())?;
    let package_metadata = acquired.package_metadata();
    acquired.persist_to(&module_dest)?;
    let sidecar = Sidecar::new(
        scope,
        &tool.name,
        &tool.version,
        source,
        PermissionsSnapshot::from(&tool.permissions),
        tool.sha256.clone(),
        package_metadata,
        now,
    )?;
    cache::write_sidecar(&staged.join(SIDECAR_FILENAME), &sidecar)?;
    cache::stage_and_install(staged, dest)
}

fn resolved(scope: &ToolScope, tool: &Tool, bytes_path: PathBuf) -> ResolvedTool {
    ResolvedTool {
        bytes_path,
        scope: scope.clone(),
        tool: tool.clone(),
    }
}

fn acquire_source_bytes(
    source: &ToolSource, dest_hint: &Path, package_client: &impl PackageClient,
) -> Result<AcquiredBytes, ToolError> {
    match source {
        ToolSource::LocalPath(path) => buffered_into_acquired(
            &local::read_local_path(path, &path.to_string_lossy())?,
            dest_hint,
        ),
        ToolSource::FileUri(uri) => buffered_into_acquired(&local::read_file_uri(uri)?, dest_hint),
        ToolSource::HttpsUri(url) => http::download_https(url, dest_hint),
        ToolSource::Package(package) => package_client.fetch(package, dest_hint).map(
            |FetchedPackage {
                 temp,
                 sha256,
                 metadata,
             }| AcquiredBytes {
                temp,
                sha256,
                package_metadata: Some(metadata),
            },
        ),
    }
}

fn buffered_into_acquired(bytes: &[u8], dest_hint: &Path) -> Result<AcquiredBytes, ToolError> {
    let parent = dest_hint.parent().ok_or_else(|| {
        ToolError::cache_root(format!(
            "tool staging destination has no parent: {}",
            dest_hint.display()
        ))
    })?;
    fs::create_dir_all(parent)
        .map_err(|err| ToolError::cache_io("create local staging parent", parent, err))?;
    let temp = NamedTempFile::new_in(parent)
        .map_err(|err| ToolError::cache_io("create local staging tempfile", parent, err))?;
    let sha256 = digest::sha256_hex(bytes);
    fs::write(temp.path(), bytes)
        .map_err(|err| ToolError::cache_io("write local staging tempfile", temp.path(), err))?;
    Ok(AcquiredBytes {
        temp,
        sha256,
        package_metadata: None,
    })
}

/// Bytes acquired from a tool source, ready for digest validation and
/// installation into the cache. Every source streams to a sibling
/// `NamedTempFile` so the install step is a uniform `persist` regardless of
/// whether the bytes came from a local file, an HTTPS download, or a
/// package registry.
#[derive(Debug)]
pub(crate) struct AcquiredBytes {
    pub(crate) temp: NamedTempFile,
    pub(crate) sha256: String,
    pub(crate) package_metadata: Option<PackageMetadata>,
}

impl AcquiredBytes {
    pub(crate) fn len(&self) -> Result<u64, ToolError> {
        self.temp
            .as_file()
            .metadata()
            .map(|m| m.len())
            .map_err(|err| ToolError::cache_io("stat staged tool body", self.temp.path(), err))
    }

    pub(crate) fn sha256_hex(&self) -> String {
        self.sha256.clone()
    }

    pub(crate) fn package_metadata(&self) -> Option<PackageMetadata> {
        self.package_metadata.clone()
    }

    pub(crate) fn persist_to(self, dest: &Path) -> Result<(), ToolError> {
        self.temp.persist(dest).map(|_| ()).map_err(|err| {
            ToolError::atomic_move_failed(
                err.file.path().to_path_buf(),
                dest.to_path_buf(),
                err.error,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::io::Write as _;
    use std::path::Path;

    use super::*;
    use crate::manifest::{PackageRequest, ToolSource};
    use crate::package::{FetchedPackage, PackageClient, PackageMetadata};
    use crate::test_support::{
        cached_bytes, capability_scope, fixed_now, project_scope, scratch_dir, tool,
        with_cache_env, write_source,
    };

    struct MockPackageClient {
        bytes: &'static [u8],
        calls: Cell<u32>,
    }

    impl MockPackageClient {
        fn new(bytes: &'static [u8]) -> Self {
            Self {
                bytes,
                calls: Cell::new(0),
            }
        }
    }

    impl PackageClient for MockPackageClient {
        fn fetch(
            &self, request: &PackageRequest, dest_hint: &Path,
        ) -> Result<FetchedPackage, ToolError> {
            self.calls.set(self.calls.get() + 1);
            let parent = dest_hint.parent().expect("dest hint has parent");
            fs::create_dir_all(parent).expect("create mock package temp parent");
            let mut temp = NamedTempFile::new_in(parent).expect("create mock package tempfile");
            temp.write_all(self.bytes).expect("write mock package bytes");
            Ok(FetchedPackage {
                temp,
                sha256: digest::sha256_hex(self.bytes),
                metadata: PackageMetadata {
                    name: request.name_ref(),
                    version: request.version.clone(),
                    registry: "augentic.io".to_string(),
                    oci_reference: Some(format!(
                        "ghcr.io/augentic/specify/{}:{}",
                        request.name, request.version
                    )),
                },
            })
        }
    }

    #[test]
    fn local_path_cache_miss_hit_and_source_change() {
        let cache_dir = scratch_dir("resolver-local-cache");
        let project_dir = scratch_dir("resolver-local-project");
        let source_dir = scratch_dir("resolver-local-source");
        let first = write_source(&source_dir, "first.wasm", b"first");
        let second = write_source(&source_dir, "second.wasm", b"second");
        let scope = project_scope();
        let first_tool = tool(ToolSource::LocalPath(first.clone()), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved =
                resolve(&scope, &first_tool, fixed_now(), &project_dir).expect("cache miss resolves");
            assert_eq!(fs::read(&resolved.bytes_path).expect("cached bytes"), b"first");

            fs::write(&first, b"changed-at-source").expect("mutate source");
            let hit = resolve(&scope, &first_tool, fixed_now(), &project_dir)
                .expect("cache hit resolves");
            assert_eq!(hit.bytes_path, resolved.bytes_path);
            assert_eq!(cached_bytes(&scope, &first_tool), b"first");

            let changed_tool = tool(ToolSource::LocalPath(second), None);
            let changed = resolve(&scope, &changed_tool, fixed_now(), &project_dir)
                .expect("changed source re-stages");
            assert_eq!(changed.bytes_path, resolved.bytes_path);
            assert_eq!(cached_bytes(&scope, &changed_tool), b"second");
        });
    }

    #[test]
    fn project_and_capability_scopes_have_isolated_cache_dirs() {
        let cache_dir = scratch_dir("resolver-scope-cache");
        let project_dir = scratch_dir("resolver-scope-project");
        let source_dir = scratch_dir("resolver-scope-source");
        let capability_dir = scratch_dir("resolver-capability");
        let source = write_source(&source_dir, "module.wasm", b"same");
        let project = project_scope();
        let capability = capability_scope(&capability_dir);
        let declared = tool(ToolSource::LocalPath(source), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let project_resolved =
                resolve(&project, &declared, fixed_now(), &project_dir).expect("project resolve");
            let capability_resolved = resolve(&capability, &declared, fixed_now(), &project_dir)
                .expect("capability resolve");
            assert_ne!(project_resolved.bytes_path, capability_resolved.bytes_path);
            assert!(project_resolved.bytes_path.to_string_lossy().contains("project--demo"));
            assert!(
                capability_resolved.bytes_path.to_string_lossy().contains("capability--contracts")
            );
        });
    }

    #[test]
    fn package_source_uses_injected_client_and_records_metadata() {
        let cache_dir = scratch_dir("resolver-package-cache");
        let scope = project_scope();
        let package = PackageRequest {
            namespace: "specify".to_string(),
            name: "contract".to_string(),
            version: "1.0.0".to_string(),
        };
        let declared = tool(ToolSource::Package(package), None);
        let client = MockPackageClient::new(b"package-bytes");

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved =
                resolve_with(&scope, &declared, fixed_now(), &client).expect("package resolves");
            assert_eq!(fs::read(resolved.bytes_path).expect("cached bytes"), b"package-bytes");
            assert_eq!(client.calls.get(), 1);

            let sidecar = cache::read_sidecar(
                &cache::sidecar_path(&scope, &declared.name, &declared.version)
                    .expect("sidecar path"),
            )
            .expect("read sidecar")
            .expect("sidecar exists");
            assert_eq!(sidecar.source, "specify:contract@1.0.0");
            assert_eq!(
                sidecar.package.as_ref().map(|package| package.name.as_str()),
                Some("specify:contract")
            );
            assert_eq!(
                sidecar.oci.as_ref().map(|oci| oci.reference.as_str()),
                Some("ghcr.io/augentic/specify/contract:1.0.0")
            );

            resolve_with(&scope, &declared, fixed_now(), &client)
                .expect("package cache hit resolves");
            assert_eq!(client.calls.get(), 1, "cache hit must not fetch package again");
        });
    }
}
