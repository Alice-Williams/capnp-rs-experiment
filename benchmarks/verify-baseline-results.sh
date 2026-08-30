#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir="$repo_root/benchmarks/results/2026-08-30-g-drive-docker"

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' \
    "$result_dir/metadata.txt"
grep -Fx 'rust_oracle_commit=2228b71e55cee819c30450bb9bfd9c1f6a722429' \
    "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=5' "$result_dir/metadata.txt"

test "$(wc -l < "$result_dir/results.tsv")" -eq 61
test "$(wc -l < "$result_dir/summary.tsv")" -eq 11

awk -F '\t' '
    NR == 1 { next }
    NF != 8 { exit 1 }
    $6 !~ /^[0-9]+$/ || $7 !~ /^[0-9]+$/ || $8 <= 0 { exit 1 }
    END { if (NR != 11) exit 1 }
' "$result_dir/summary.tsv"

for implementation in cpp rust; do
    test "$(grep -c "^${implementation}"$'\t' "$result_dir/summary.tsv")" -eq 5
done
