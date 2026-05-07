//! Source resolution for local paths, `file:` URIs, and `https:` URIs.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use ureq::ResponseExt;

use crate::cache::{
    self, CacheStatus, MODULE_FILENAME, PermissionsSnapshot, SIDECAR_FILENAME, Sidecar,
};
use crate::error::ToolError;
use crate::manifest::{Tool, ToolScope, ToolSource};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TEMP_ATTEMPTS: u8 = 64;
const USER_AGENT: &str =
    concat!("specify-tool/", env!("CARGO_PKG_VERSION"), " (+https://github.com/augentic/specify)");

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
/// # Errors
///
/// Returns cache errors, source read errors, digest mismatches, or typed network
/// resolver errors.
pub fn resolve(scope: &ToolScope, tool: &Tool) -> Result<ResolvedTool, ToolError> {
    let source = tool.source.to_wire_string().into_owned();
    let module = cache::module_path(scope, &tool.name, &tool.version)?;
    if cache::cache_status(scope, &tool.name, &tool.version, &source, tool.sha256.as_deref())?
        == CacheStatus::Hit
        && cached_digest_matches(&module, tool.sha256.as_deref())?
    {
        return Ok(resolved(scope, tool, module));
    }

    let bytes = read_source_bytes(&tool.source)?;
    validate_resolved_bytes(&source, &bytes, tool.sha256.as_deref())?;
    let bytes_path = install_bytes(scope, tool, &source, &bytes)?;
    Ok(resolved(scope, tool, bytes_path))
}

fn resolved(scope: &ToolScope, tool: &Tool, bytes_path: PathBuf) -> ResolvedTool {
    ResolvedTool {
        bytes_path,
        scope: scope.clone(),
        tool: tool.clone(),
    }
}

fn cached_digest_matches(module: &Path, expected: Option<&str>) -> Result<bool, ToolError> {
    let Some(expected) = expected else {
        return Ok(true);
    };
    let bytes = fs::read(module).map_err(|err| {
        ToolError::cache_io("read cached module for sha256 verification", module, err)
    })?;
    Ok(sha256_hex(&bytes) == expected)
}

fn read_source_bytes(source: &ToolSource) -> Result<Vec<u8>, ToolError> {
    match source {
        ToolSource::LocalPath(path) => read_local_path(path, &path.to_string_lossy()),
        ToolSource::FileUri(uri) => read_file_uri(uri),
        ToolSource::HttpsUri(url) => download_https(url),
    }
}

fn read_file_uri(uri: &str) -> Result<Vec<u8>, ToolError> {
    let Some(rest) = uri.strip_prefix("file://") else {
        return Err(ToolError::invalid_source(uri, "file URI sources must start with file://"));
    };
    if rest.is_empty() {
        return Err(ToolError::invalid_source(uri, "file URI source path must not be empty"));
    }
    read_local_path(&PathBuf::from(rest), uri)
}

fn read_local_path(path: &Path, source: &str) -> Result<Vec<u8>, ToolError> {
    if !path.is_absolute() {
        return Err(ToolError::invalid_source(source, "local source path must be absolute"));
    }
    let metadata = fs::metadata(path)
        .map_err(|err| ToolError::source_io("inspect local source", path, err))?;
    if !metadata.is_file() {
        return Err(ToolError::invalid_source(
            source,
            "local source path must resolve to a regular file",
        ));
    }
    fs::read(path).map_err(|err| ToolError::source_io("read local source", path, err))
}

fn download_https(url: &str) -> Result<Vec<u8>, ToolError> {
    if !url.starts_with("https://") {
        return Err(ToolError::invalid_source(
            url,
            "production network sources must use https://; http:// is not supported",
        ));
    }

    let mut response = https_agent().get(url).call().map_err(|err| map_network_error(url, err))?;
    let final_uri = response.get_uri().to_string();
    if !final_uri.starts_with("https://") {
        return Err(ToolError::invalid_source(
            url,
            format!("redirect target must remain https://, got `{final_uri}`"),
        ));
    }

    let status = response.status().as_u16();
    if status != 200 {
        return Err(ToolError::NetworkStatus {
            url: url.to_string(),
            status,
        });
    }

    if let Some(length) = response.body().content_length()
        && length > MAX_RESPONSE_BYTES
    {
        return Err(ToolError::NetworkTooLarge {
            url: url.to_string(),
            limit: MAX_RESPONSE_BYTES,
            actual: Some(length),
        });
    }

    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES + 1)
        .read_to_vec()
        .map_err(|err| map_network_body_error(url, err))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(ToolError::NetworkTooLarge {
            url: url.to_string(),
            limit: MAX_RESPONSE_BYTES,
            actual: Some(bytes.len() as u64),
        });
    }
    Ok(bytes)
}

