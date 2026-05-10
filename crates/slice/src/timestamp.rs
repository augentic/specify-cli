use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

/// A second-precision UTC timestamp string in `%Y-%m-%dT%H:%M:%SZ` form.
///
/// Stored transparently as a YAML/JSON string; callers are expected to construct via
/// [`format_rfc3339`](crate::actions::format_rfc3339) or another well-formed
/// source — this type does not validate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Rfc3339Stamp(String);

impl Rfc3339Stamp {
    /// Wrap a raw string as an `Rfc3339Stamp`. Does not validate.
    #[must_use]
    pub const fn new(s: String) -> Self {
        Self(s)
    }
}

impl Deref for Rfc3339Stamp {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Rfc3339Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Rfc3339Stamp {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_inner_string() {
        let stamp = Rfc3339Stamp::new("2024-08-01T10:00:00Z".to_string());
        assert_eq!(stamp.to_string(), "2024-08-01T10:00:00Z");
    }

    #[test]
    fn deref_to_str() {
        let stamp = Rfc3339Stamp::new("2024-08-01T10:00:00Z".to_string());
        let s: &str = &stamp;
        assert_eq!(s, "2024-08-01T10:00:00Z");
    }

    #[test]
    fn serde_round_trip_json() {
        let stamp = Rfc3339Stamp::new("2024-08-01T10:00:00Z".to_string());
        let json = serde_json::to_string(&stamp).unwrap();
        assert_eq!(json, "\"2024-08-01T10:00:00Z\"");
        let parsed: Rfc3339Stamp = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, stamp);
    }

    #[test]
    fn serde_round_trip_yaml() {
        let stamp = Rfc3339Stamp::new("2024-08-01T10:00:00Z".to_string());
        let yaml = serde_saphyr::to_string(&stamp).unwrap();
        let parsed: Rfc3339Stamp = serde_saphyr::from_str(yaml.trim()).unwrap();
        assert_eq!(parsed, stamp);
    }

    #[test]
    fn ord_sorts_chronologically() {
        let a = Rfc3339Stamp::new("2024-01-01T00:00:00Z".to_string());
        let b = Rfc3339Stamp::new("2024-06-15T12:00:00Z".to_string());
        assert!(a < b);
    }
}
