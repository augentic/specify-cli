//! Serde adapter for `jiff::Timestamp` rendered as RFC 3339 in
//! UTC with second precision and a literal `Z` suffix.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serializer};

/// Serialise a [`Timestamp`] as `%Y-%m-%dT%H:%M:%SZ`.
///
/// # Errors
///
/// Propagates any error produced by the underlying [`Serializer`].
pub fn serialize<S: Serializer>(ts: &Timestamp, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&ts.strftime("%Y-%m-%dT%H:%M:%SZ"))
}

/// Deserialise a [`Timestamp`] from any rfc3339 string. Accepts
/// `+00:00` and `Z` suffixes alike — the writer canonicalises to `Z`,
/// but pre-canonical fixtures must keep parsing.
///
/// # Errors
///
/// Returns the deserializer's error type when the input is not a
/// well-formed rfc3339 timestamp.
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Timestamp, D::Error> {
    let raw = String::deserialize(d)?;
    raw.parse().map_err(serde::de::Error::custom)
}

/// Adapter for `Option<Timestamp>` carrying the same wire shape.
pub mod option {
    use jiff::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialise an `Option<Timestamp>`; `None` emits a null value.
    ///
    /// # Errors
    ///
    /// Propagates any error produced by the underlying [`Serializer`].
    pub fn serialize<S: Serializer>(opt: &Option<Timestamp>, s: S) -> Result<S::Ok, S::Error> {
        match opt {
            Some(ts) => super::serialize(ts, s),
            None => s.serialize_none(),
        }
    }

    /// Deserialise an `Option<Timestamp>`. Missing → `None`;
    /// present-but-malformed → error.
    ///
    /// # Errors
    ///
    /// Returns the deserializer's error type when a present value is
    /// not a well-formed rfc3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Timestamp>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        opt.map(|s| s.parse().map_err(serde::de::Error::custom)).transpose()
    }
}
