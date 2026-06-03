# Code & Skill Review — subtraction-biased, single pass

Scope: `specify-cli` (Rust workspace) + `specify` (plugin/skill repo). Pre-1.0; back-compat ignored.
All findings target `specify-cli` unless tagged `[specify]`. File paths are repo-relative to `specify-cli`.

## Summary

1. **Top three (sort key):** `S1` delete dead public API (`validate_plan_file`, `TargetRef::new`, `FromStr for TargetRef`, ~−39 LOC); `S5` collapse the duplicated merge/build journal lifecycle bracket (~−35 LOC); `S2` fold the triplicate `*.md` dir-walk into one helper (~−28 LOC).
2. **Total ΔLOC if all land:** ≈ **−146 LOC** (all subtraction; no positive-LOC defect findings).
3. **Primary non-LOC axes moved:** duplicate impls/branches collapsed (S2/S3/S5), one DTO-field plumbing block deduped (S4), two unused `impl` blocks removed (S1), one cross-module `use` removed via inlining (T1).
4. **Verified defects closed:** **none qualified.** `cargo clippy --workspace --all-targets -- -D warnings` → clean; `make lint` (`specify`) → 0 critical/important, 8 intentional `CORE-051` suggestions. Net ΔLOC from defect-only findings = **0** (well under the +30 cap). Defects still open: 0 reproducible. One borderline panic-surface (`source/preview.rs:90` invariant `expect`) reviewed and not promoted (see Not-a-finding).
5. **Most likely to break in remediation:** `S4` — collapsing the three `lint/eval.rs` `Diagnostic` builders via a struct-update base touches fingerprint-stamped wire output; a wrong default field flips `compute_fingerprint` and breaks golden lint fixtures.

## Reconnaissance (current-state numbers)

- `tokei`: Rust 552 files, **74,922 code** lines.
- `cargo clippy --workspace --all-targets -- -D warnings`: **pass**, `Finished dev profile ... in 11.85s`, zero warnings.
- `make lint` (`specify`): `Summary: 0 critical, 0 important, 8 suggestion, 0 optional` (all `CORE-051`, intentional `execution: agent` adapters).
- `rg -c '^#\[test\]' crates/ src/ tests/` → **1266** tests.
- `rg --files -g '**/mod.rs'` → 5 hits, **all under `tests/`** (convention-compliant; no finding).
- non-test `rg -c '\.(unwrap|expect)\('` → **1206** (dominated by `#[cfg(test)] mod` blocks inside `src/`; operator-path subset is tiny — see Not-a-finding).
- non-test `rg -c 'panic!|unreachable!'` → **84** (audited: handler-reachable hits are all in `#[cfg(test)]`).
- Files > 500 lines: 21 (largest non-test src: `slice/validate.rs` 1093, `adapter/core.rs` 744, `plugins.rs` 744, `journal.rs` 741).

---

## Structural findings

### S1 — Delete dead public API in `workflow` crate

- **Evidence:**
  - `rg -nw validate_plan_file crates/ src/` → **1 hit** (the definition, `crates/workflow/src/schema.rs:91`). Plan loading inlines `read_to_string` + `validate_plan_yaml` at `change/plan/core/io.rs`, so the wrapper has no caller.
  - `rg -n 'TargetRef::new' crates/ src/` → 2 hits, both inside its own doc (`model.rs:315`) and its own `debug_assert!` message (`model.rs:338`). No call site.
  - `rg -n 'parse::<TargetRef>|TargetRef::from_str' crates/ src/` → only a doc mention (`model.rs:413`); serde deserialization calls `TargetRef::parse` directly (`model.rs:408`).
- **Action:**
  1. Delete `validate_plan_file` (`crates/workflow/src/schema.rs:86-100`, doc + fn).
  2. Delete `TargetRef::new` (`crates/workflow/src/change/plan/core/model.rs:325-341`).
  3. Delete `impl FromStr for TargetRef` (`model.rs:392-398`) **and** the now-unused `use std::str::FromStr;` (`model.rs:9`).
  4. Trim the `TargetRef`/`new`/`FromStr` mentions in the struct doc (`model.rs:313-318`).
