//! `BullMQ` queue/worker detector.
//!
//! Scans `.ts`/`.js` files for `new Queue(name, …)` (message-pub) and
//! `new Worker(name, processor, …)` (message-sub) construction sites.
//! Only runs when `package.json` lists `bullmq` as a dependency.

use std::collections::HashMap;
use std::fs;

use regex::Regex;

use crate::survey::detector::{Detector, DetectorError, DetectorInput, DetectorOutput};
use crate::survey::dto::{Surface, SurfaceKind};

/// Detects `BullMQ` queues (message-pub) and workers (message-sub) from
/// constructor call sites.
#[derive(Debug, Clone, Copy)]
pub struct BullMqDetector;

impl Detector for BullMqDetector {
    fn name(&self) -> &'static str {
        "bullmq"
    }

    fn detect(&self, input: &DetectorInput<'_>) -> Result<DetectorOutput, DetectorError> {
        let Some(pkg) = super::read_package_json(input.source_root)? else {
            return Ok(DetectorOutput { surfaces: vec![] });
        };
        if !super::has_dependency(&pkg, "bullmq") {
            return Ok(DetectorOutput { surfaces: vec![] });
        }

        let files = super::walk_source_files(input.source_root)?;
        let queue_re = Regex::new(r#"new\s+Queue\s*\(\s*['"]([^'"]+)['"]"#).expect("constant");
        let worker_re =
            Regex::new(r#"new\s+Worker\s*\(\s*['"]([^'"]+)['"]\s*,\s*(\S+)"#).expect("constant");

        let mut surfaces = Vec::new();
        let mut seen_ids = HashMap::new();

        for file in &files {
            let content = fs::read_to_string(file).map_err(|e| DetectorError::Io {
                reason: format!("{}: {e}", file.display()),
            })?;
            let rel = super::rel_path(input.source_root, file);

            for (i, line) in content.lines().enumerate() {
                let line_num = i + 1;

                if let Some(cap) = queue_re.captures(line) {
                    let name = cap.get(1).unwrap().as_str();
                    let base_id = format!("message-pub-{}", super::slugify(name));
                    let id = super::dedup_id(&base_id, &mut seen_ids);

                    surfaces.push(Surface {
                        id,
                        kind: SurfaceKind::MessagePub,
                        identifier: name.to_string(),
                        handler: format!("{rel}:{line_num}"),
                        touches: vec![rel.clone()],
                        declared_at: vec![format!("{rel}:{line_num}")],
                    });
                }

                if let Some(cap) = worker_re.captures(line) {
                    let name = cap.get(1).unwrap().as_str();
                    let raw_processor = cap.get(2).unwrap().as_str();
                    let (handler, touches) = resolve_processor(
                        raw_processor,
                        &content,
                        file,
                        input.source_root,
                        &rel,
                        line_num,
                    );

                    let base_id = format!("message-sub-{}", super::slugify(name));
                    let id = super::dedup_id(&base_id, &mut seen_ids);

                    surfaces.push(Surface {
                        id,
                        kind: SurfaceKind::MessageSub,
                        identifier: name.to_string(),
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

fn resolve_processor(
    raw: &str, content: &str, file: &std::path::Path, source_root: &std::path::Path, rel: &str,
    line: usize,
) -> (String, Vec<String>) {
    let clean =
        raw.trim_end_matches(|c: char| c == ')' || c == ';' || c == ',' || c.is_whitespace());
    if clean.contains("=>")
        || clean.starts_with("function")
        || clean.starts_with("async")
        || clean.starts_with('(')
    {
        return (format!("{rel}:{line}"), vec![rel.to_string()]);
    }
    let ident_re = Regex::new(r"^([a-zA-Z_$]\w*)$").expect("constant");
    if let Some(cap) = ident_re.captures(clean) {
        let name = cap.get(1).unwrap().as_str();
        return super::resolve_named_handler(name, content, file, source_root);
    }
    (format!("{rel}:{line}"), vec![rel.to_string()])
}
