# Code & Skill Review — `specify` + `specify-cli`

Single-pass, quality-biased review. Pre-1.0 — back-compat / migrations / deprecations are ignored.

## Summary

Top three by LOC: **(F1)** delete `TransitionTarget` clap mirror enum (~30 LOC), **(F2)** collapse `PushItem` mirror DTO into domain `PushResult` (~25 LOC), **(F4)** dedupe identical `PlanCounts` / `Counts` structs and their construction (~23 LOC). Total ΔLOC across all five structural findings: roughly **−115 LOC**. Primary non-LOC axes moved: types (−5), error variants (−3), module edges (−3). Most likely to break in remediation: **F2** — wire field rename `pr_number` → `pr` on the workspace-push envelope; needs `#[serde(rename = "pr")]` to keep skills' JSON parsers stable, otherwise the rename leaks.

Reconnaissance (numbers quoted in findings):

- tokei Rust: 47,898 lines / 269 files. Skill markdown: 28 files, max 168 LOC (`change-analyze`).
- Files > 500 LOC: 14, all but two are tests/test-fixtures (only `crates/tool/src/package.rs` 504 LOC and `crates/domain/src/change/plan/doctor/tests.rs` 549 LOC are non-suite).
- `mod.rs` count: 3 (test-only `tests/common/mod.rs` × 3); compliant with the "no `mod.rs` outside tests" rule in `coding-standards.md`.
- `cargo tree --duplicates`: only transitive (`base64`, `bitflags`, `rustix` — all in the `wasm-pkg-client` / `warg-*` subtree). Nothing actionable in our `Cargo.toml`s.

---

## Structural findings

### F1 — Delete `TransitionTarget` clap mirror

**Evidence**: `src/commands/slice/cli.rs:257-287` declares `TransitionTarget` (19 LOC enum + 11 LOC `From<TransitionTarget> for LifecycleStatus`), used at one call site `src/commands/slice.rs:79` (`target.into()`). `LifecycleStatus` already derives `clap::ValueEnum` (`crates/domain/src/slice/lifecycle.rs:18`). The wrapper exists exclusively to omit the `Merged` variant from clap's accepted set; cf. the doc comment line 257-261 "Mirrors [`LifecycleStatus`] minus `Merged`".

`rg -c TransitionTarget --type rust` returns **1** file (`src/commands/slice/cli.rs`).

**Action**:

1. Delete `pub enum TransitionTarget { … }` and `impl From<TransitionTarget> for LifecycleStatus { … }` in `src/commands/slice/cli.rs:257-287`.
2. In `SliceAction::Transition`, change `target: TransitionTarget` → `target: LifecycleStatus`.
3. In `src/commands/slice.rs:79`, replace `lifecycle::transition(ctx, name, target.into())` with a guard:

```rust
SliceAction::Transition { name, target } => {
    if matches!(target, LifecycleStatus::Merged) {
        return Err(Error::Argument {
            flag: "<target>",
            detail: "use `specify slice merge run` to reach `merged`".to_string(),
        });
    }
    lifecycle::transition(ctx, name, target)
}
```

`Error::Argument` keeps the exit-2 contract that clap currently surfaces.

**Quality delta**: −22 net LOC, −1 type, −1 module edge (no more cli→domain `From` impl).
**Net LOC**: 30 → 8.
**Done when**: `rg -c '\bTransitionTarget\b' --type rust` returns nothing (currently `src/commands/slice/cli.rs:1`).
**Rule?**: no — single occurrence.
**Counter-argument**: "Compile-time guarantee that no path can transition slices to `Merged`." Loses because clap doesn't give compile-time guarantees against operator input anyway — it's a runtime parse step either way; the mirror enum just moves the rejection from our error site to clap's parse-error site.
**Depends on**: none.

---

### F2 — Collapse `PushItem` into domain `PushResult`

