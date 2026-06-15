//! Conventional export paths for `vectis materialize assets` (RFC-46 §2, Resolved §7).
//!
//! Paths are relative to the directory containing `assets.yaml` (typically
//! `design-system/`) and use the `assets/exports/<platform>/…` prefix.

use std::path::{Path, PathBuf};

/// Android drawable density buckets for rasterized exports.
pub const ANDROID_DENSITIES: &[&str] = &["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"];

/// iOS raster scales for vector illustration materialize (`@2x` / `@3x` only).
pub const IOS_ILLUSTRATION_SCALES: &[&str] = &["2x", "3x"];

/// iOS raster scales accepted when copying per-density photo masters.
pub const IOS_RASTER_SCALES: &[&str] = &["1x", "2x", "3x"];

/// Android density scale factors relative to the SVG 1× logical canvas (mdpi baseline).
#[must_use]
pub fn android_density_factor(density: &str) -> Option<f32> {
    match density {
        "mdpi" => Some(1.0),
        "hdpi" => Some(1.5),
        "xhdpi" => Some(2.0),
        "xxhdpi" => Some(3.0),
        "xxxhdpi" => Some(4.0),
        _ => None,
    }
}

/// iOS imageset scale factor relative to the SVG 1× logical canvas.
#[must_use]
pub fn ios_scale_factor(scale: &str) -> Option<f32> {
    match scale {
        "1x" => Some(1.0),
        "2x" => Some(2.0),
        "3x" => Some(3.0),
        _ => None,
    }
}

/// iOS imageset PNG filename for a raster export (`1x` omits the `@` suffix).
#[must_use]
pub fn ios_raster_filename(asset_id: &str, scale: &str) -> String {
    if scale == "1x" {
        format!("{asset_id}.png")
    } else {
        format!("{asset_id}@{scale}.png")
    }
}

/// Design-system-relative path for one iOS raster artifact inside an imageset.
#[must_use]
pub fn ios_raster_artifact_rel(asset_id: &str, scale: &str) -> String {
    format!(
        "{}/{}",
        ios_imageset_dir(asset_id),
        ios_raster_filename(asset_id, scale)
    )
}

/// Design-system-relative path for one Android raster drawable PNG.
#[must_use]
pub fn android_raster_artifact_rel(asset_id: &str, density: &str) -> String {
    let snake = kebab_to_snake(asset_id);
    format!(
        "{}/drawable-{density}/{snake}.png",
        exports_root(Platform::Android)
    )
}

/// Target platform for export path computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Ios,
    Android,
}

impl Platform {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }

    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "ios" => Some(Self::Ios),
            "android" => Some(Self::Android),
            _ => None,
        }
    }
}

/// Resolved export layout for one asset platform slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportLayout {
    /// Value recorded in `sources.<platform>` after auto-write (Resolved §7).
    pub pin: String,
    /// Artifact paths materialize will create under `design-system/`.
    pub artifacts: Vec<String>,
}

/// Translate a kebab-case asset id to `snake_case` for Android `R.drawable` names.
#[must_use]
pub fn kebab_to_snake(id: &str) -> String {
    id.replace('-', "_")
}

/// Compute the conventional export layout for auto-materialize from `source:`.
///
/// Returns `None` for roles/kinds that do not auto-convert from a canonical
/// master (`symbol`, `photo`, raster UI icons without `source:`, etc.).
#[must_use]
pub fn export_layout(role: &str, kind: &str, platform: Platform, asset_id: &str) -> Option<ExportLayout> {
    let materialize_role = resolve_materialize_role(role, kind)?;
    Some(match materialize_role {
        MaterializeRole::IconVector => icon_vector_layout(platform, asset_id),
        MaterializeRole::IllustrationVector => illustration_vector_layout(platform, asset_id),
        MaterializeRole::AppIcon => app_icon_layout(platform),
    })
}

