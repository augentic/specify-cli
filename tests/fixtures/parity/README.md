# Parity fixtures (frozen regression baseline)

Ground-truth fixtures for `specify-spec` (Change C) and `specify-merge` (Change D). These were originally captured from the archived Python reference implementation; the Python script has since been retired. The fixtures are now a **frozen regression baseline** against the current Rust implementation.

Each case directory contains a subset of:

- `baseline.md` — the pre-merge baseline spec (may be empty/missing for new-baseline cases).
- `delta.md` — the delta spec to merge.
- `expected-merged.md` — the canonical merged output. The Rust merge engine in `specify-merge` must reproduce this byte-for-byte.
- `expected-merge-errors.txt` — canonical stderr for merge failures. Empty file = success.
- `design.md` — optional design file for validation orphan-reference checks.
- `expected-validation.txt` — canonical stderr from `validate_baseline`. Empty file = all coherence checks passed.

These are checked in so Rust unit tests can compare byte-for-byte without invoking any external tool. A change to one of the cases must land alongside the corresponding source edit in `specify-merge` / `specify-validate` and a hand-crafted update to the `expected-*` file in the same commit.

## Known parity quirks preserved from the historical Python reference

- **case-10-design-refs:** The original `validate_baseline` compiled its requirement-ID pattern as `^REQ-[0-9]{3}$` **without** `re.MULTILINE`, so `ref_pattern.finditer(design_text)` never matched anything inside a multi-line design string. `expected-validation.txt` is therefore empty even though `design.md` contains `REQ-999` / `REQ-042` that are absent from `baseline.md`. The Rust port in `specify-merge::validate_baseline` reproduces this (flagged as a parity quirk in a code comment). A *correct* orphan-reference check lives in `specify-validate` (Change G, rule `cross.design-references-valid`) — that one uses a proper multiline/un-anchored regex.
