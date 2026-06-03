//! Closed platform enum — the set of target platforms a project may
//! declare in `project.yaml`.

use serde::{Deserialize, Serialize};

/// Target platform for a Specify project.
///
/// `Core` is the shared Rust business-logic crate; every project that
/// declares platforms must include it. The shell variants (`Ios`,
/// `Android`, `Web`, `Desktop`) represent native presentation layers.
///
/// Only `Ios` and `Android` have scaffold/build/verify support today;
/// `Web` and `Desktop` are type-system placeholders signalling future
/// functionality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    /// Shared Rust business-logic crate (mandatory in every platform set).
    Core,
    /// iOS native shell (Swift + UIKit/SwiftUI).
    Ios,
    /// Android native shell (Kotlin + Compose/Views).
    Android,
    /// Web shell (future — accepted but no build/scaffold support yet).
    Web,
    /// Desktop shell (future — accepted but no build/scaffold support yet).
    Desktop,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core => f.write_str("core"),
            Self::Ios => f.write_str("ios"),
            Self::Android => f.write_str("android"),
            Self::Web => f.write_str("web"),
            Self::Desktop => f.write_str("desktop"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_kebab_case() {
        let platforms = vec![
            Platform::Core,
            Platform::Ios,
            Platform::Android,
            Platform::Web,
            Platform::Desktop,
        ];
        let yaml = serde_saphyr::to_string(&platforms).unwrap();
        assert!(yaml.contains("core"));
        assert!(yaml.contains("ios"));
        assert!(yaml.contains("android"));
        assert!(yaml.contains("web"));
        assert!(yaml.contains("desktop"));

        let back: Vec<Platform> = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(back, platforms);
    }

    #[test]
    fn deserialize_rejects_unknown_variant() {
        let result: Result<Platform, _> = serde_saphyr::from_str("\"unknown\"");
        result.unwrap_err();
    }

    #[test]
    fn display_matches_serde_name() {
        assert_eq!(Platform::Core.to_string(), "core");
        assert_eq!(Platform::Ios.to_string(), "ios");
        assert_eq!(Platform::Android.to_string(), "android");
        assert_eq!(Platform::Web.to_string(), "web");
        assert_eq!(Platform::Desktop.to_string(), "desktop");
    }

    #[test]
    fn ord_is_stable() {
        let mut platforms = vec![
            Platform::Desktop,
            Platform::Android,
            Platform::Core,
            Platform::Web,
            Platform::Ios,
        ];
        platforms.sort();
        assert_eq!(
            platforms,
            vec![
                Platform::Core,
                Platform::Ios,
                Platform::Android,
                Platform::Web,
                Platform::Desktop
            ]
        );
    }
}
