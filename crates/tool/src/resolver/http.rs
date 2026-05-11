//! HTTPS source resolution.
//!
//! Per-call timeouts, a 64 MiB body cap, and stream-to-tempfile persistence.
//! New code that adds an HTTP path must adopt the same shape (see AGENTS.md
//! §"ureq fetch hardening").

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use std::{fs, io};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use ureq::ResponseExt;

use super::AcquiredBytes;
use crate::error::ToolError;

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
pub(super) const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const STREAM_CHUNK_BYTES: usize = 64 * 1024;
const USER_AGENT: &str =
    concat!("specify-tool/", env!("CARGO_PKG_VERSION"), " (+https://github.com/augentic/specify)");

pub(super) fn download_https(url: &str, dest_hint: &Path) -> Result<AcquiredBytes, ToolError> {
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

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::super::resolve;
    use super::super::tests_common::*;
    use super::*;
    use crate::error::ToolError;
    use crate::manifest::ToolSource;
    use crate::test_support::{scratch_dir, with_cache_env};

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