- **Quality delta:** `−39 LOC, −2 impl/fn, −1 use (module edge)`.
- **Net LOC:** ~74,922 → ~74,883 in touched files (pure deletion).
- **Done when:** `rg -nw 'validate_plan_file|TargetRef::new' crates/ src/` returns **0** and `cargo clippy --workspace -- -D warnings` stays green (proves the `FromStr` import drop was needed).
- **Rule?:** no.
- **Counter-argument:** "`TargetRef::new` is a documented infallible constructor for future in-process callers." Loses — pre-1.0, YAGNI; serde + `parse` already cover construction and the regex schema is the primary defence (per its own doc).
- **Depends on:** none.

### S5 — Collapse duplicated merge/build journal lifecycle bracket

- **Evidence:** `src/runtime/commands/slice/merge.rs:29-59` and `src/runtime/commands/slice/build.rs:157-187` are the same `started → match work { Ok => succeeded, Err(err) => failed { reason: err.variant_str() } }` shape with `SliceMerge*` vs `SliceBuild*` variants. The two best-effort emit helpers are identical bar one string literal:

```107:111:src/runtime/commands/slice/merge.rs
fn emit_merge_event(ctx: &Ctx, kind: EventKind) {
    let event = Event::new(Timestamp::now(), kind);
    if let Err(err) = journal::append_batch(ctx.layout(), std::slice::from_ref(&event)) {
        eprintln!("warning: slice.merge journal append: {err}");
    }
}
```

```329:333:src/runtime/commands/slice/build.rs
    let event = Event::new(Timestamp::now(), kind);
    if let Err(err) = journal::append_batch(ctx.layout(), std::slice::from_ref(&event)) {
        eprintln!("warning: slice.build journal append: {err}");
    }
}
```

- **Action:**
  1. Add one `journal::emit_best_effort(ctx, kind, scope: &str)` in `journal.rs` (it already owns `append_batch`); delete both `emit_*_event` fns and call it with `"slice.merge"` / `"slice.build"`.
  2. Add `fn bracket(ctx, started, work, failed)` taking the two `EventKind` constructors + closure; replace the two hand-written `match` brackets. Both `run`/`finalize` keep their bodies as the `work` closure.
- **Quality delta:** `−35 LOC, −1 duplicate fn, −2 duplicate match brackets`.
- **Net LOC:** the two files drop ~35 lines net (helper is ~10 lines; deleted duplication is ~45).
- **Done when:** `rg -n 'journal append: \{err\}' src/runtime/commands/slice/` returns **1** (was 2) and `rg -n 'SliceMergeFailed|SliceBuildFailed' src/runtime/commands/slice/` shows each constructed once.
- **Rule?:** no.
- **Counter-argument:** "AGENTS.md documents build as an intentional C5 mirror of merge." Loses on LOC/branch grounds — the contract is the *journal events emitted*, which the shared bracket preserves verbatim; only the copy-paste goes. (If the reviewer rejects the closure-taking `bracket` as too clever, the emit-helper half alone still lands ~−5 LOC as a clean tidy.)
- **Depends on:** none.

### S2 — Fold triplicate `*.md` directory walk into one helper

- **Evidence:** the identical readdir + `readdir-entry` + `.md`-extension scaffold appears 3×:
  - `crates/workflow/src/decisions.rs:94-107` (`read_baseline`)
  - `crates/workflow/src/decisions.rs:148-161` (`read_slice_records`)
  - `crates/workflow/src/slice/validate.rs:639-652` (`collect_decision_gates`)

```94:107:crates/workflow/src/decisions.rs
    for entry in std::fs::read_dir(decisions_dir).map_err(|source| Error::Filesystem {
        op: "readdir",
        path: decisions_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Filesystem {
            op: "readdir-entry",
            path: decisions_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
```

