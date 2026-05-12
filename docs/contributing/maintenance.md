# Maintenance playbook

This document describes the recurring maintenance work that keeps `specify-cli` honest against the coding standards. The mechanical surface is `cargo make standards` (predicates in [`xtask/src/standards.rs`](../../xtask/src/standards.rs)); predicate semantics are documented in [docs/standards/predicates.md](../standards/predicates.md).

## When to run a sweep

- **Scheduled.** The first business week of each quarter. Cut a PR even if `cargo make standards` is clean — the act of reviewing the top of the predicate list (which files are closest to a cap, which crate roots are creeping toward 30 lines of prose) is the point.
- **On demand.** Whenever a refactor exposes new debt against [`docs/standards/style.md`](../standards/style.md) — naming-by-context misses, error variants budgeted by source rather than recovery, testability-only traits.
- **Pre-release.** Before a tagged release, run `cargo make ci` and skim crate-root docs for archaeology that should have moved to [`DECISIONS.md`](../../DECISIONS.md).

A sweep is not the place to refactor architecture; it is the place to migrate the *top few* offenders one predicate at a time. Larger refactors go through the normal slice/change workflow.

## How to identify targets

1. Run `cargo make standards` from the repo root. Every predicate is zero-baseline — any non-zero count is a violation to fix, not a budget to spend.
2. For long-form drift (variant proliferation, wrapper newtypes, traits with one non-test impl) the predicates won't help. Read [`docs/standards/style.md`](../standards/style.md) and audit the crates touched since the last sweep.
3. Cross-check by running `cargo make standards` after a tentative fix to confirm the live count dropped to zero. The check is fast; iterate freely.

## PR shape

- **Title.** `chore: q<N> standards sweep` (e.g. `chore: q1 standards sweep`). Use the calendar quarter, not the fiscal one.
- **Scope.** One sweep PR per quarter. Keep it surgical — top few files, ideally one commit per file so reviewers can read the migration one target at a time. If a target turns out to be larger than a sweep should carry, split it off as its own change/slice.
- **Body.** Include the before/after `cargo make standards` summary, the list of files touched, and — for any target left undone — a one-paragraph rationale.
- **Reviewers.** Whoever owns the affected crates. A sweep that crosses crate boundaries is fine; just make sure each touched area has an owner on the review.

## Cross-links

- [docs/standards/style.md](../standards/style.md) — cross-cutting code-quality rules.
- [docs/standards/predicates.md](../standards/predicates.md) — the mechanical predicate table.
- [`xtask/src/standards.rs`](../../xtask/src/standards.rs) — predicate implementations.
