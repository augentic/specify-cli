# Code & Skill Review — single pass, quality-biased

## Summary

1. **Top three by sort key:** S1 (collapse 3 hand-rolled frontmatter splitters in `lint/index` into one, −38 LOC) → T1 (delete byte-identical `frontmatter_block` copy in `scenarios.rs`) → T2 (drop the redundant `is_url_scheme`/dup helper noise). Subtraction-only pass; no defect tier qualified.
2. **Total ΔLOC if all land:** ≈ **−48 LOC**.
3. **Primary non-LOC axes moved:** hand-rolled→shared impls (−3 duplicate parser bodies), module-internal duplication.
4. **Top three verified defects closed:** **none qualified.** `cargo make check` (fmt + clippy `-D warnings` + nextest + doctests) exits **0**, and `make lint` (specify) exits **0** with 0 critical / 0 important (8 `CORE-051` *suggestions* only). No reachable `unwrap`/`expect`/`panic!` on a CLI-handler path was found — every hit under `src/runtime/**` outside `#[cfg(test)]` is a guarded invariant (`preview.rs:90` is `Some(out_dir)`-guarded) or lives in a `mod tests`. Net defect-only ΔLOC: **0** (≤ +30 ✓).
5. **Most likely to break in remediation:** S1 — the three splitters have *subtly* different return contracts (block-before vs body-after vs CRLF handling); collapsing them must preserve each call site's half of the tuple exactly, or skill/brief body-line counts shift.

**Context numbers (current state):**

- `tokei`: 431 Rust files, 70,454 code lines.
- `cargo make check`: **pass** (288s, exit 0).
- `make lint` (specify): **pass**, `Summary: 0 critical, 0 important, 8 suggestion, 0 optional`.
- Panic-adjacent surface: `rg -c '\.(unwrap|expect)\(' --glob '!**/tests/**' crates/ src/` → 1102; `panic!|unreachable!` → 87. Spot-audit of `src/runtime/**` (the operator path) found 0 reachable non-test panics.
- Files > 500 lines: 27 (largest `crates/workflow/src/journal.rs` 1330 — a closed wire-contract enum, deliberately *not* a subtraction target).

---

## Structural findings

### S1 — Collapse three frontmatter splitters in `lint/index` into one

**Evidence.** Three byte-walking front-matter splitters live in `crates/standards/src/lint/index/`, all running the same `strip_prefix("---\n") … find("\n---")` loop:

```bash
$ rg -n 'fn split\(|fn strip_frontmatter' crates/standards/src/lint/index/
frontmatter.rs:53:fn split(content: &str) -> Option<&str>      # returns the block (before \n---)
skill.rs:66:fn strip_frontmatter(text: &str) -> &str           # returns the body (after \n---)
brief.rs:72:fn strip_frontmatter(text: &str) -> &str           # byte-identical to skill.rs:66
```

`skill.rs:66-87` and `brief.rs:72-93` are **byte-identical** 22-line functions; `frontmatter.rs:53-66` is the same loop returning the complementary half.

**Action.**

1. In `crates/standards/src/lint/index/frontmatter.rs`, widen the private `split` to return both halves:

```rust
fn split(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let mut from = 0;
    while let Some(rel) = rest[from..].find("\n---") {
        let pos = from + rel;
        let tail = &rest[pos + 4..];
        if tail.is_empty() { return Some((&rest[..pos], "")); }
        if let Some(b) = tail.strip_prefix('\n').or_else(|| tail.strip_prefix("\r\n")) {
            return Some((&rest[..pos], b));
        }
        from = pos + 4;
    }
    None
}
```

2. In the same file's `extract`, change `let frontmatter_body = split(&text)?;` → `let (frontmatter_body, _) = split(&text)?;`.
3. Make `split` reachable from siblings: `pub(super) fn split`.
4. Delete `strip_frontmatter` from both `skill.rs` and `brief.rs`; replace each call (`strip_frontmatter(&text)`) with `frontmatter::split(&text).map_or(text.as_str(), |(_, body)| body)`. `skill.rs` already `use super::frontmatter;`; add the same `use` to `brief.rs`.

**Quality delta.** `−38 LOC, −2 duplicate impls, −1 hand-rolled-vs-shared`.

**Net LOC.** `frontmatter.rs 14 + skill.rs 22 + brief.rs 22 = 58` → `~20` (one widened fn + two call expressions). Precedent: `rules/parse.rs::split_frontmatter` already returns the `(front, body)` tuple — this makes the index layer agree with it.

**Done when.** `rg -c 'fn strip_frontmatter' crates/standards/src/lint/index/` flips from `2` to `0`, and `cargo make check` stays green.

**Rule?** No — 2 in-crate copies, below the >3× bar; a clippy lint cannot express it.

**Counter-argument.** "Three tiny functions are cheaper to read in place than one shared one." Loses: they are not *similar*, they are the *same loop*; a future edit to the `\r\n` handling already has to be applied three times, which is exactly the drift this removes.

**Depends on.** none.

---

## One-touch tidies

### T1 — Delete the duplicate `frontmatter_block` in `scenarios.rs`

**Evidence.**

```bash
$ rg -n 'fn frontmatter_block' crates/standards/src
framework/helpers.rs:138:fn frontmatter_block(content: &str) -> Option<&str>
framework/check/scenarios.rs:528:fn frontmatter_block(content: &str) -> Option<&str>
```

