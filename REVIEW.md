# Code & Skill Review — single pass, quality-biased

**Top three by LOC removed**: (1) collapse `outcome.dry_run == Some(true)` ladder + the `Option<bool>` wrapper in `finalize::Outcome` and `PushBody` (≈ −11 LOC across 3 files, −1 type wrapper, −2 awkward branches); (2) fold the `[dry-run]` if/else writeln duplication in `render_finalize_outcome` to the `let prefix = …` form already used at `workspace.rs:263` (≈ −7 LOC, −1 branch); (3) inline `RuleView::summary`/`RuleView::full` shims into their callers via `RuleView::build(_, bool)` (≈ −5 LOC, −2 fns). **If all land**: ≈ −25 LOC across `crates/domain/src/change/finalize.rs`, `src/commands/change.rs`, `src/commands/workspace.rs`, `src/commands/codex.rs`, plus −1 type, −1 branch, −2 named fns, −1 wire-shape inconsistency. **Primary non-LOC axes moved**: types, idiom, branches. **Most likely to break in remediation**: S1 — the `dry_run: Option<bool>` field is asserted in `crates/domain/tests/finalize.rs` as `Some(true)` at two sites; remediation must update those alongside the type flip, and any out-of-tree consumer of the JSON envelope that reads `dry-run: false` (rather than treating an absent key as false) loses that key.

The codebase is already tight after the prior review pass; my findings are smaller individually than the previous round. Most are tidies.

---

## Structural findings

### S1. Collapse `Option<bool>` dry-run wrapper into `bool` in two wire types

- **Evidence**:
  - `crates/domain/src/change/finalize.rs:172` declares `pub dry_run: Option<bool>` with `#[serde(skip_serializing_if = "Option::is_none")]`. Constructed at `:266` as `dry_run: inputs.dry_run.then_some(true)`. The same boolean is `bool` on the `Inputs` side at `:187` — round-tripping a `bool` through `Option<bool>` is the only thing the `.then_some(true)` adapter does.
  - Two readers in `src/commands/change.rs` use `outcome.dry_run == Some(true)` (lines 181, 206), an awkward way to spell "the bool is true".
  - `src/commands/workspace.rs:252-260` carries the same pattern *plus* a parallel hand-rolled mirror: it has both `#[serde(skip)] dry_run_flag: bool` and `#[serde(skip_serializing_if = "Option::is_none")] dry_run: Option<bool>`, set together at `:138, :140` from the same `dry_run: bool` argument.

  Current state:

```168:172:crates/domain/src/change/finalize.rs
    /// Echo of the `--dry-run` flag. `Some(true)` only when the run
    /// was a dry-run; serialised omitted otherwise so real-run output
    /// stays minimal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
```

```252:260:src/commands/workspace.rs
struct PushBody {
    #[serde(skip)]
    plan_name: String,
    #[serde(skip)]
    dry_run_flag: bool,
    projects: Vec<PushItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}
```

  `rg 'outcome.dry_run == Some|dry_run\.then_some|dry_run_flag' src crates -t rust` returns 6 matches today; the previous pass collapsed `serde_helpers::is_false` to `std::ops::Not::not` for exactly this idiom (REVIEW.md S3).

- **Action**:
  1. In `crates/domain/src/change/finalize.rs`: change `pub dry_run: Option<bool>` → `pub dry_run: bool`; change `#[serde(skip_serializing_if = "Option::is_none")]` → `#[serde(skip_serializing_if = "std::ops::Not::not")]`; change the constructor at `:266` from `dry_run: inputs.dry_run.then_some(true)` to `dry_run: inputs.dry_run`. Tighten the doc comment from three lines to one.
  2. In `src/commands/change.rs`: change `outcome.dry_run == Some(true)` → `outcome.dry_run` at lines 181 and 206.
  3. In `src/commands/workspace.rs`: drop the `dry_run_flag: bool` field; rewrite `dry_run: Option<bool>` to `dry_run: bool` with `skip_serializing_if = "std::ops::Not::not"`; drop the `dry_run_flag: dry_run,` and `dry_run: dry_run.then_some(true)` adapter lines, replacing them with one `dry_run,` field-init shorthand; rename the two `body.dry_run_flag` reads in `write_push_text` to `body.dry_run`.
  4. In `crates/domain/tests/finalize.rs`: change the two `assert_eq!(outcome.dry_run, Some(true))` lines to `assert!(outcome.dry_run)`.
