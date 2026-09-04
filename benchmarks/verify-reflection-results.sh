#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:?result directory is required}
expected_passes=${2:-100000}
expected_cases=${3:-4}

if ((expected_cases < 4 || expected_cases > 11)); then
    printf 'expected case count must be from 4 through 11\n' >&2
    exit 2
fi
expected_raw_lines=$((1 + expected_cases * 2 * 11))
expected_summary_lines=$((1 + expected_cases * 2))
expected_comparison_lines=$((1 + expected_cases))

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
awk -F '\t' -v passes="$expected_passes" -v cases="$expected_cases" -v expected="$expected_raw_lines" '
  NR == 1 {
    if ($0 != "implementation\tcase\tpasses\trun\telapsed_ns\tchecksum") exit 1
    next
  }
  NF != 6 || $3 != passes || $5 <= 0 || $6 < 0 { exit 1 }
  $1 != "cpp" && $1 != "native" { exit 1 }
  $2 != "schema-name" && $2 != "schema-index" &&
      $2 != "dynamic-name" && $2 != "dynamic-index" &&
      !(cases >= 5 && $2 == "dynamic-field") &&
      !(cases >= 6 && $2 == "dynamic-blobs-borrowed") &&
      !(cases >= 7 && $2 == "dynamic-blobs-owned") &&
      !(cases >= 8 && $2 == "dynamic-primitive-list") &&
      !(cases >= 9 && $2 == "dynamic-nested-struct") &&
      !(cases >= 10 && $2 == "dynamic-struct-list") &&
      !(cases >= 11 && $2 == "dynamic-nested-list") { exit 1 }
  !($2 in checksum) { checksum[$2] = $6; next }
  $6 != checksum[$2] { exit 1 }
  END { if (NR != expected || length(checksum) != cases) exit 1 }
' "$result_dir/results.tsv"
awk -F '\t' -v expected="$expected_summary_lines" 'NR == 1 { next } NF != 8 || $4 != 9 || $5 <= 0 || $6 <= 0 || $7 <= 0 || $8 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/summary.tsv"
awk -F '\t' -v expected="$expected_comparison_lines" 'NR == 1 { next } NF != 4 || $2 <= 0 || $3 <= 0 || $4 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/comparison.tsv"
