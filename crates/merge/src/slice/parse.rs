//! Time conversions shared by [`super::commit`] and
//! [`super::conflict_check`].
//!
//! The slice merge engine consumes two flavours of timestamp:
//!
//! - rfc3339 strings stamped into `.metadata.yaml` (`defined_at`).
//! - `std::fs::Metadata::modified()` on baseline files, surfaced as
//!   `chrono::DateTime<Utc>` for direct comparison against `defined_at`.

use std::time::SystemTime;

use chrono::{DateTime, Utc};
use specify_error::Error;

/// Parse an rfc3339 string into a UTC instant. Used to lift the
/// `.metadata.yaml.defined_at` stamp into a comparable instant when
/// detecting baseline drift.
pub(super) fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&Utc))
}

/// Convert a [`SystemTime`] (as returned by `fs::metadata().modified()`)
/// into UTC. Returns a typed `Error::Diag` rather than panicking when
/// the host clock reports a baseline mtime that predates the UNIX
/// epoch, overflows `i64` seconds, or otherwise falls outside chrono's
/// representable range.
pub(super) fn system_time_to_utc(t: SystemTime) -> Result<DateTime<Utc>, Error> {
    let duration = t.duration_since(SystemTime::UNIX_EPOCH).map_err(|err| Error::Diag {
        code: "merge-mtime-pre-epoch",
        detail: format!("baseline mtime predates the UNIX epoch: {err}"),
    })?;
    let secs = i64::try_from(duration.as_secs()).map_err(|err| Error::Diag {
        code: "merge-mtime-overflow",
        detail: format!("baseline mtime overflow: {err}"),
    })?;
    let nanos = duration.subsec_nanos();
    DateTime::<Utc>::from_timestamp(secs, nanos).ok_or_else(|| Error::Diag {
        code: "merge-mtime-out-of-range",
        detail: "baseline mtime out of range".to_string(),
    })
}
