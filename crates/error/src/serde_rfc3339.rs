//! Serde adapter for `jiff::Timestamp` rendered as RFC 3339.
//!
//! Output is UTC with second precision and a literal `Z` suffix.
//! The same `with = "specify_error::serde_rfc3339"` path applies to
//! both `Timestamp` and `Option<Timestamp>` fields via the
//! [`Rfc3339`] sealed trait.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serializer};

mod sealed {
    pub trait Sealed {}
    impl Sealed for jiff::Timestamp {}
    impl Sealed for Option<jiff::Timestamp> {}
}

/// Sealed marker for the two field shapes the adapter accepts.
/// Implementations live in this module; downstream crates must not
/// extend it (the wire format is owned by `specify_error`).
pub trait Rfc3339: Sized + sealed::Sealed {
    /// Serialise `self` through `serializer`.
    ///
    /// # Errors
    ///
    /// Propagates any error produced by the underlying [`Serializer`].
    fn ser<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;

    /// Deserialise `Self` from `deserializer`.
    ///
    /// # Errors
    ///
    /// Returns the deserializer's error type when the input is not a
    /// well-formed rfc3339 timestamp (or, for the `Option` shape,
    /// when a present value is not well-formed; missing → `None`).
    fn de<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error>;
}

impl Rfc3339 for Timestamp {
    fn ser<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.strftime("%Y-%m-%dT%H:%M:%SZ"))
    }

    fn de<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

impl Rfc3339 for Option<Timestamp> {
    fn ser<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(ts) => ts.ser(serializer),
            None => serializer.serialize_none(),
        }
    }

    fn de<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        opt.map(|s| s.parse().map_err(serde::de::Error::custom)).transpose()
    }
}

/// Serialise a [`Timestamp`] (or `Option<Timestamp>`) as
/// `%Y-%m-%dT%H:%M:%SZ`. Wired by serde's `with = "…"` attribute.
///
/// # Errors
///
/// Propagates any error produced by the underlying [`Serializer`].
pub fn serialize<T: Rfc3339, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    value.ser(serializer)
}

/// Deserialise a [`Timestamp`] (or `Option<Timestamp>`) from any
/// rfc3339 string. Accepts `+00:00` and `Z` suffixes alike — the
/// writer canonicalises to `Z`, but pre-canonical fixtures must keep
/// parsing.
///
/// # Errors
///
/// Returns the deserializer's error type when the input is not a
/// well-formed rfc3339 timestamp.
pub fn deserialize<'de, T: Rfc3339, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    T::de(deserializer)
}
