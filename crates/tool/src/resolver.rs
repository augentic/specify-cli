//! Source resolution for local paths, `file:` URIs, and `https:` URIs.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, io};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use ureq::ResponseExt;

use crate::cache::{
    self, CacheStatus, MODULE_FILENAME, PermissionsSnapshot, SIDECAR_FILENAME, Sidecar,
};
use crate::error::ToolError;
use crate::manifest::{Tool, ToolScope, ToolSource};

/// Whole-call cap; covers DNS + connect + headers + body.
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
/// TCP + TLS handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Sending request headers / body. WASI tool fetches issue empty bodies, but
/// the explicit cap defends against a peer that stalls during the request.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
/// Receiving the response body. Generous to allow large WASI components on
/// slow links without breaching the global cap.
const RECV_BODY_TIMEOUT: Duration = Duration::from_mins(1);
/// Receiving the response headers.
const RECV_HEADERS_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum accepted WASI component download size. Larger payloads abort
/// streaming before they exhaust the cache filesystem.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;
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
/// `now` records the sidecar `fetched_at`; the dispatcher passes
/// `Utc::now`, tests pin a deterministic stamp.
///
/// # Errors
///
/// Returns cache errors, source read errors, digest mismatches, or typed network
/// resolver errors.
pub fn resolve(
    scope: &ToolScope, tool: &Tool, now: chrono::DateTime<chrono::Utc>,
) -> Result<ResolvedTool, ToolError> {
    let source = tool.source.to_wire_string().into_owned();
    let module = cache::module_path(scope, &tool.name, &tool.version)?;
    if cache::cache_status(scope, &tool.name, &tool.version, &source, tool.sha256.as_deref())?
        == CacheStatus::Hit
        && cached_digest_matches(&module, tool.sha256.as_deref())?
    {
        return Ok(resolved(scope, tool, module));
    }

    let dest = cache::tool_dir(scope, &tool.name, &tool.version)?;
    let staged = unique_staging_dir(&dest)?;
    let install_result = stage_and_install(scope, tool, &source, &staged, &dest, now);
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
    scope: &ToolScope, tool: &Tool, source: &str, staged: &Path, dest: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ToolError> {
    let module_dest = staged.join(MODULE_FILENAME);
    let acquired = acquire_source_bytes(&tool.source, &module_dest)?;
    validate_acquired_bytes(source, &acquired, tool.sha256.as_deref())?;
    acquired.persist_to(&module_dest)?;
    let sidecar = Sidecar::new(
        scope,
        &tool.name,
        &tool.version,
        source,
        PermissionsSnapshot::from(&tool.permissions),
        tool.sha256.clone(),
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

fn cached_digest_matches(module: &Path, expected: Option<&str>) -> Result<bool, ToolError> {
    let Some(expected) = expected else {
        return Ok(true);
    };
    let bytes = fs::read(module).map_err(|err| {
        ToolError::cache_io("read cached module for sha256 verification", module, err)
    })?;
    Ok(sha256_hex(&bytes) == expected)
}

/// Bytes acquired from a tool source, ready for digest validation and
/// installation into the cache. HTTPS streams to a sibling `NamedTempFile`
/// (so the bytes never live in a `Vec`); local sources read into memory
/// because their bodies are bounded by the on-disk source file.
#[derive(Debug)]
enum AcquiredBytes {
    Buffered(Vec<u8>),
    Streamed { temp: NamedTempFile, sha256: String },
}

impl AcquiredBytes {
    fn len(&self) -> Result<u64, ToolError> {
        match self {
            Self::Buffered(bytes) => Ok(bytes.len() as u64),
            Self::Streamed { temp, .. } => temp
                .as_file()
                .metadata()
                .map(|m| m.len())
                .map_err(|err| ToolError::cache_io("stat staged tool body", temp.path(), err)),
        }
    }

    fn sha256_hex(&self) -> String {
        match self {
            Self::Buffered(bytes) => sha256_hex(bytes),
            Self::Streamed { sha256, .. } => sha256.clone(),
        }
    }

    fn persist_to(self, dest: &Path) -> Result<(), ToolError> {
        match self {
            Self::Buffered(bytes) => fs::write(dest, bytes)
                .map_err(|err| ToolError::cache_io("write staged module", dest, err)),
            Self::Streamed { temp, .. } => {
                temp.persist(dest).map(|_| ()).map_err(|err| ToolError::AtomicMoveFailed {
                    from: err.file.path().to_path_buf(),
                    to: dest.to_path_buf(),
                    source: err.error,
                })
            }
        }
    }
}

fn acquire_source_bytes(source: &ToolSource, dest_hint: &Path) -> Result<AcquiredBytes, ToolError> {
    match source {
        ToolSource::LocalPath(path) => {
            read_local_path(path, &path.to_string_lossy()).map(AcquiredBytes::Buffered)
        }
        ToolSource::FileUri(uri) => read_file_uri(uri).map(AcquiredBytes::Buffered),
        ToolSource::HttpsUri(url) => download_https(url, dest_hint),
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

fn download_https(url: &str, dest_hint: &Path) -> Result<AcquiredBytes, ToolError> {
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

    // Fail fast when the server advertises a body larger than the cap. Without
    // a Content-Length the cap is enforced while streaming below.
    if let Some(length) = response.body().content_length()
        && length > MAX_RESPONSE_BYTES
    {
        return Err(ToolError::NetworkTooLarge {
            url: url.to_string(),
            limit: MAX_RESPONSE_BYTES,
            actual: Some(length),
        });
    }

    let temp_parent = dest_hint.parent().ok_or_else(|| {
        ToolError::CacheRoot(format!(
            "tool download destination has no parent: {}",
            dest_hint.display()
        ))
    })?;
    fs::create_dir_all(temp_parent)
        .map_err(|err| ToolError::cache_io("create download staging parent", temp_parent, err))?;
    let temp = NamedTempFile::new_in(temp_parent)
        .map_err(|err| ToolError::cache_io("create download tempfile", temp_parent, err))?;

    let mut reader = response.body_mut().with_config().limit(MAX_RESPONSE_BYTES + 1).reader();
    let sha256 = stream_to_tempfile(url, &mut reader, &temp)?;
    Ok(AcquiredBytes::Streamed { temp, sha256 })
}

fn stream_to_tempfile<R: Read>(
    url: &str, reader: &mut R, temp: &NamedTempFile,
) -> Result<String, ToolError> {
    let mut hasher = Sha256::new();
    let mut writer = io::BufWriter::with_capacity(STREAM_CHUNK_BYTES, temp.as_file());
    let mut buf = vec![0u8; STREAM_CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                if let Some(ureq_err) = err.get_ref().and_then(|e| e.downcast_ref::<ureq::Error>())
                {
                    return Err(map_streamed_body_error(url, ureq_err));
                }
                return Err(ToolError::Network {
                    url: url.to_string(),
                    detail: err.to_string(),
                });
            }
        };
        total = total.saturating_add(n as u64);
        if total > MAX_RESPONSE_BYTES {
            return Err(ToolError::NetworkTooLarge {
                url: url.to_string(),
                limit: MAX_RESPONSE_BYTES,
                actual: Some(total),
            });
        }
        hasher.update(&buf[..n]);
        writer
            .write_all(&buf[..n])
            .map_err(|err| ToolError::cache_io("write download tempfile", temp.path(), err))?;
    }
    writer
        .flush()
        .map_err(|err| ToolError::cache_io("flush download tempfile", temp.path(), err))?;
    drop(writer);
    temp.as_file()
        .sync_all()
        .map_err(|err| ToolError::cache_io("sync download tempfile", temp.path(), err))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn https_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_send_request(Some(SEND_TIMEOUT))
        .timeout_send_body(Some(SEND_TIMEOUT))
        .timeout_recv_response(Some(RECV_HEADERS_TIMEOUT))
        .timeout_recv_body(Some(RECV_BODY_TIMEOUT))
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

fn map_streamed_body_error(url: &str, err: &ureq::Error) -> ToolError {
    match err {
        ureq::Error::Timeout(timeout) => ToolError::NetworkTimeout {
            url: url.to_string(),
            detail: timeout.to_string(),
        },
        ureq::Error::BodyExceedsLimit(limit) => ToolError::NetworkTooLarge {
            url: url.to_string(),
            limit: MAX_RESPONSE_BYTES,
            actual: Some(*limit),
        },
        err => ToolError::Network {
            url: url.to_string(),
            detail: err.to_string(),
        },
    }
}

fn validate_acquired_bytes(
    source: &str, acquired: &AcquiredBytes, expected_sha256: Option<&str>,
) -> Result<(), ToolError> {
    if acquired.len()? == 0 {
        return Err(ToolError::EmptySource {
            source_value: source.to_string(),
        });
    }

    if let Some(expected) = expected_sha256 {
        let actual = acquired.sha256_hex();
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

    use chrono::{DateTime, Utc};

    use super::*;
    use crate::manifest::{ToolPermissions, ToolSource};
    use crate::test_support::{scratch_dir, with_cache_env};

    fn fixed_now() -> DateTime<Utc> {
        "2026-05-07T00:00:00Z".parse().expect("fixed test stamp")
    }

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
            let resolved = resolve(&scope, &first_tool, fixed_now()).expect("cache miss resolves");
            assert_eq!(fs::read(&resolved.bytes_path).expect("cached bytes"), b"first");

            fs::write(&first, b"changed-at-source").expect("mutate source");
            let hit = resolve(&scope, &first_tool, fixed_now()).expect("cache hit resolves");
            assert_eq!(hit.bytes_path, resolved.bytes_path);
            assert_eq!(cached_bytes(&scope, &first_tool), b"first");

            let changed_tool = tool(ToolSource::LocalPath(second), None);
            let changed =
                resolve(&scope, &changed_tool, fixed_now()).expect("changed source re-stages");
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
            resolve(&scope, &old_tool, fixed_now()).expect("initial digest install");
            let err =
                resolve(&scope, &wrong_tool, fixed_now()).expect_err("wrong digest must fail");
            assert!(matches!(err, ToolError::DigestMismatch { .. }), "{err}");
            assert_eq!(cached_bytes(&scope, &old_tool), b"old-good");

            let correct_tool =
                tool(ToolSource::LocalPath(new_source), Some(sha256_hex(b"new-good")));
            resolve(&scope, &correct_tool, fixed_now()).expect("correct digest updates cache");
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
            let resolved = resolve(&scope, &pinned, fixed_now()).expect("install pinned");
            fs::write(&resolved.bytes_path, b"corrupt").expect("corrupt cache");
            let repaired =
                resolve(&scope, &pinned, fixed_now()).expect("digest mismatch re-fetches");
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
            let local = resolve(&scope, &local, fixed_now()).expect("local resolves");
            let uri = resolve(&scope, &file_uri, fixed_now()).expect("file URI resolves");
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
            let dir_err = resolve(
                &scope,
                &tool(ToolSource::LocalPath(source_dir.clone()), None),
                fixed_now(),
            )
            .expect_err("directory source must fail");
            assert!(matches!(dir_err, ToolError::InvalidSource { .. }), "{dir_err}");

            let empty_err = resolve(&scope, &tool(ToolSource::LocalPath(empty), None), fixed_now())
                .expect_err("empty file");
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
            let resolved = resolve(&scope, &symlink_tool, fixed_now()).expect("symlink resolves");
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
            let project_resolved =
                resolve(&project, &declared, fixed_now()).expect("project resolve");
            let capability_resolved =
                resolve(&capability, &declared, fixed_now()).expect("capability resolve");
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
            let err = resolve(&scope, &declared, fixed_now()).expect_err("http must be rejected");
            assert!(matches!(err, ToolError::InvalidSource { .. }), "{err}");
        });
    }

    #[test]
    fn malformed_https_url_returns_typed_error() {
        let dest = scratch_dir("resolver-malformed-https").join("module.wasm");
        let err = download_https("https://", &dest).expect_err("malformed URL must fail");
        assert!(matches!(err, ToolError::NetworkMalformed { .. }), "{err}");
    }

    #[test]
    fn air_gapped_https_error_names_url() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused port");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        let url = format!("https://{addr}/tool.wasm");
        let dest = scratch_dir("resolver-air-gapped-https").join("module.wasm");
        let err = download_https(&url, &dest).expect_err("closed local port must fail");
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

        let too_large = map_streamed_body_error(
            "https://example.test/tool.wasm",
            &ureq::Error::BodyExceedsLimit(1),
        );
        assert!(matches!(too_large, ToolError::NetworkTooLarge { .. }), "{too_large}");
    }
}