/// Join `assets/exports/<platform>/…` under the design-system root.
#[must_use]
pub fn exports_root(platform: Platform) -> String {
    format!("assets/exports/{}", platform.as_str())
}

/// iOS imageset directory for a kebab-case asset id.
#[must_use]
pub fn ios_imageset_dir(asset_id: &str) -> String {
    format!("{}/{}.imageset", exports_root(Platform::Ios), asset_id)
}

fn resolve_materialize_role(role: &str, kind: &str) -> Option<MaterializeRole> {
    match (role, kind) {
        ("app-icon", _) => Some(MaterializeRole::AppIcon),
        ("icon" | "decorative", "vector") => Some(MaterializeRole::IconVector),
        ("illustration", "vector") => Some(MaterializeRole::IllustrationVector),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaterializeRole {
    IconVector,
    IllustrationVector,
    AppIcon,
}

fn icon_vector_layout(platform: Platform, asset_id: &str) -> ExportLayout {
    match platform {
        Platform::Ios => {
            let imageset = ios_imageset_dir(asset_id);
            let pdf = format!("{imageset}/{asset_id}.pdf");
            ExportLayout {
                pin: pdf.clone(),
                artifacts: vec![pdf, format!("{imageset}/Contents.json")],
            }
        }
        Platform::Android => {
            let snake = kebab_to_snake(asset_id);
            let xml = format!("{}/drawable/{snake}.xml", exports_root(platform));
            ExportLayout { pin: xml.clone(), artifacts: vec![xml] }
        }
    }
}

fn illustration_vector_layout(platform: Platform, asset_id: &str) -> ExportLayout {
    match platform {
        Platform::Ios => {
            let imageset = ios_imageset_dir(asset_id);
            let mut artifacts = IOS_ILLUSTRATION_SCALES
                .iter()
                .map(|scale| ios_raster_artifact_rel(asset_id, scale))
                .collect::<Vec<_>>();
            let pin = artifacts.last().expect("illustration scales non-empty").clone();
            artifacts.push(format!("{imageset}/Contents.json"));
            ExportLayout { pin, artifacts }
        }
        Platform::Android => {
            let artifacts = ANDROID_DENSITIES
                .iter()
                .map(|density| android_raster_artifact_rel(asset_id, density))
                .collect::<Vec<_>>();
            let pin = artifacts.last().expect("android densities non-empty").clone();
            ExportLayout { pin, artifacts }
        }
    }
}

fn app_icon_layout(platform: Platform) -> ExportLayout {
    match platform {
        Platform::Ios => {
            let root = format!("{}/app-icon/AppIcon.appiconset", exports_root(platform));
            ExportLayout { pin: root.clone(), artifacts: vec![root] }
        }
        Platform::Android => {
            let root = format!("{}/app-icon", exports_root(platform));
            ExportLayout { pin: root.clone(), artifacts: vec![root] }
        }
    }
}

/// Resolve a design-system-relative pin path to an absolute path under `assets_dir`.
#[must_use]
pub fn resolve_under_assets_dir(assets_dir: &Path, pin_rel: &str) -> PathBuf {
    assets_dir.join(pin_rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_to_snake_translates_drawable_ids() {
        assert_eq!(kebab_to_snake("onboarding-hero"), "onboarding_hero");
        assert_eq!(kebab_to_snake("settings"), "settings");
    }

    #[test]
    fn icon_vector_ios_paths() {
        let layout = export_layout("icon", "vector", Platform::Ios, "settings")
            .expect("icon vector ios");
        assert_eq!(
            layout.pin,
            "assets/exports/ios/settings.imageset/settings.pdf"
        );
        assert_eq!(
            layout.artifacts,
            vec![
                "assets/exports/ios/settings.imageset/settings.pdf".to_string(),
                "assets/exports/ios/settings.imageset/Contents.json".to_string(),
            ]
        );
    }

    #[test]
    fn icon_vector_android_paths() {
        let layout = export_layout("icon", "vector", Platform::Android, "chevron-right")
            .expect("icon vector android");
        assert_eq!(
            layout.pin,
            "assets/exports/android/drawable/chevron_right.xml"
        );
        assert_eq!(layout.artifacts, vec![layout.pin.clone()]);
    }

    #[test]
    fn decorative_vector_follows_icon_paths() {
        let ios = export_layout("decorative", "vector", Platform::Ios, "sparkle")
            .expect("decorative ios");
        let icon = export_layout("icon", "vector", Platform::Ios, "sparkle")
            .expect("icon ios");
        assert_eq!(ios, icon);
    }

    #[test]
    fn scale_factors_match_platform_conventions() {
        assert_eq!(ios_scale_factor("2x"), Some(2.0_f32));
        assert_eq!(ios_scale_factor("3x"), Some(3.0_f32));
        assert_eq!(android_density_factor("mdpi"), Some(1.0_f32));
        assert_eq!(android_density_factor("hdpi"), Some(1.5_f32));
        assert_eq!(android_density_factor("xhdpi"), Some(2.0_f32));
        assert_eq!(android_density_factor("xxhdpi"), Some(3.0_f32));
        assert_eq!(android_density_factor("xxxhdpi"), Some(4.0_f32));
    }

    #[test]
    fn ios_raster_filenames_follow_imageset_conventions() {
        assert_eq!(ios_raster_filename("hero", "1x"), "hero.png");
        assert_eq!(ios_raster_filename("hero", "2x"), "hero@2x.png");
    }

    #[test]
    fn illustration_ios_paths() {
        let layout = export_layout("illustration", "vector", Platform::Ios, "onboarding-hero")
            .expect("illustration ios");
        assert_eq!(
            layout.pin,
            "assets/exports/ios/onboarding-hero.imageset/onboarding-hero@3x.png"
        );
        assert_eq!(
            layout.artifacts,
            vec![
                "assets/exports/ios/onboarding-hero.imageset/onboarding-hero@2x.png".to_string(),
                "assets/exports/ios/onboarding-hero.imageset/onboarding-hero@3x.png".to_string(),
                "assets/exports/ios/onboarding-hero.imageset/Contents.json".to_string(),
            ]
        );
    }

    #[test]
    fn illustration_android_paths() {
        let layout =
            export_layout("illustration", "vector", Platform::Android, "onboarding-hero")
                .expect("illustration android");
        assert_eq!(
            layout.pin,
            "assets/exports/android/drawable-xxxhdpi/onboarding_hero.png"
        );
        assert_eq!(
            layout.artifacts,
            vec![
                "assets/exports/android/drawable-mdpi/onboarding_hero.png".to_string(),
                "assets/exports/android/drawable-hdpi/onboarding_hero.png".to_string(),
                "assets/exports/android/drawable-xhdpi/onboarding_hero.png".to_string(),
                "assets/exports/android/drawable-xxhdpi/onboarding_hero.png".to_string(),
                "assets/exports/android/drawable-xxxhdpi/onboarding_hero.png".to_string(),
            ]
        );
    }

    #[test]
    fn app_icon_export_roots() {
        let ios = export_layout("app-icon", "vector", Platform::Ios, "app-icon")
            .expect("app-icon ios");
        assert_eq!(
            ios.pin,
            "assets/exports/ios/app-icon/AppIcon.appiconset"
        );

        let android = export_layout("app-icon", "raster", Platform::Android, "app-icon")
            .expect("app-icon android");
        assert_eq!(android.pin, "assets/exports/android/app-icon");
    }

    #[test]
    fn unsupported_roles_return_none() {
        assert!(export_layout("photo", "raster", Platform::Ios, "hero").is_none());
        assert!(export_layout("icon", "symbol", Platform::Ios, "close").is_none());
        assert!(export_layout("icon", "raster", Platform::Android, "badge").is_none());
    }
}
