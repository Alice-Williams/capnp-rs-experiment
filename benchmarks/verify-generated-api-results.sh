#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-03-m55-generated-reader-baseline-g-drive-docker"}
expected_passes=${2:-100000}
expected_cases=${3:-4}

if [[ "$expected_cases" != 4 && "$expected_cases" != 6 && "$expected_cases" != 8 && "$expected_cases" != 10 ]]; then
    printf 'expected case count must be 4, 6, 8, or 10\n' >&2
    exit 2
fi
expected_raw_lines=$((1 + expected_cases * 2 * 11))
expected_summary_lines=$((1 + expected_cases * 2))
expected_comparison_lines=$((1 + expected_cases))
expected_incremental_lines=3
if [[ "$expected_cases" == 6 || "$expected_cases" == 8 ]]; then expected_incremental_lines=5; fi

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' "$result_dir/metadata.txt"
grep -Fx 'schema=conformance/schemas/wire-fixture.capnp' "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=9' "$result_dir/metadata.txt"
grep -Fx "passes=$expected_passes" "$result_dir/metadata.txt"
native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi

test "$(wc -l < "$result_dir/results.tsv")" -eq "$expected_raw_lines"
test "$(wc -l < "$result_dir/summary.tsv")" -eq "$expected_summary_lines"
test "$(wc -l < "$result_dir/comparison.tsv")" -eq "$expected_comparison_lines"
test "$(wc -l < "$result_dir/incremental.tsv")" -eq "$expected_incremental_lines"
awk -F '\t' -v passes="$expected_passes" -v cases="$expected_cases" -v expected_lines="$expected_raw_lines" '
  NR == 1 {
    if ($0 != "implementation\tcase\tpasses\trun\telapsed_ns\tchecksum") exit 1
    next
  }
  NF != 6 || $3 != passes || $5 <= 0 || $6 < 0 { exit 1 }
  $1 != "cpp" && $1 != "native" { exit 1 }
  $2 != "direct-scalars" && $2 != "generated-scalars" &&
      $2 != "direct-blobs" && $2 != "generated-blobs" &&
      !(cases >= 6 && ($2 == "borrowed-scalars" || $2 == "borrowed-blobs")) &&
      !(cases >= 8 && ($2 == "borrowed-direct-scalars" || $2 == "borrowed-direct-blobs")) &&
      !(cases == 10 && ($2 == "borrowed-direct-groups" || $2 == "borrowed-groups")) { exit 1 }
  {
    shape = $2
    sub(/^(borrowed-direct|direct|generated|borrowed)-/, "", shape)
  }
  !(shape in checksum) { checksum[shape] = $6; next }
  $6 != checksum[shape] { exit 1 }
  END { if (NR != expected_lines || length(checksum) != (cases == 10 ? 3 : 2)) exit 1 }
' "$result_dir/results.tsv"
awk -F '\t' -v expected="$expected_summary_lines" 'NR == 1 { next } NF != 8 || $4 != 9 || $5 <= 0 || $6 <= 0 || $7 <= 0 || $8 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/summary.tsv"
awk -F '\t' -v expected="$expected_comparison_lines" 'NR == 1 { next } NF != 4 || $2 <= 0 || $3 <= 0 || $4 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/comparison.tsv"
awk -F '\t' -v expected="$expected_incremental_lines" 'NR == 1 { next } NF != 6 || $5 <= 0 || $6 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/incremental.tsv"
