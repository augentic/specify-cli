//! `NestJS` decorator-based route detector.
//!
//! Scans `.ts`/`.js` files for `@Controller(...)` class decorators and
//! `@Get`/`@Post`/`@Put`/`@Delete`/`@Patch` method decorators,
//! combining controller and method paths into HTTP-route surfaces.
//! Also recognises `@MessagePattern(...)` and `@EventPattern(...)` from
//! `@nestjs/microservices`.
//!
//! Only runs when `package.json` lists an `@nestjs/*` dependency.

use std::collections::HashMap;
use std::fs;

use regex::Regex;

use crate::survey::detector::{Detector, DetectorError, DetectorInput, DetectorOutput};
use crate::survey::dto::{Surface, SurfaceKind};

/// Detects `NestJS` HTTP routes and microservice message patterns from
/// decorator metadata.
#[derive(Debug, Clone, Copy)]
pub struct NestJsDetector;

impl Detector for NestJsDetector {
    fn name(&self) -> &'static str {
        "nestjs"
    }

    fn detect(&self, input: &DetectorInput<'_>) -> Result<DetectorOutput, DetectorError> {
        let Some(pkg) = super::read_package_json(input.source_root)? else {
            return Ok(DetectorOutput { surfaces: vec![] });
        };
        if !super::has_dependency_prefix(&pkg, "@nestjs/") {
            return Ok(DetectorOutput { surfaces: vec![] });
        }

        let files = super::walk_source_files(input.source_root)?;

        let ctrl_re =
            Regex::new(r#"@Controller\s*\(\s*(?:'([^']*)'|"([^"]*)")?\s*\)"#).expect("constant");
        let class_re = Regex::new(r"class\s+(\w+)").expect("constant");
        let method_re =
            Regex::new(r#"@(Get|Post|Put|Delete|Patch)\s*\(\s*(?:'([^']*)'|"([^"]*)")?\s*\)"#)
                .expect("constant");
        let msg_re =
            Regex::new(r#"@(MessagePattern|EventPattern)\s*\(\s*(?:'([^']*)'|"([^"]*)")\s*\)"#)
                .expect("constant");
        let fn_re = Regex::new(r"^\s*(?:async\s+)?(\w+)\s*\(").expect("constant");

        let mut surfaces = Vec::new();
        let mut seen_ids = HashMap::new();

        for file in &files {
            let content = fs::read_to_string(file).map_err(|e| DetectorError::Io {
                reason: format!("{}: {e}", file.display()),
            })?;
            let rel = super::rel_path(input.source_root, file);
            let touches = super::resolve_touches(input.source_root, file);

            let mut ctrl_path: Option<String> = None;
            let mut class_name: Option<String> = None;
            let mut pending: Option<PendingDecorator> = None;

            for (i, line) in content.lines().enumerate() {
                let line_num = i + 1;

                if let Some(cap) = ctrl_re.captures(line) {
                    ctrl_path = Some(
                        cap.get(1)
                            .or_else(|| cap.get(2))
                            .map_or(String::new(), |m| m.as_str().to_string()),
                    );
                }
                if let Some(cap) = class_re.captures(line) {
                    class_name = Some(cap.get(1).unwrap().as_str().to_string());
                }

                if let Some(cap) = method_re.captures(line) {
                    let http = cap.get(1).unwrap().as_str().to_uppercase();
                    let sub = cap
                        .get(2)
                        .or_else(|| cap.get(3))
                        .map_or(String::new(), |m| m.as_str().to_string());
                    pending = Some(PendingDecorator::Http(http, sub, line_num));
                    continue;
                }
                if let Some(cap) = msg_re.captures(line) {
                    let pattern = cap
                        .get(2)
                        .or_else(|| cap.get(3))
                        .map_or(String::new(), |m| m.as_str().to_string());
                    pending = Some(PendingDecorator::Message(pattern, line_num));
                    continue;
                }

                if let Some(decorator) = pending.take()
                    && let Some(cap) = fn_re.captures(line)
                {
                    let method_name = cap.get(1).unwrap().as_str();
                    let cn = class_name.as_deref().unwrap_or("Unknown");

                    match decorator {
                        PendingDecorator::Http(http, sub, decl_line) => {
                            let cp = ctrl_path.as_deref().unwrap_or("");
                            let full = combine_paths(cp, &sub);
                            let base_id =
                                format!("http-{}-{}", http.to_lowercase(), super::slugify(&full));
                            let id = super::dedup_id(&base_id, &mut seen_ids);

                            surfaces.push(Surface {
                                id,
                                kind: SurfaceKind::HttpRoute,
                                identifier: format!("{http} {full}"),
                                handler: format!("{rel}:{cn}.{method_name}"),
                                touches: touches.clone(),
                                declared_at: vec![format!("{rel}:{decl_line}")],
                            });
                        }
                        PendingDecorator::Message(pattern, decl_line) => {
                            let base_id = format!("message-sub-{}", super::slugify(&pattern));
                            let id = super::dedup_id(&base_id, &mut seen_ids);

                            surfaces.push(Surface {
                                id,
                                kind: SurfaceKind::MessageSub,
                                identifier: pattern,
                                handler: format!("{rel}:{cn}.{method_name}"),
                                touches: touches.clone(),
                                declared_at: vec![format!("{rel}:{decl_line}")],
                            });
                        }
                    }
                }
            }
        }

        surfaces.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(DetectorOutput { surfaces })
    }
}

enum PendingDecorator {
    Http(String, String, usize),
    Message(String, usize),
}

fn combine_paths(controller: &str, method: &str) -> String {
    let mut parts = Vec::new();
    if !controller.is_empty() {
        parts.push(controller);
    }
    if !method.is_empty() {
        parts.push(method);
    }
    if parts.is_empty() { "/".to_string() } else { format!("/{}", parts.join("/")) }
}
