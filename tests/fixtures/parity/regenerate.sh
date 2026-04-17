#!/usr/bin/env bash
# Regenerate expected-*.{md,txt} files by running merge-specs.py against each
# case directory. Idempotent — run whenever an input changes.
#
# Requires: python3 on PATH.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PY="$ROOT/scripts/legacy/merge-specs.py"
cd "$ROOT/tests/fixtures/parity"

# ---- Merge cases (01 through 07) ----
for dir in case-01-single-req case-02-multi-req case-03-new-baseline case-04-modified case-05-removed case-06-renamed case-07-all-sections; do
    echo "==> $dir (merge)"
    pushd "$dir" > /dev/null

    baseline_arg=""
    if [[ -f baseline.md ]]; then
        baseline_arg="--baseline baseline.md"
    fi

    merge_stderr=$(mktemp)
    set +e
    python3 "$PY" $baseline_arg --delta delta.md --output expected-merged.md 2> "$merge_stderr"
    exit_code=$?
    set -e

    if [[ $exit_code -eq 0 ]]; then
        : > expected-merge-errors.txt
    else
        cp "$merge_stderr" expected-merge-errors.txt
        # Keep any partial output that might exist
        rm -f expected-merged.md
    fi
    rm -f "$merge_stderr"

    popd > /dev/null
done

# ---- Validation cases (08, 09, 10) ----
# Case 08: baseline is valid, validate it directly
pushd case-08-validation-ok > /dev/null
echo "==> case-08-validation-ok (validate)"
validate_stderr=$(mktemp)
set +e
python3 "$PY" --validate baseline.md 2> "$validate_stderr" > /dev/null
set -e
cp "$validate_stderr" expected-validation.txt
rm -f "$validate_stderr"
popd > /dev/null

# Case 09: baseline is malformed, validate
pushd case-09-validation-fails > /dev/null
echo "==> case-09-validation-fails (validate)"
validate_stderr=$(mktemp)
set +e
python3 "$PY" --validate baseline.md 2> "$validate_stderr" > /dev/null
set -e
cp "$validate_stderr" expected-validation.txt
rm -f "$validate_stderr"
popd > /dev/null

# Case 10: validate baseline with a design.md that has orphan refs
pushd case-10-design-refs > /dev/null
echo "==> case-10-design-refs (validate with design)"
validate_stderr=$(mktemp)
set +e
python3 "$PY" --validate baseline.md --design design.md 2> "$validate_stderr" > /dev/null
set -e
cp "$validate_stderr" expected-validation.txt
rm -f "$validate_stderr"
popd > /dev/null

echo "Regeneration complete."