fn https_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .https_only(true)
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

fn map_network_error(url: &str, err: ureq::Error) -> ToolError {
    match err {
        ureq::Error::StatusCode(status) => ToolError::NetworkStatus {
            url: url.to_string(),
            status,
        },
        ureq::Error::Timeout(timeout) => ToolError::NetworkTimeout {
            url: url.to_string(),
            detail: timeout.to_string(),
        },
        ureq::Error::BadUri(detail) => ToolError::NetworkMalformed {
            url: url.to_string(),
            detail,
        },
        ureq::Error::Http(err) => ToolError::NetworkMalformed {
            url: url.to_string(),
            detail: err.to_string(),
        },
        ureq::Error::BodyExceedsLimit(limit) => ToolError::NetworkTooLarge {
            url: url.to_string(),
            limit: MAX_RESPONSE_BYTES,
            actual: Some(limit),
        },
        ureq::Error::RequireHttpsOnly(detail) => ToolError::invalid_source(url, detail),
        err => ToolError::Network {
            url: url.to_string(),
            detail: err.to_string(),
        },
    }
}

fn map_network_body_error(url: &str, err: ureq::Error) -> ToolError {
    match err {
        ureq::Error::Timeout(timeout) => ToolError::NetworkTimeout {
            url: url.to_string(),
            detail: timeout.to_string(),
        },
        ureq::Error::BodyExceedsLimit(limit) => ToolError::NetworkTooLarge {
            url: url.to_string(),
            limit: MAX_RESPONSE_BYTES,
            actual: Some(limit),
        },
        err => ToolError::Network {
            url: url.to_string(),
            detail: err.to_string(),
        },
    }
}

fn validate_resolved_bytes(
    source: &str, bytes: &[u8], expected_sha256: Option<&str>,
) -> Result<(), ToolError> {
    if bytes.is_empty() {
        return Err(ToolError::EmptySource {
            source_value: source.to_string(),
        });
    }

    if let Some(expected) = expected_sha256 {
        let actual = sha256_hex(bytes);
        if actual != expected {
            return Err(ToolError::DigestMismatch {
                source_value: source.to_string(),
                expected: expected.to_string(),
                actual,
            });
        }
    }
    Ok(())
}

fn install_bytes(
    scope: &ToolScope, tool: &Tool, source: &str, bytes: &[u8],
) -> Result<PathBuf, ToolError> {
    let dest = cache::tool_dir(scope, &tool.name, &tool.version)?;
    let staged = unique_staging_dir(&dest)?;
    fs::write(staged.join(MODULE_FILENAME), bytes).map_err(|err| {
        ToolError::cache_io("write staged module", staged.join(MODULE_FILENAME), err)
    })?;
    let sidecar = Sidecar::new(
        scope,
        &tool.name,
        &tool.version,
        source,
        PermissionsSnapshot::from(&tool.permissions),
        tool.sha256.clone(),
    )?;
    cache::write_sidecar(&staged.join(SIDECAR_FILENAME), &sidecar)?;

    let install_result = cache::stage_and_install(&staged, &dest);
    let cleanup_result = fs::remove_dir_all(&staged);
    match (install_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(dest.join(MODULE_FILENAME)),
        (Ok(()), Err(err)) => {
            Err(ToolError::cache_io("remove resolver staging directory", staged, err))
        }
        (Err(err), _) => {
            let _ = fs::remove_dir_all(&staged);
            Err(err)
        }
    }
}