- **Quality delta**: −≈11 LOC across `crates/domain/src/change/finalize.rs`, `src/commands/change.rs`, `src/commands/workspace.rs`, `crates/domain/tests/finalize.rs`. −1 type wrapper (`Option<bool>` → `bool`). −2 `== Some(true)` branch comparisons. −1 `then_some(true)` adapter. −1 hand-rolled `dry_run_flag` mirror in `PushBody`. Same idiom (`skip_serializing_if = "std::ops::Not::not"`) the prior pass adopted for `ProjectConfig::hub`.
- **Net LOC**: `finalize.rs` 306 → ≈303; `change.rs` 309 → ≈307; `workspace.rs` 306 → ≈301.
- **Done when**: `rg 'dry_run.*Option<bool>|dry_run_flag|then_some\(true\)|dry_run == Some' src crates -t rust` → no matches; `cargo make ci` clean.
- **Rule?**: No.
- **Counter-argument**: "`Option<bool>` makes `null` distinguishable from `false` for downstream JSON consumers" — loses because (a) `skip_serializing_if = "std::ops::Not::not"` produces the *same* JSON shape (omitted when false, `"dry-run": true` when true) and (b) only `tests/change_finalize.rs:120` pins `dry-run` on the wire, and it asserts `value["dry-run"], true`, which both shapes satisfy. cargo, jj, and ripgrep all use plain `bool` with `skip_serializing_if` for echo-the-flag fields.
- **Depends on**: none.

---

### S2. Fold `[dry-run]` writeln duplication in `render_finalize_outcome`

- **Evidence**: `src/commands/change.rs:181-193` writes the same `"specify: change finalize — {} ({})"` string twice — once with a `[dry-run] ` prefix and once without — through full-form `if/else` arms. The same idiom is already done correctly in this codebase at `src/commands/workspace.rs:263`:

```262:264:src/commands/workspace.rs
fn write_push_text(w: &mut dyn Write, body: &PushBody) -> std::io::Result<()> {
    let prefix = if body.dry_run_flag { "[dry-run] " } else { "" };
    writeln!(w, "{prefix}specify: workspace push — {}", body.plan_name)?;
```

  Current state in `change.rs`:

```181:193:src/commands/change.rs
    if outcome.dry_run == Some(true) {
        writeln!(
            w,
            "[dry-run] specify: change finalize \u{2014} {} ({})",
            outcome.name, outcome.expected_branch
        )?;
    } else {
        writeln!(
            w,
            "specify: change finalize \u{2014} {} ({})",
            outcome.name, outcome.expected_branch
        )?;
    }
```

  `rg '\[dry-run\]' src -t rust` shows 2 sites in `src/commands/change.rs` and 1 in `src/commands/workspace.rs` — idiom drift between sibling files for the exact same prefix concept.

- **Action**: replace the if/else block with the `let prefix = …` form already used by `workspace.rs:263`. Combined with S1, the predicate becomes `outcome.dry_run` directly:

```rust
let prefix = if outcome.dry_run { "[dry-run] " } else { "" };
writeln!(
    w,
    "{prefix}specify: change finalize \u{2014} {} ({})",
    outcome.name, outcome.expected_branch
)?;
```

  Leave the second site (`:206`) alone — its else-branch carries extra `archived/cleaned` writes, so it's not a pure prefix swap.
- **Quality delta**: −≈7 LOC, −1 branch, idiom alignment with the only other `[dry-run]` text-prefix site in the binary.
- **Net LOC**: 309 → ≈302 (or ≈300 once S1 lands and the `== Some(true)` shrinks too).
- **Done when**: `rg -c '\[dry-run\] specify: change finalize' src/commands/change.rs` → `0` (the literal moves into the `{prefix}` template); `rg -c 'let prefix = if' src/commands/change.rs` → `1`; `cargo test --test change_finalize` clean.
- **Rule?**: No.
- **Counter-argument**: "the explicit if/else makes the dry-run case grep-able as a literal" — loses because the same workspace.rs site already chose the `let prefix` form for the same reason (one source of truth for the user-facing message), and grep on `\[dry-run\]` still finds the prefix literal in either form.
- **Depends on**: S1 makes the predicate simpler (`outcome.dry_run` instead of `outcome.dry_run == Some(true)`), but S2 lands cleanly even without S1.

