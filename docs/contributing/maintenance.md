# Maintenance playbook

This document describes the recurring maintenance work that keeps `specify-cli`'s standards baselines from accumulating debt. The cadence itself is codified in [AGENTS.md §"Quarterly migration cadence"](../../AGENTS.md#quarterly-migration-cadence); this file is the operational companion.

The mechanical surface is `cargo make standards` (predicates in [`xtask/src/standards.rs`](../../xtask/src/standards.rs), per-file baselines in [`scripts/standards-allowlist.toml`](../../scripts/standards-allowlist.toml)). Predicate semantics are documented in [AGENTS.md §"Mechanical enforcement"](../../AGENTS.md#mechanical-enforcement).

## When to run a sweep

- **Scheduled.** The first business week of each quarter. Cut a PR even if the allowlist looks healthy — the act of reviewing the top of the list is the point.
- **On demand.** Whenever an unrelated PR is forced to raise a baseline to land (this should be rare; raising baselines is forbidden in any other context). Treat the raise as a debt entry and clear it inside the next sweep.
- **Pre-release.** Before a tagged release, glance at the totals to make sure no quiet drift has happened mid-cycle.

A sweep is not the place to refactor architecture; it is the place to migrate the *top five* offenders one predicate at a time. Larger refactors go through the normal slice/change workflow.

## How to identify targets

1. Run `cargo make standards` from the repo root. The footer prints per-predicate totals across the whole tree.
2. For per-file ranking, open `scripts/standards-allowlist.toml` and sort files by their **total** grandfathered violations (sum of every numeric entry on the file, excluding `module-line-count` if you only care about predicate debt — include it if module length is the worry). The five highest-total files are this quarter's candidates.
3. Cross-check by running `cargo make standards` after a tentative fix to confirm the live count dropped. The check is fast; iterate freely.

Predicates worth watching, in roughly decreasing migration friction:

- `rfc-numbers-in-code` — usually doc-comment rewrites; safe to chip at.
- `ritual-doc-paragraphs` — almost always a one-line deletion.
- `format-match-dispatch` — requires introducing a `Render` impl; see [`src/commands/codex.rs`](../../src/commands/codex.rs) for the canonical pattern.
- `module-line-count` — split by concern (one verb per file, model vs IO vs transitions); see AGENTS.md §"Module layout".
- The zero-baseline predicates (`error-envelope-inlined`, `path-helper-inlined`) — start at zero and should stay at zero; if one drifts, that's a regression, not a sweep target.

## How to update baselines

- After landing a fix that lowers a live count, run `cargo make standards-tighten`. This rewrites `scripts/standards-allowlist.toml` so every per-file baseline matches today's actual count. Commit the diff together with the fix.
- **Never raise a baseline.** The ratchet is one-way. If a code change would require raising a baseline, the change is the problem; redesign it. `cargo make standards-check-tight` (run as part of `cargo make ci`) fails any PR that could lower a baseline without code changes, so an honest sweep is the only way the file ever shrinks.
- `cargo make ci` is the gate. It runs `lint`, `standards-check`, `standards-check-tight`, `test`, `test-docs`, `doc`, `vet`, `outdated`, `deny`, and `fmt`. A sweep PR that doesn't pass `cargo make ci` locally is not ready for review.

## PR shape

- **Title.** `chore: q<N> standards-allowlist sweep` (e.g. `chore: q1 standards-allowlist sweep`). Use the calendar quarter, not the fiscal one.
- **Scope.** One sweep PR per quarter. Keep it surgical — top five files, ideally one commit per file so reviewers can read the migration one target at a time. If a target turns out to be larger than a sweep should carry, split it off as its own change/slice and document the deferral in the PR body.
- **Body.** Include the before/after totals from `cargo make standards`, the list of files touched, and — for any target left undone — a one-paragraph rationale that gets mirrored back into [AGENTS.md §"Quarterly migration cadence"](../../AGENTS.md#quarterly-migration-cadence) so the next sweep inherits the context.
- **Reviewers.** Whoever owns the affected crates. A sweep that crosses crate boundaries is fine; just make sure each touched area has an owner on the review.

## Cross-links

- [AGENTS.md §"Coding standards"](../../AGENTS.md#coding-standards) — the standards themselves and the predicate table.
- [AGENTS.md §"Quarterly migration cadence"](../../AGENTS.md#quarterly-migration-cadence) — the cadence rule this playbook implements.
- [`scripts/standards-allowlist.toml`](../../scripts/standards-allowlist.toml) — the file every sweep is editing.
- [`xtask/src/standards.rs`](../../xtask/src/standards.rs) — predicate implementations.