fn unique_staging_dir(dest: &Path) -> Result<PathBuf, ToolError> {
    let Some(parent) = dest.parent() else {
        return Err(ToolError::CacheRoot(format!(
            "tool cache destination has no parent: {}",
            dest.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|err| ToolError::cache_io("create resolver staging parent", parent, err))?;

    let stem = dest.file_name().unwrap_or_else(|| OsStr::new("tool")).to_string_lossy();
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate =
            parent.join(format!(".resolver-{stem}.{}.{}.{}.tmp", std::process::id(), nanos, n));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(ToolError::cache_io(
                    "create resolver staging directory",
                    candidate,
                    err,
                ));
            }
        }
    }
    Err(ToolError::CacheCollision {
        parent: parent.to_path_buf(),
        stem: stem.into_owned(),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::TcpListener;

    use super::*;
    use crate::manifest::{ToolPermissions, ToolSource};
    use crate::test_support::{scratch_dir, with_cache_env};

    fn project_scope() -> ToolScope {
        ToolScope::Project {
            project_name: "demo".to_string(),
        }
    }

    fn capability_scope(root: &Path) -> ToolScope {
        ToolScope::Capability {
            capability_slug: "contracts".to_string(),
            capability_dir: root.to_path_buf(),
        }
    }

    fn tool(source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: "contract".to_string(),
            version: "1.0.0".to_string(),
            source,
            sha256,
            permissions: ToolPermissions::default(),
        }
    }

    fn named_tool(name: &str, source: ToolSource, sha256: Option<String>) -> Tool {
        Tool {
            name: name.to_string(),
            ..tool(source, sha256)
        }
    }

    fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, bytes).expect("write source");
        path
    }

    fn cached_bytes(scope: &ToolScope, tool: &Tool) -> Vec<u8> {
        fs::read(cache::module_path(scope, &tool.name, &tool.version).expect("module path"))
            .expect("read cached module")
    }

    #[test]
    fn local_path_cache_miss_hit_and_source_change() {
        let cache_dir = scratch_dir("resolver-local-cache");
        let source_dir = scratch_dir("resolver-local-source");
        let first = write_source(&source_dir, "first.wasm", b"first");
        let second = write_source(&source_dir, "second.wasm", b"second");
        let scope = project_scope();
        let first_tool = tool(ToolSource::LocalPath(first.clone()), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved = resolve(&scope, &first_tool).expect("cache miss resolves");
            assert_eq!(fs::read(&resolved.bytes_path).expect("cached bytes"), b"first");

            fs::write(&first, b"changed-at-source").expect("mutate source");
            let hit = resolve(&scope, &first_tool).expect("cache hit resolves");
            assert_eq!(hit.bytes_path, resolved.bytes_path);
            assert_eq!(cached_bytes(&scope, &first_tool), b"first");

            let changed_tool = tool(ToolSource::LocalPath(second), None);
            let changed = resolve(&scope, &changed_tool).expect("changed source re-stages");
            assert_eq!(changed.bytes_path, resolved.bytes_path);
            assert_eq!(cached_bytes(&scope, &changed_tool), b"second");
        });
    }

    #[test]
    fn digest_mismatch_fails_before_install_and_preserves_previous_cache() {
        let cache_dir = scratch_dir("resolver-digest-cache");
        let source_dir = scratch_dir("resolver-digest-source");
        let old_source = write_source(&source_dir, "old.wasm", b"old-good");
        let new_source = write_source(&source_dir, "new.wasm", b"new-good");
        let old_sha = sha256_hex(b"old-good");
        let wrong_sha =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        let scope = project_scope();
        let old_tool = tool(ToolSource::LocalPath(old_source), Some(old_sha));
        let wrong_tool = tool(ToolSource::LocalPath(new_source.clone()), Some(wrong_sha));

        with_cache_env(Some(&cache_dir), None, None, || {
            resolve(&scope, &old_tool).expect("initial digest install");
            let err = resolve(&scope, &wrong_tool).expect_err("wrong digest must fail");
            assert!(matches!(err, ToolError::DigestMismatch { .. }), "{err}");
            assert_eq!(cached_bytes(&scope, &old_tool), b"old-good");

            let correct_tool =
                tool(ToolSource::LocalPath(new_source), Some(sha256_hex(b"new-good")));
            resolve(&scope, &correct_tool).expect("correct digest updates cache");
            assert_eq!(cached_bytes(&scope, &correct_tool), b"new-good");
        });
    }

    #[test]
    fn cached_bytes_are_rehashed_on_digest_pinned_hit() {
        let cache_dir = scratch_dir("resolver-hit-digest-cache");
        let source_dir = scratch_dir("resolver-hit-digest-source");
        let source = write_source(&source_dir, "module.wasm", b"trusted");
        let scope = project_scope();
        let pinned = tool(ToolSource::LocalPath(source), Some(sha256_hex(b"trusted")));

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved = resolve(&scope, &pinned).expect("install pinned");
            fs::write(&resolved.bytes_path, b"corrupt").expect("corrupt cache");
            let repaired = resolve(&scope, &pinned).expect("digest mismatch re-fetches");
            assert_eq!(repaired.bytes_path, resolved.bytes_path);
            assert_eq!(fs::read(repaired.bytes_path).expect("repaired bytes"), b"trusted");
        });
    }

    #[test]
    fn file_uri_reuses_local_path_resolution() {
        let cache_dir = scratch_dir("resolver-file-cache");
        let source_dir = scratch_dir("resolver-file-source");
        let source = write_source(&source_dir, "module.wasm", b"file-uri");
        let scope = project_scope();
        let local = named_tool("local", ToolSource::LocalPath(source.clone()), None);
        let file_uri = named_tool(
            "file-uri",
            ToolSource::FileUri(format!("file://{}", source.display())),
            None,
        );

        with_cache_env(Some(&cache_dir), None, None, || {
            let local = resolve(&scope, &local).expect("local resolves");
            let uri = resolve(&scope, &file_uri).expect("file URI resolves");
            assert_eq!(fs::read(local.bytes_path).expect("local bytes"), b"file-uri");
            assert_eq!(fs::read(uri.bytes_path).expect("uri bytes"), b"file-uri");
        });
    }

    #[test]
    fn local_path_rejects_non_file_and_empty_file() {
        let cache_dir = scratch_dir("resolver-invalid-local-cache");
        let source_dir = scratch_dir("resolver-invalid-local-source");
        let empty = write_source(&source_dir, "empty.wasm", b"");
        let scope = project_scope();

        with_cache_env(Some(&cache_dir), None, None, || {
            let dir_err = resolve(&scope, &tool(ToolSource::LocalPath(source_dir.clone()), None))
                .expect_err("directory source must fail");
            assert!(matches!(dir_err, ToolError::InvalidSource { .. }), "{dir_err}");

            let empty_err =
                resolve(&scope, &tool(ToolSource::LocalPath(empty), None)).expect_err("empty file");
            assert!(matches!(empty_err, ToolError::EmptySource { .. }), "{empty_err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn local_path_chases_symlinks_to_regular_files() {
        let cache_dir = scratch_dir("resolver-symlink-cache");
        let source_dir = scratch_dir("resolver-symlink-source");
        let target = write_source(&source_dir, "target.wasm", b"symlink-target");
        let link = source_dir.join("link.wasm");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");
        let scope = project_scope();
        let symlink_tool = tool(ToolSource::LocalPath(link), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved = resolve(&scope, &symlink_tool).expect("symlink resolves");
            assert_eq!(fs::read(resolved.bytes_path).expect("cached bytes"), b"symlink-target");
        });
    }

    #[test]
    fn project_and_capability_scopes_have_isolated_cache_dirs() {
        let cache_dir = scratch_dir("resolver-scope-cache");
        let source_dir = scratch_dir("resolver-scope-source");
        let capability_dir = scratch_dir("resolver-capability");
        let source = write_source(&source_dir, "module.wasm", b"same");
        let project = project_scope();
        let capability = capability_scope(&capability_dir);
        let declared = tool(ToolSource::LocalPath(source), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let project_resolved = resolve(&project, &declared).expect("project resolve");
            let capability_resolved = resolve(&capability, &declared).expect("capability resolve");
            assert_ne!(project_resolved.bytes_path, capability_resolved.bytes_path);
            assert!(project_resolved.bytes_path.to_string_lossy().contains("project--demo"));
            assert!(
                capability_resolved.bytes_path.to_string_lossy().contains("capability--contracts")
            );
        });
    }

    #[test]
    fn http_sources_are_rejected_before_network_access() {
        let cache_dir = scratch_dir("resolver-http-cache");
        let scope = project_scope();
        let declared = tool(ToolSource::HttpsUri("http://127.0.0.1/tool.wasm".to_string()), None);

        with_cache_env(Some(&cache_dir), None, None, || {
            let err = resolve(&scope, &declared).expect_err("http must be rejected");
            assert!(matches!(err, ToolError::InvalidSource { .. }), "{err}");
        });
    }

    #[test]
    fn malformed_https_url_returns_typed_error() {
        let err = download_https("https://").expect_err("malformed URL must fail");
        assert!(matches!(err, ToolError::NetworkMalformed { .. }), "{err}");
    }

    #[test]
    fn air_gapped_https_error_names_url() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused port");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        let url = format!("https://{addr}/tool.wasm");
        let err = download_https(&url).expect_err("closed local port must fail");
        assert!(err.to_string().contains(&url), "{err}");
        assert!(
            matches!(
                err,
                ToolError::Network { .. }
                    | ToolError::NetworkTimeout { .. }
                    | ToolError::NetworkMalformed { .. }
            ),
            "{err}"
        );
    }

    #[test]
    fn network_error_mapping_has_timeout_and_too_large_variants() {
        let timeout = map_network_error(
            "https://example.test/tool.wasm",
            ureq::Error::Timeout(ureq::Timeout::Global),
        );
        assert!(matches!(timeout, ToolError::NetworkTimeout { .. }), "{timeout}");

        let too_large = map_network_body_error(
            "https://example.test/tool.wasm",
            ureq::Error::BodyExceedsLimit(1),
        );
        assert!(matches!(too_large, ToolError::NetworkTooLarge { .. }), "{too_large}");
    }
}