- **Action:** add `fn list_md_files(dir: &Path) -> Result<Vec<PathBuf>, Error>` in `decisions.rs` (sorted or not as each caller needs), then replace the three loop heads with `for path in list_md_files(dir)?`. `validate.rs` imports the one helper (one new cross-module `use`, replacing its own scaffold).
- **Quality delta:** `−28 LOC, −2 duplicate Error::Filesystem branch pairs; +1 module edge`. Trade justified: removes 6 duplicated `map_err` arms and a copy of the extension filter across 3 sites.
- **Net LOC:** ~−28 (helper ~12 lines; deleted scaffold ~40 lines).
- **Done when:** `rg -c 'op: "readdir-entry"' crates/workflow/src/` drops from **3** to **1**.
- **Rule?:** no.
- **Counter-argument:** "Three readdir loops aren't worth a shared helper." Loses — they are byte-identical and the helper is smaller than one copy; ripgrep/fd use exactly this `WalkDir`-style single entry point for directory iteration.
- **Depends on:** none.

### S3 — Centralize duplicate framework finding helpers

- **Evidence:** `rg -n -A3 'fn infrastructure_finding' crates/standards/src/framework/check/` shows **byte-identical** bodies in 3 files:

```311:313:crates/standards/src/framework/check/prose.rs
fn infrastructure_finding(rule_id: &'static str, error: ToolingError) -> Diagnostic {
    framework_finding(rule_id, error.to_string(), None)
}
```

(repeated verbatim at `skill_body.rs:673` and `skill_frontmatter.rs:431`). Likewise `fn finding(rule_id, message: String, path: Option<PathBuf>)` is identical at `agent_teams.rs:149`, `brief.rs:236`, `scenarios.rs:536`. `framework_finding` + `loc` already live in `crates/standards/src/framework/builder.rs`.

- **Action:** move the two one-liner wrappers (`infrastructure_finding`, the `Option<PathBuf>` form of `finding`) into `builder.rs` beside `framework_finding`; delete the 3 + 3 local copies; each call site already imports from `builder` so the `use` piggybacks.
- **Quality delta:** `−12 LOC, −4 duplicate fn impls`.
- **Net LOC:** ~−12 (delete 6 three-line fns ≈ 18; add 2 ≈ 6).
- **Done when:** `rg -c 'fn infrastructure_finding' crates/standards/` drops from **3** to **1**.
- **Rule?:** no.
- **Counter-argument:** "Local helpers keep each check module self-contained." Loses — they call into `builder` already; the deletion removes ≥2 duplicate impls per helper, which is exactly the bar for an extract.
- **Depends on:** none.

### S4 — Collapse three lint `Diagnostic` builders via struct-update base