---

## One-touch tidies

### T1. Inline `RuleView::summary` + `RuleView::full` into `RuleView::build(_, bool)`

- **Evidence**: `src/commands/codex.rs:181-203` defines three constructors where two are 3-line shims around the third:

```181:203:src/commands/codex.rs
impl<'a> RuleView<'a> {
    fn summary(resolved: &'a ResolvedCodexRule) -> Self {
        Self::build(resolved, false)
    }

    fn full(resolved: &'a ResolvedCodexRule) -> Self {
        Self::build(resolved, true)
    }
    fn build(resolved: &'a ResolvedCodexRule, with_body: bool) -> Self { ... }
}
```

  `rg 'RuleView::(summary|full|build)' src crates -t rust` shows 3 call sites: `RuleView::summary` once, `RuleView::full` twice, no direct `RuleView::build` calls. Each shim exists only to bake in a `bool`.

- **Action**: rename `build` → `new` (or keep `build`); delete `summary` and `full`; rewrite the three call sites in `list`, `show`, `export` as `RuleView::build(r, false)` / `RuleView::build(resolved, true)` / `|r| RuleView::build(r, true)`. Drop the field-private `// Filters on …` style comment on `summary`/`full`.
- **Quality delta**: −≈5 LOC (2 named-method shims gone), −2 fns, no new types.
- **Net LOC**: `codex.rs` 203 → ≈198.
- **Done when**: `rg 'fn (summary|full)\(' src/commands/codex.rs` → no matches.
- **Rule?**: No.
- **Counter-argument**: "named constructors document intent at the call site" — loses because one of the three call sites is already a closure (`.map(|r| RuleView::build(r, true))` is the same character count as `.map(RuleView::full)` plus four characters), and the bool-with-comment form `RuleView::build(r, /* with_body */ true)` is ripgrep / jj's preferred idiom for two-call-site discriminators.
- **Depends on**: none.

### T2. Drop dead `registry`/`slots` fields on `StatusBody::Absent`

- **Evidence**: `src/commands/workspace.rs:182-187` declares the variant with two fields that are *always* `None`:

```49:52:src/commands/workspace.rs
            StatusBody::Absent {
                registry: None,
                slots: None,
            }
```

```183:187:src/commands/workspace.rs
#[serde(untagged, rename_all = "kebab-case")]
enum StatusBody {
    Absent { registry: Option<Registry>, slots: Option<Vec<SlotStatus>> },
    Present { slots: Vec<SlotStatus> },
}
```

  `rg '"slots".*null|"registry".*null' tests -t rust` → no matches (no test pins the Absent JSON shape; `tests/workspace.rs:461` only reads `slots` on the Present path). The fields exist, get serialized as `{"registry": null, "slots": null}`, and are read by nothing.

- **Action**: change `StatusBody::Absent { registry: Option<Registry>, slots: Option<Vec<SlotStatus>> }` → `StatusBody::Absent {}`; drop the `registry: None, slots: None` field-inits at `:50-51`. The `untagged` enum still serializes correctly (Absent → `{}`, Present → `{"slots": [...]}`); the text writer's `StatusBody::Absent { .. } => writeln!(...)` arm at `:191` keeps working unchanged.
- **Quality delta**: −≈4 LOC, −2 dead fields, −1 wire-shape inconsistency (the no-registry case stops claiming a `slots` key alongside its true emptiness).
- **Net LOC**: `workspace.rs` 306 → ≈302 (or ≈297 with S1).
- **Done when**: `rg 'StatusBody::Absent \{ registry|slots: None' src/commands/workspace.rs` → no matches.
- **Rule?**: No.
- **Counter-argument**: "downstream JSON consumers may key on `registry == null` to detect absence" — loses because the `untagged` enum serializes Absent to `{}` either way once both fields are dead, and absence of a `slots` key is the canonical absence signal already used by `parse_stdout`-side assertions in `tests/workspace.rs`.
- **Depends on**: none.

---

## Dropped findings (and why)

