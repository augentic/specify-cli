#!/usr/bin/env bash
# scripts/standards-check.sh
#
# Mechanical enforcement of the rules in AGENTS.md#coding-standards. CI runs
# this via `cargo make standards-check`; failures cite this script and the
# AGENTS.md anchor to follow up on.
#
# Strategy: count-baseline. Each rule has an integer ceiling in
# scripts/standards-allowlist.txt. If the live count exceeds the ceiling the
# check fails. Reductions are encouraged; lower the ceiling in the same PR.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

allowlist="scripts/standards-allowlist.txt"
status=0

# load_baseline KEY -> echoes integer
baseline() {
    awk -F= -v key="$1" '$1 == key { print $2 }' "$allowlist"
}

count_pattern() {
    local pattern="$1"
    shift
    # grep exits 1 when there are no matches; treat that as zero under set -e.
    grep -rE "$pattern" "$@" --include='*.rs' 2>/dev/null | wc -l | tr -d ' ' || true
}

count_multiline_pattern() {
    local pattern="$1"
    shift
    grep -rPzo "$pattern" "$@" --include='*.rs' 2>/dev/null | tr -cd '\0' | wc -c | tr -d ' ' || true
}

check_count() {
    local name="$1" want max actual
    max="$(baseline "$name")"
    actual="$2"
    if [[ -z "$max" ]]; then
        printf '  standards-check: missing baseline %s in %s\n' "$name" "$allowlist" >&2
        status=1
        return
    fi
    if (( actual > max )); then
        printf '  FAIL %s: %d hits, baseline %d (see AGENTS.md#%s)\n' \
            "$name" "$actual" "$max" "$3" >&2
        status=1
    else
        printf '  OK   %s: %d hits (baseline %d)\n' "$name" "$actual" "$max"
    fi
}

# 1. RFC numbers in code (AGENTS.md#comments).
rfc_hits="$(count_pattern 'RFC[- ]?[0-9]+' src/ crates/)"
check_count "rfc-numbers-in-code" "${rfc_hits:-0}" "comments"

# 2. Inline DTOs inside fn bodies (AGENTS.md#dtos).
#    A `fn` body that contains `#[derive(... Serialize ...)]` before the
#    closing brace is almost certainly an inline DTO.
inline_dto_hits="$(count_multiline_pattern '(?s)fn [^{]*\{[^}]*?#\[derive\([^)]*Serialize' src/ crates/)"
check_count "inline-dtos" "${inline_dto_hits:-0}" "dtos"

# 3. Free-form Error::*(String) call sites (AGENTS.md#errors).
error_hits="$(count_pattern 'Error::(Config|Merge|ToolResolver|ToolRuntime|CapabilityResolution)\(' src/ crates/)"
check_count "free-form-error-strings" "${error_hits:-0}" "errors"

# 4. Format-dispatch open-coding (AGENTS.md#format-dispatch).
#    Once Render lands, hand-rolled `match ctx.format { Json => ... }` should
#    be the exception, not the rule.
format_match_hits="$(count_pattern 'match[[:space:]]+(ctx\.)?format[[:space:]]*\{' src/ crates/)"
check_count "format-match-dispatch" "${format_match_hits:-0}" "format-dispatch"

if (( status != 0 )); then
    printf '\nstandards-check failed. Reduce the offending counts or, if a hit is justified, lower the baseline in %s in the same PR.\n' "$allowlist" >&2
fi

exit "$status"
