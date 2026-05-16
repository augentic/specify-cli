//! Express route detector.
//!
//! Scans `.ts`/`.js` files for `app.get(...)`, `router.post(...)`, and
//! similar Express route-mount patterns. Only runs when `package.json`
//! lists `express` as a dependency.

use std::collections::HashMap;
use std::fs;

use regex::Regex;

use crate::survey::detector::{Detector, DetectorError, DetectorInput, DetectorOutput};
use crate::survey::dto::{Surface, SurfaceKind};

/// Detects Express HTTP routes from `.get`/`.post`/`.put`/`.delete`/`.patch`
/// call patterns.
#[derive(Debug, Clone, Copy)]
pub struct ExpressDetector;

impl Detector for ExpressDetector {
    fn name(&self) -> &'static str {
        "express"
    }

    fn detect(&self, input: &DetectorInput<'_>) -> Result<DetectorOutput, DetectorError> {
        let Some(pkg) = super::read_package_json(input.source_root)? else {
            return Ok(DetectorOutput { surfaces: vec![] });
        };
        if !super::has_dependency(&pkg, "express") {
            return Ok(DetectorOutput { surfaces: vec![] });
        }

        let files = super::walk_source_files(input.source_root)?;
        let route_re = Regex::new(
            r#"\b\w+\s*\.\s*(get|post|put|delete|patch)\s*\(\s*['"](/[^'"]*)['"]\s*(?:,\s*(.*))?"#,
        )
        .expect("constant");

        let mut surfaces = Vec::new();
        let mut seen_ids = HashMap::new();

        for file in &files {
            let content = fs::read_to_string(file).map_err(|e| DetectorError::Io {
                reason: format!("{}: {e}", file.display()),
            })?;
            let rel = super::rel_path(input.source_root, file);

            for (i, line) in content.lines().enumerate() {
                let line_num = i + 1;
                if let Some(cap) = route_re.captures(line) {
                    let method = cap.get(1).unwrap().as_str().to_uppercase();
                    let path = cap.get(2).unwrap().as_str();
                    let rest = cap.get(3).map_or("", |m| m.as_str());

                    let (handler, touches) = super::extract_handler_name(rest).map_or_else(
                        || (format!("{rel}:{line_num}"), vec![rel.clone()]),
                        |name| {
                            super::resolve_named_handler(&name, &content, file, input.source_root)
                        },
                    );

                    let base_id =
                        format!("http-{}-{}", method.to_lowercase(), super::slugify(path));
                    let id = super::dedup_id(&base_id, &mut seen_ids);

                    surfaces.push(Surface {
                        id,
                        kind: SurfaceKind::HttpRoute,
                        identifier: format!("{method} {path}"),
                        handler,
                        touches,
                        declared_at: vec![format!("{rel}:{line_num}")],
                    });
                }
            }
        }

        surfaces.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(DetectorOutput { surfaces })
    }
}
