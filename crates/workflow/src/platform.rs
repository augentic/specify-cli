//! Closed platform enum — the set of target platforms a project may
//! declare in `project.yaml`.

mod detect;

pub use detect::vectis_missing_platforms;

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

impl std::str::FromStr for Platform {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "core" => Ok(Self::Core),
            "ios" => Ok(Self::Ios),
            "android" => Ok(Self::Android),
            "web" => Ok(Self::Web),
            "desktop" => Ok(Self::Desktop),
            other => Err(format!(
                "unknown platform `{other}`; expected one of: core, ios, android, web, desktop"
            )),
        }
    }
}

/// Parse a comma-separated platform string into a sorted, deduplicated
/// `Vec<Platform>`. Returns an error naming the first unknown token.
///
/// # Errors
///
/// Returns a human-readable `String` when any token is not a valid
/// [`Platform`] variant.
pub fn parse_platforms_csv(csv: &str) -> Result<Vec<Platform>, String> {
    let mut platforms: Vec<Platform> = csv
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()?;
    platforms.sort();
    platforms.dedup();
    Ok(platforms)
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
    fn from_str_known_variants() {
        assert_eq!("core".parse::<Platform>().unwrap(), Platform::Core);
        assert_eq!("ios".parse::<Platform>().unwrap(), Platform::Ios);
        assert_eq!("android".parse::<Platform>().unwrap(), Platform::Android);
        assert_eq!("web".parse::<Platform>().unwrap(), Platform::Web);
        assert_eq!("desktop".parse::<Platform>().unwrap(), Platform::Desktop);
    }

    #[test]
    fn from_str_rejects_unknown() {
        "windows".parse::<Platform>().unwrap_err();
    }

    #[test]
    fn parse_csv_basic() {
        let platforms = parse_platforms_csv("core,ios,android").unwrap();
        assert_eq!(platforms, vec![Platform::Core, Platform::Ios, Platform::Android]);
    }

    #[test]
    fn parse_csv_deduplicates_and_sorts() {
        let platforms = parse_platforms_csv("android,core,ios,core").unwrap();
        assert_eq!(platforms, vec![Platform::Core, Platform::Ios, Platform::Android]);
    }

    #[test]
    fn parse_csv_rejects_unknown() {
        let err = parse_platforms_csv("core,windows").unwrap_err();
        assert!(err.contains("windows"), "error should name the bad token: {err}");
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