`scenarios.rs:528-532` is byte-identical to `helpers.rs:138-142`, and `scenarios.rs` already sits under `framework/check/` (a child of `framework`).

**Action.** Change `helpers.rs:138` `fn frontmatter_block` → `pub(crate) fn frontmatter_block`; delete `scenarios.rs:528-532`; update `scenarios.rs` call sites to `crate::framework::helpers::frontmatter_block(...)` (or a local `use`).

**Quality delta.** `−5 LOC, −1 duplicate impl`.

**Net LOC.** `5 → 0` in `scenarios.rs` (+1 visibility word in `helpers.rs`).

**Done when.** `rg -c 'fn frontmatter_block' crates/standards/src` flips from `2` to `1`; `cargo make check` green.

**Rule?** No.

**Counter-argument.** "`helpers.rs` becomes a dumping ground." Loses: `helpers.rs` is already the framework-check helper module (`under_symlink`, etc.); this is its job.

**Depends on.** none.

### T2 — `body_after_frontmatter` (scenarios.rs) can reuse the same split

**Evidence.** `crates/standards/src/framework/check/scenarios.rs:538-547` re-walks the `---\n … \n---` block a *second* time (after `frontmatter_block` already walked it) purely to return the body.

**Action.** Once T1 exposes a shared splitter, fold `body_after_frontmatter` into a single `helpers::frontmatter_split(content) -> Option<(&str, &str)>` call and take `.1`; delete the standalone 10-line `body_after_frontmatter`. If T1 is not taken, leave this — it is not worth a bespoke helper on its own.

**Quality delta.** `−9 LOC, −1 duplicate scan`.

**Net LOC.** `10 → ~1`.

**Done when.** `rg -c 'fn body_after_frontmatter' crates/standards/src` flips from `1` to `0`; `cargo make check` green.

**Rule?** No.

**Counter-argument.** "Two reads of a small string is negligible." Loses on LOC, not on perf — the function body is the cost, not the scan.

**Depends on.** T1.

---

## Considered and dropped (transparency)

- **Collapse the twin `Finding` + `into_diagnostic` mirror types** (`crates/model/src/spec/provenance.rs:123` and `crates/model/src/decision.rs:103`, both in `specify-model`). Dropped: they differ (`provenance` carries `span`/`Artifact::Specs`, `decision` carries neither/`Artifact::Decisions`), so unifying them adds an `Option<Span>` + `artifact` param to **~5 `decision.rs` call sites** — net LOC ≈ flat with *increased* call-site burden. Fails "burden of proof rises sharply when LOC goes up." (Also: `decision.rs` is uncommitted RFC-37 in-progress work.)
- **Full cross-crate frontmatter consolidation** (~7 splitters across `lint/`, `framework/`, `rules/parse.rs`, plus 3 in `tests/`). Dropped: spans the `framework`↔`lint` module boundary with distinct return contracts; a single shared util would need a new home (violates "no new modules") or would couple unrelated submodules. S1 + T1 capture the safe, in-tree subset.
- **`journal.rs` (1330 lines)** — the per-event enum is a closed wire taxonomy governed by `DECISIONS.md §journal event taxonomy`; no duplicated id-mapping exists (`rg 'fn id\(|fn wire_id'` → none). Not a subtraction target; touching it is wire-contract risk for zero axis gain.
- **Defect tier** — no `make lint` skill-predicate failure, no clippy violation, no reachable operator panic. Nothing to close.

---

## Post-mortem

- **S1:** actual **−37 LOC** (19 ins / 56 del across the 3 files) vs predicted −38; done-when flipped cleanly (`rg -c 'fn strip_frontmatter' crates/standards/src/lint/index/` → 2→0); no regression — `cargo make check` exit 0 (fmt + clippy `-D warnings` + nextest + doctests). `pub(super)` sufficed; kept the `"\n---".len()` named-length idiom over the literal `4`; body half byte-identical so skill/brief `body_line_count` unchanged.
- **T1:** actual **−6 LOC** (2 ins / 8 del) vs predicted −5 (extra line was the orphaned blank separator at the deleted fn); done-when flipped cleanly (`rg -c 'fn frontmatter_block' crates/standards/src` → 2→1, sole survivor in `helpers.rs`); no regression — `cargo make check` exit 0, no fixes needed. `pub(crate)` visibility; `frontmatter_block` folded into the existing `use crate::framework::helpers::{…}`.
- **T2:** actual **≈−4 LOC** (T2-only hunks; scenarios.rs ≈−9, helpers.rs +5 for the new `frontmatter_split` + delegate) vs predicted −9 — gap is because the recommended shared-splitter approach *adds* a helper rather than pure deletion (prediction counted only the scenarios.rs side); done-when flipped cleanly (`rg -c 'fn body_after_frontmatter' crates/standards/src` → 1→0); no regression — `cargo make check` exit 0. Body now a borrowed `&str` (no `String` alloc); `opted` invariant guarantees `frontmatter_split` is `Some` so the `else { continue }` fallback is unreachable, byte-identical to the old fallback behaviour.
- **All three (final):** `cargo make ci` green (249s) — fmt + clippy `-D warnings` + nextest + doctests + doc + vet + outdated + deny all pass. Combined working-tree delta ≈ **−47 LOC** (S1 −37, T1 −6, T2-only ≈−4), in line with the predicted ≈−48; no defect tier was opened and nothing regressed.