- **Evidence:** `crates/standards/src/lint/eval.rs:460-489`, `497-525`, `556-593` (`make_finding`, `make_review_finding`, `make_synthetic_finding`) each spell out the same ~21-field `Diagnostic { … }` literal; the constant block (`related_rule_ids: None, source_adapter: None, slice: None, change: None, artifact: Artifact::Code, fingerprint: String::new(), status: None, disposition: None`) plus the 3-line `clamp_evidence + compute_fingerprint + finding` tail repeats verbatim 3×.
- **Action:** add `fn base(id_num) -> Diagnostic` returning the constant-field skeleton, and `fn finalize(mut f) -> Diagnostic { clamp_evidence(&mut f); f.fingerprint = compute_fingerprint(&f); f }`. Rewrite the three builders as `finalize(Diagnostic { source: …, kind: …, evidence: …, .. base(id_num) })`.
- **Quality delta:** `−12 LOC, −24 duplicated field assignments collapsed to one base`.
- **Net LOC:** ~−12.
- **Done when:** `rg -c 'fingerprint = compute_fingerprint' crates/standards/src/lint/eval.rs` drops from **3** to **1**.
- **Rule?:** no.
- **Counter-argument:** "Explicit literals are clearer than `..base()`." Loses on the LOC axis (taste is not an axis); struct-update base is idiomatic (cargo's `Config`/`Manifest` merge uses it). **Risk:** a wrong base default changes `compute_fingerprint`; gate on the existing lint golden fixtures.
- **Depends on:** none.

---

## One-touch tidies

### T1 — Inline single-call wrapper `model_schema_finding`

- **Evidence:** `rg -n model_schema_finding crates/ src/` → defined `crates/workflow/src/slice/validate.rs:208`, **one** call at `:188`.
- **Action:** inline the `model_drift(…)` call at `:188`; delete the helper (`:208-214`).
- **Quality delta:** `−7 LOC, −1 fn`.
- **Net LOC:** ~−7.
- **Done when:** `rg -c model_schema_finding crates/workflow/` → **0**.
- **Rule?:** no. **Counter-argument:** "Named helper documents intent." Loses — one call site, name adds no information `model_drift`'s args don't. **Depends on:** none.

### T2 — Delete test-only `TargetAdapter::effective_cache_mode`

- **Evidence:** `rg -n effective_cache_mode --glob '!**/tests*' crates/ src/` → production calls hit only `SourceAdapter::effective_cache_mode` (`source/op.rs:342`); the `TargetAdapter` method (`adapter/core.rs:474-476`) is referenced only by `adapter/core/tests.rs:181`. The shared free fn `effective_cache_mode` (`core.rs:423`) stays.
- **Action:** delete the `TargetAdapter` method (`core.rs:471-476`) and the one test assertion that calls it; trim the `TargetAdapter::effective_cache_mode` doc cross-refs.
- **Quality delta:** `−6 LOC, −1 method`.
- **Net LOC:** ~−6.
- **Done when:** `rg -c 'TargetAdapter::effective_cache_mode\b' crates/ src/` counts only doc-link survivors (no `fn`).
- **Rule?:** no. **Counter-argument:** "Symmetry with the source side." Loses — target dispatch never calls it; symmetry is not an axis. **Depends on:** none.

### T3 — Merge `serialise_request` / `serialise_model` into one generic

- **Evidence:** `src/runtime/commands/slice/build.rs:281-287` and `src/runtime/commands/slice/synthesize.rs:229-235` are identical bar the input type:

```281:285:src/runtime/commands/slice/build.rs
fn serialise_request(request: &BuildRequest) -> Result<String> {
    let mut content = serde_saphyr::to_string(request)?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
```

- **Action:** replace both with one `fn serialise_yaml<T: Serialize>(v: &T) -> Result<String>` (co-located or in `crates/model/src/atomic.rs` next to `yaml_write`); update the two call sites.
- **Quality delta:** `−4 LOC, −1 duplicate fn`.
- **Net LOC:** ~−4.
- **Done when:** `rg -c "ends_with\('\\\\n'\)" src/runtime/commands/slice/` drops from **2** to **0**.
- **Rule?:** no. **Counter-argument:** "Distinct names aid call sites." Loses — the bodies are type-parametric copies. **Depends on:** none.

### T4 — Drop test-only `SliceSourceBinding::is_bare`

- **Evidence:** `rg -n is_bare crates/ src/` → defined `model.rs:511`, referenced only in `model/tests.rs:231-252`.
- **Action:** delete `is_bare` (`model.rs:510-513`); the four asserting tests inline `binding.lead.is_none()`.
- **Quality delta:** `−4 LOC, −1 method`.
- **Net LOC:** roughly flat (the 4 test sites gain `.lead.is_none()` chars but stay one-liners); justified by removing dead production surface.
- **Done when:** `rg -c is_bare crates/ src/` counts only test-body call sites (no `fn`).
- **Rule?:** no. **Counter-argument:** "Cheap readability in tests." Loses — production method with zero production callers. **Depends on:** none. *Lowest-value tidy; drop if test edits net-positive LOC.*

---

## Not a finding (audited, deliberately excluded)

- **`source/preview.rs:90` `.expect("evidence_root: Some(..) => evidence_dir present")`** — an operator-path `expect`, but it is a guarded invariant established a few lines up (`evidence_root` is `Some` ⇒ `evidence_dir` is `Some`), not a reproducible panic. The smallest fix that removes it (thread an error or restructure the `Option` pair) costs > +8 LOC with no paired deletion → fails the defect LOC budget. Left open.
- **`make_finding`/`make_review_finding` shared tail only** — covered by S4; not split out.
- **`base64` 0.21 vs 0.22 duplicate** (`cargo tree --duplicates`) — transitive via `warg-*`/`oci-client`; `Cargo.toml` is frozen for the pass and these are not our direct deps.
- **`[specify]` skills** — `make lint` is green (0 critical/important); no body-cap, broken `refs/`, or description↔body drift. **No skill finding qualified.**
- **`mod.rs` files** — all 5 are under `tests/`, which the `<module>.rs + <module>/` convention explicitly permits.