**Evidence**: `src/commands/workspace.rs:289-300` declares `PushItem`, byte-for-byte equivalent to `PushResult` in `crates/domain/src/registry/workspace/push.rs:43-56` apart from field name `pr_number → pr`. `src/commands/workspace.rs:121-130` is a 10-line `.iter().map(...).collect()` whose entire body is `name: r.name.clone()`, `status: r.status`, etc. `PushResult` lacks `#[derive(Serialize)]`.

```rust
// src/commands/workspace.rs:125-130 (current)
.map(|r| PushItem {
    name: r.name.clone(),
    status: r.status,
    branch: r.branch.clone(),
    pr: r.pr_number,
    error: r.error.clone(),
})
```

**Action**:

1. In `crates/domain/src/registry/workspace/push.rs`, add `serde::Serialize` to the derive on `PushResult` and tag `pr_number` with `#[serde(rename = "pr", skip_serializing_if = "Option::is_none")]`; mark `branch` / `error` `skip_serializing_if = "Option::is_none"` to match the wire shape.
2. Delete `struct PushItem` and `struct PushBody`'s `projects: Vec<PushItem>` allocation in `src/commands/workspace.rs`. Use `projects: Vec<PushResult>` directly (or `&'a [PushResult]` borrowed).
3. Delete the mapping loop at lines 121-130; pass `results` straight through.

**Quality delta**: −23 net LOC, −1 type (DTO mirror), −1 branch (`.iter().map()` removed).
**Net LOC**: 25 → ~2 (the rename attribute).
**Done when**: `rg -c 'struct PushItem' src/commands/workspace.rs` returns nothing (currently `1`).
**Rule?**: no — single occurrence, but the same anti-pattern shows up below.
**Counter-argument**: "Domain types shouldn't derive `Serialize` for the wire." Loses because `PushOutcome` already does (line 23-27 of the same file), and `MergeOperation`, `MergePreviewEntry`, `BaselineConflict`, `finalize::Outcome`, `SlotStatus`, `Created`, etc. all derive `Serialize` and are emitted directly. The CLI already trusts the domain crate to own the wire shape; `PushItem` is the outlier.
**Depends on**: none.

---

### F3 — Collapse boutique `Error` variants into `Diag`

**Evidence**: Three single-or-double-call-site typed variants exist purely as ceremony around `Diag { code, detail }`:

- `Error::PlanTransition { from, to }` (`crates/error/src/error.rs:64-70`) — produced once at `change/plan/core/transitions.rs:43`, asserted by 4 test cases (`transitions.rs:175,203,293`); consumer (`output.rs` → `ErrorBody`) only emits `error/message/exit_code` — `from`/`to` never reach the wire (string-formatted into `message`).
- `Error::DriverBusy { pid }` (`error.rs:74-78`) — produced once at `change/plan/lock.rs:124`, asserted once (`lock/tests.rs:54`). `pid` likewise never reaches wire.
- `Error::SliceNotFound { name }` (`error.rs:90-94`) — 4 producers in 2 files (`slice/outcome.rs`, `slice/journal.rs`); detail message rebuildable from `Diag`.

The sibling slice lifecycle already uses `Error::Diag { code: "lifecycle", detail }` for the same shape (`crates/domain/src/slice/lifecycle.rs:73-82`) — the asymmetry is the finding.

`rg 'Error::PlanTransition\b' --type rust` returns **1 prod + 5 test sites**; `Error::DriverBusy` returns **1 prod + 1 test**; `Error::SliceNotFound` returns **4 prod + 0 test**.

**Action**:

1. In `crates/error/src/error.rs`, delete the `PlanTransition`, `DriverBusy`, `SliceNotFound` variants and their three arms in `variant_str()` (lines 64-94 + 184, 185, 187). Variant kebab strings preserve via the `Diag.code` field.
2. Replace producers:
   - `transitions.rs:43-46` → `Err(Error::Diag { code: "plan-transition", detail: format!("cannot transition from {self:?} to {target:?}") })`.
   - `lock.rs:124` → `Err(Error::Diag { code: "driver-busy", detail: format!("another /change:execute driver is running (pid {pid}); refusing to proceed") })`.
   - 4 `Err(Error::SliceNotFound { name })` → `Err(Error::Diag { code: "slice-not-found", detail: format!("slice '{name}' not found") })`.
3. Update test asserts to match `Error::Diag { code, detail }` instead of the typed pattern (replace `match err { Error::PlanTransition { from, to } => …` with `match err { Error::Diag { code: "plan-transition", detail } => …`).

Wire-visible JSON (`error: "plan-transition"`, `"driver-busy"`, `"slice-not-found"`) stays byte-identical because the kebab string was already the `Cow::Borrowed` returned by `variant_str()`. Exit codes also stay the same — `Exit::from(&Error)` only special-cases `CliTooOld`/`Validation`/`Argument`; everything else is `GenericFailure` either way.

**Quality delta**: −25 net LOC, −3 enum variants, −3 branches in `variant_str`.
**Net LOC**: ~30 → ~5.
**Done when**: `rg -c 'PlanTransition|DriverBusy|SliceNotFound' crates/error/src/error.rs` returns nothing (currently `7`).
**Rule?**: no — local pattern, enforcing it would mean banning typed error variants which is taste-driven. But the criterion in `coding-standards.md:185` ("dedicated typed variant remains correct…") should be tightened: typed variants must carry data the *wire* exposes; `pid`, `from`, `to`, and `name` here all collapse into `detail`.
**Counter-argument**: "Typed variants give exhaustive `match`-driven tests." Loses because the existing tests already pivot on the `code: &str` literal (the only stable wire identifier), so they're equivalently strong against `Diag { code, .. }`; jj/ripgrep both use a single `Error::Other(String)`-shaped escape hatch for exactly this category.
**Depends on**: none.

---

### F4 — Dedupe `Counts` / `PlanCounts` structs and constructors

**Evidence**: `src/commands/plan/status.rs:35-43` and `src/commands/status.rs:56-66` declare byte-identical structs (just renamed: 7 numeric fields, same order, same kebab-case rename, same derives). Both call sites build the same way:

```rust
// src/commands/plan/status.rs:142-146 (also at src/commands/status.rs:75-79)
let mut counts: BTreeMap<Status, usize> = Status::ALL.iter().map(|&s| (s, 0)).collect();
for entry in &plan.entries {
    *counts.get_mut(&entry.status).expect("ALL covers status") += 1;
}
let total: usize = counts.values().sum();
```

Identical loop appears verbatim at `src/commands/status.rs:75-79`, followed by the same 7-field projection (lines 84-91 vs 175-183).

`rg -c 'Status::ALL.iter\(\).map\(\|&s\| \(s, 0\)\).collect\(\)' src/` returns **2**.

**Action**:

1. Move `struct Counts` to `src/commands/plan/status.rs` (already there) and make it `pub(crate)`.
2. Add `pub(crate) fn from_entries(entries: &[Entry]) -> Counts` in the same file: it builds the `BTreeMap`, projects into the struct, returns. Both call sites call it.
3. In `src/commands/status.rs`, delete `struct PlanCounts` (9 LOC) and the 17-line construction; replace `counts: PlanCounts { … }` with `counts: super::plan::status::Counts::from_entries(&plan.entries)`. Update the dashboard text renderer (line 121-131) to read `p.counts.done` / `p.counts.in_progress` etc. — the field names are unchanged.

**Quality delta**: −23 net LOC, −1 type, −1 duplicate algorithm.
**Net LOC**: ~50 → ~27 (struct kept once + helper fn ~10 LOC).
**Done when**: `rg -c 'Status::ALL.iter\(\).map\(\|&s\| \(s, 0\)\)' src/` returns `1` (currently `2`).
**Rule?**: no — local duplication.
**Counter-argument**: "`status` and `plan status` are different commands and should ship independent shapes." Loses because the JSON envelope is already identical key-for-key, and any future divergence is one struct away — ripgrep does the same with `stats::Stats` shared across `summary`/`per-file` commands.
**Depends on**: none.

---

### F5 — Inline single-caller `serde_path_display` adapter into the two `PathBuf` fields

**Evidence**: `crates/error/src/serde_path_display.rs` is a 27-line module (12 lines doc + 1 `#[expect]` attribute + 5 LOC of code) wired via `#[serde(serialize_with = "specify_error::serde_path_display::serialize")]` at exactly two sites (`crates/domain/src/slice/actions/create.rs:35` and `crates/domain/src/merge/slice.rs:50`). The "adapter" body is one line: `serializer.collect_str(&value.display())`.

`rg --type rust -l 'serde_path_display::'` returns **2 callers**.

**Action**: Replace each `pub dir: PathBuf` / `pub baseline_path: PathBuf` field with a sibling `#[serde(skip)] pub baseline_path: PathBuf` plus a `#[serde(serialize_with = "Path::display")]`-style inline closure — except serde doesn't accept inline closures. Cleaner: keep the `PathBuf` field, drop the adapter file, and add a `#[serde(getter = "<lambda>")]` is also unavailable. So the smallest move is: drop the module and replace each annotation with the local one-liner via a 2-line per-struct private fn:

```rust
#[serde(serialize_with = "ser_dir")]
pub dir: PathBuf,
// elsewhere in the file:
fn ser_dir<S: Serializer>(v: &PathBuf, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&v.display())
}
```

Each call site adds 3 LOC; the shared module deletes 27. Net is still −21 LOC, plus −1 module edge (no more `specify_error::serde_path_display::` reach-across).

**Quality delta**: −21 net LOC, −1 module edge.
**Net LOC**: 27 → 6 (3 LOC × 2 sites).
**Done when**: `wc -l crates/error/src/serde_path_display.rs` errors (file deleted).
**Rule?**: no — two callers.
**Counter-argument**: "Centralised so a future third caller doesn't reinvent it." Loses because the third caller isn't on the horizon (zero new `PathBuf`-with-display-serialize fields landed in the last six commits per `git log -- crates/error/src/serde_path_display.rs`), and YAGNI says you delete the helper today and resurrect it the day caller #3 appears.
**Depends on**: none.

---

## One-touch tidies

### T1 — Inline `change_entry_json` 1-liner

`src/commands/plan.rs:94-96` is `serde_json::to_value(entry).expect("plan Entry serialises as JSON")`, called twice (`plan/create.rs:127, 166`). Inline both, drop the helper and the `change_entry_json` symbol from `pub use … change_entry_json …` at line 14. Δ: −4 LOC, −1 module edge. **Done when**: `rg change_entry_json src/` empty.

### T2 — `Error::variant_str` returns `String`, drops `Cow`

`crates/error/src/error.rs:177-193` returns `Cow<'static, str>` only because `Self::Filesystem { op, .. } => Cow::Owned(format!("filesystem-{op}"))`. The single caller `src/output.rs:142` immediately `.to_string()`s. Switch return to `String`, replace 11 `Cow::Borrowed("…")` with `"…".to_string()` (or `.into()`), drop `use std::borrow::Cow;`, drop `.to_string()` at the call site. Δ: −1 LOC + −1 import, −1 type-parameter on the public signature. **Done when**: `rg 'Cow' crates/error/src/error.rs` returns nothing (currently 14 hits).

### T3 — `EntryAction` enum → `&'static str` field

`src/commands/plan/create.rs:194-202` is a 9-line enum used as a wire discriminator (`"create"` / `"amend"`); both producers pick the value statically (`EntryAction::Create` at line 126, `Amend` at line 165) and the renderer dispatches on it (lines 207-208). Replace `action: EntryAction` with `action: &'static str` + tag the field `#[serde(serialize_with = …)]` is unnecessary because `&str` already serialises; both producers pass `"create"` / `"amend"` literally. Δ: −9 LOC, −1 type. **Done when**: `rg 'enum EntryAction' src/` empty.

### T4 — `Status::ALL` → `<Status as ValueEnum>::value_variants()`

`crates/domain/src/change/plan/core/transitions.rs:12-13` declares a `pub const ALL: [Self; 6] = [...]` next to the same enum that already derives `clap::ValueEnum` (`change/plan/core/model.rs:14-26`). Five callers (transitions tests + two status handlers) iterate it. Replace `&Status::ALL` with `Status::value_variants()` (clap exposes it). Δ: −4 LOC + 1 idiom. **Done when**: `rg 'Status::ALL' crates/ src/` empty (currently 5 hits in production code).

Same pattern for `LifecycleStatus::ALL_STATUSES` in `crates/domain/src/slice/lifecycle.rs:91-98` — fold into T4.

### T5 — `change-analyze` SKILL collapses §"Critical Path" / §"Process" duplication

`plugins/change/skills/analyze/SKILL.md:9-17` (Critical Path 1-7) and lines 142-149 (Process 1-4) cover the same workflow; the body of §Process re-states the same 4a/4b output split that §Critical Path 4 + 6 already imply. Delete §Process (lines 142-150). Δ: −10 LOC; tightens largest skill (168 → 158 LOC) below the median for the directory. **Done when**: `wc -l plugins/change/skills/analyze/SKILL.md` ≤ 158.

---

## Items considered and dropped

- **Test split `*_text` / `*_json` pairs in `tests/plan_orchestrate.rs` and friends.** Rule "extract function only if ≥ 2 call sites delete duplicate code as a result" is met (literally 50+ pairs), but each pair only shares the `specify().current_dir(...).args(...)` boilerplate, ~3 LOC; combining them into one test that runs both formats either (a) doubles assertion bodies or (b) loses per-format failure clarity. Net trade is roughly LOC-neutral.
- **`commands.rs` `dispatch` vs `scoped` collapse.** Two callers of `dispatch` (`init`, `capability::resolve`) genuinely don't want `Ctx::load`. Collapsing forces them through and re-introduces the "no `.specify/`" failure that `init` is supposed to *create*. Drop.
- **`auto_commit` workspace shell-out (`src/commands/slice/merge.rs:262-307`).** The closure-with-`warn` shape exists because the merge must not fail on git issues; rewriting via `?` would change behaviour. Drop.
- **Cargo `--duplicates`.** Only transitive (`base64 v0.21/v0.22`, `bitflags v2/v1`, `rustix v0.38/v1`) all coming through `wasm-pkg-client` / `warg-*`. No first-party fix.
- **`docs/standards/*.md` size (823 LOC across 5 files in CLI repo).** Per scope rules, doc-only changes are out. Plus they're under the agent-navigation budget (`AGENTS.md` says "three hops").
- **`xtask/src/manpage.rs` (44 LOC).** Genuinely useful, no duplication, leave alone.

---

## Post-mortem

- **F1** — applied. Predicted −22 net LOC in prod, actual −21 (16 +, 37 −). Done-when (`rg -c '\bTransitionTarget\b' --type rust` → 0) flipped cleanly. No regressions: 813/813 nextest tests pass. One follow-on the review under-counted: `tests/slice.rs::transition_rejects_merged_target` was asserting clap's "invalid value" + the legal-targets list, so it had to be rewritten to assert `error: "argument"`, exit 2, and that the message names `merged` + redirects to `specify slice merge run` (net −1 LOC in tests). `cargo make ci` blocked at the `vet` step on a DNS lookup failure (no network for `raw.githubusercontent.com/divviup/libprio-rs/...`); unrelated to this finding.
