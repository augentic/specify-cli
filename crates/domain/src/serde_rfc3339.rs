//! Serde adapter for `chrono::DateTime<Utc>` rendered as RFC 3339 in
//! UTC with second precision and a literal `Z` suffix.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serializer};

/// Serialise a `DateTime<Utc>` as `%Y-%m-%dT%H:%M:%SZ`.
///
/// # Errors
///
/// Propagates any error produced by the underlying [`Serializer`].
pub fn serialize<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&dt.format("%Y-%m-%dT%H:%M:%SZ"))
}

/// Deserialise a `DateTime<Utc>` from any rfc3339 string. Accepts
/// `+00:00` and `Z` suffixes alike — the writer canonicalises to `Z`,
/// but pre-canonical fixtures must keep parsing.
///
/// # Errors
///
/// Returns the deserializer's error type when the input is not a
/// well-formed rfc3339 timestamp.
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
    let raw = String::deserialize(d)?;
    raw.parse().map_err(serde::de::Error::custom)
}

/// Adapter for `Option<DateTime<Utc>>` carrying the same wire shape.
pub mod option {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialise an `Option<DateTime<Utc>>`; `None` emits a null value.
    ///
    /// # Errors
    ///
    /// Propagates any error produced by the underlying [`Serializer`].
    pub fn serialize<S: Serializer>(opt: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
        match opt {
            Some(dt) => super::serialize(dt, s),
            None => s.serialize_none(),
        }
    }

    /// Deserialise an `Option<DateTime<Utc>>`. Missing → `None`;
    /// present-but-malformed → error.
    ///
    /// # Errors
    ///
    /// Returns the deserializer's error type when a present value is
    /// not a well-formed rfc3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        opt.map(|s| s.parse().map_err(serde::de::Error::custom)).transpose()
    }
}
