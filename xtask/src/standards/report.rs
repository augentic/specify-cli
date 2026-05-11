//! Per-run failure list, totals, and human-readable report.

use std::collections::BTreeMap;

use super::allowlist::ALLOWLIST;
use super::types::{Counts, FileBaseline};

#[derive(Default)]
pub(super) struct Report {
    pub(super) passed: bool,
    failures: Vec<String>,
    totals: BTreeMap<&'static str, u32>,
}

impl Report {
    pub(super) fn merge(&mut self, rel: &str, counts: &Counts, baseline: &FileBaseline) {
        if self.failures.is_empty() {
            self.passed = true;
        }
        for (key, value) in counts.iter() {
            // module-line-count contributes to totals only as an
            // overflow indicator, not a sum (LoC totals would dwarf
            // every other predicate).
            if key != "module-line-count" {
                *self.totals.entry(key).or_insert(0) += value;
            }
            let cap = baseline.cap(key);
            if value > cap {
                self.passed = false;
                self.failures.push(format!("  FAIL {rel}: {key} {value} > baseline {cap}"));
            }
        }
    }

    pub(super) fn print(&self) {
        for line in &self.failures {
            println!("{line}");
        }
        println!();
        println!("standards-check totals:");
        for (key, value) in &self.totals {
            println!("  {key}: {value}");
        }
        if self.passed {
            println!("\nstandards-check: PASS");
        } else {
            println!(
                "\nstandards-check: FAIL — reduce the offending counts or, if a hit is justified, raise the per-file baseline in {ALLOWLIST} in the same PR."
            );
        }
    }
}