- **`ErrorBody.hint_source: &'a Error` field in `src/output.rs`** — looked redundant (the field exists only so `write_error_text` can call `body.hint_source.hint()`), but `emit`'s closure signature is fixed at `FnOnce(&mut dyn Write, &T)` where `T = ErrorBody`. Inlining the writer as a closure that captures `err` saves ~3 LOC for the field/init but adds a 5-line closure body where the named fn used to be. Net delta is roughly 0 LOC; the lifetime parameter can't be dropped because `results: Option<&'a [ValidationSummary]>` already requires `'a`.
- **`Registry::select` slow-path `requested`/`matched` HashSet pair (`crates/domain/src/registry/catalog.rs:145-180`)** — visually duplicated, but `requested` is built from `selectors: &[String]` (which may have duplicates) while `matched` is built from `&Vec<&RegistryProject>` (deduplicated by registry uniqueness). Folding them risks the `selectors.len() != selected.len()` happy-path comparison breaking on duplicate selectors.
- **`SyncBody.synced` field redundancy with `registry.is_some()` (`src/commands/workspace.rs:165-180`)** — `tests/workspace.rs:417` pins `v["synced"], false` on the wire; the field can't be derived without a wire-shape change that requires test updates with no LOC win. The 4-field struct stays.
- **`PlanCounts` named per-status fields (`src/commands/status.rs:56-66`)** — `tests/change_plan_orchestrate.rs` pins each named key (`done`, `in-progress`, `pending`, `blocked`, `failed`, `skipped`, `total`); collapsing to a `BTreeMap<Status, usize>` would change wire shape. Stays.
- **`change/plan/lifecycle.rs:61-64` `(registry, registry_err)` tuple** — the bespoke shape captures both Ok-Some and Err-Some so the validate flow can layer registry-shape findings into `results`. Restructuring to a flat `match` doesn't shave lines and risks reordering the `registry-shape` finding's emit position.
- **Skill body cap drift** — top three skills are `omnia/skills/code-reviewer/SKILL.md` (185), `spec/skills/analyze/SKILL.md` (168), `spec/skills/extract/SKILL.md` (163). All under the 200-line cap; the prior pass already dropped this for the same reason and the situation hasn't changed.
- **`Error::Diag { code: ..., detail: format!(...) }` repetition (~100 sites)** — collapsing into an `Error::diag(code, detail) -> Error` constructor would be a 100-call-site refactor with ~0 net LOC delta (each site already fits in 4-5 lines and the helper form saves only characters, not lines). The "extract function" rule requires deletion of duplicate code, not duplicate text patterns.

---

## Post-mortem

- **S1**: predicted ≈−11 LOC, actual −6 LOC net (18 ins / 24 del across the four files); doc-comment trim was only −2 (vs implied −3) and the two `change.rs` reads were 1-line-for-1-line swaps. "Done when" rg pattern flipped clean (0 matches) and `cargo make ci` was green; no regression — the only on-the-wire pin (`tests/change_finalize.rs` asserting `value["dry-run"] == true`) still passes because `skip_serializing_if = "std::ops::Not::not"` produces the identical JSON shape.
- **S2**: predicted ≈−7 LOC, actual −7 LOC net (6 ins / 13 del in `src/commands/change.rs`); the prediction was exact because S1 had already simplified `outcome.dry_run` to a plain `bool`, so the prefix-fold was a pure 1-for-1 collapse. Both "done when" rg patterns flipped clean (0 matches for the duplicated literal, 1 for `let prefix = if`) and `cargo test --test change_finalize` was green (7/7); no regression.
- **T1**: predicted ≈−5 LOC, actual −8 LOC net (3 ins / 11 del in `src/commands/codex.rs`); the shim deletions came with a leading blank line each, so the impl block shed 8 lines while the three call sites were 1-for-1 swaps. "Done when" rg pattern flipped clean (0 matches for `fn (summary|full)\(`) and `cargo make ci` was green; no regression.
- **T2**: predicted ≈−4 LOC, actual +1 LOC net (6 ins / 5 del in `src/commands/workspace.rs`) — *worse than predicted*. The unmodelled cost was clippy 1.95's `empty_enum_variants_with_brackets` lint (plus `allow-attributes` / `allow-attributes-without-reason`) forcing a 4-line `#[expect(..., reason = "...")]` block to preserve the `Absent → {}` wire shape; remove that attribute and the delta would have been −3 LOC (close to the prediction). The bare-variant fix clippy *suggested* would have flipped the wire to `null`, which is exactly the regression the review's counter-argument rejected. "Done when" rg pattern flipped clean (0 matches for `StatusBody::Absent \{ registry|slots: None`) and `cargo make ci` was green; no test regression.
