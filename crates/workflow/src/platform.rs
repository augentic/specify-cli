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
    fn wire_names_coherent() {
        // `Display` and `FromStr` are hand-written matches that must not
        // drift from the `#[serde(rename_all = "kebab-case")]` wire name.
        for platform in
            [Platform::Core, Platform::Ios, Platform::Android, Platform::Web, Platform::Desktop]
        {
            let name = platform.to_string();
            assert_eq!(name.parse::<Platform>().unwrap(), platform, "FromStr(Display) round trip");
            let yaml = serde_saphyr::to_string(&platform).unwrap();
            assert_eq!(yaml.trim(), name, "serde wire name must match Display");
        }
    }

    #[test]
    fn parse_csv_edge_cases() {
        // Whitespace and empty tokens are tolerated; duplicates collapse;
        // output is sorted with `Core` first; the first unknown token is
        // named in the error.
        let platforms = parse_platforms_csv(" android , core ,ios,core,").unwrap();
        assert_eq!(platforms, vec![Platform::Core, Platform::Ios, Platform::Android]);

        let err = parse_platforms_csv("core,windows").unwrap_err();
        assert!(err.contains("windows"), "error should name the bad token: {err}");
    }
}
