# Parity fixtures (frozen regression baseline)

Ground-truth fixtures for the spec parser and merge engine in `specify_domain::spec` / `specify_domain::merge`. These were originally captured from the archived Python reference implementation; the Python script has since been retired. The fixtures are now a **frozen regression baseline** against the current Rust implementation.

Each case directory contains a subset of:

- `baseline.md` — the pre-merge baseline spec (may be empty/missing for new-baseline cases).
- `delta.md` — the delta spec to merge.
- `expected-merged.md` — the canonical merged output. The Rust merge engine in `specify_domain::merge` must reproduce this byte-for-byte.
- `expected-merge-errors.txt` — canonical stderr for merge failures. Empty file = success.
- `design.md` — optional design file for validation orphan-reference checks.
- `expected-validation.txt` — canonical stderr from `validate_baseline`. Empty file = all coherence checks passed.

These are checked in so Rust unit tests can compare byte-for-byte without invoking any external tool. A change to one of the cases must land alongside the corresponding source edit in `specify_domain::merge` / `specify_domain::validate` and a hand-crafted update to the `expected-*` file in the same commit.

The `case-10-design-refs` empty `expected-validation.txt` is a deliberate Python-era quirk (see the parity-quirk comment in `crates/domain/src/merge/validate.rs`); a correct orphan-reference check lives in `specify_domain::validate` (rule `cross.design-references-valid`).
