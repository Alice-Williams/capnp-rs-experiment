#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-03-m52-root-baseline-g-drive-docker"}
expected_passes=${2:-100000}
workload_count=$(tail -n +2 "$result_dir/results.tsv" | cut -f2,3 | sort -u | wc -l)
expected_results=$((1 + workload_count * 2 * 11))
expected_summary=$((1 + workload_count * 2))
expected_comparison=$((1 + workload_count))

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' "$result_dir/metadata.txt"
grep -Fx 'cpp_primitive=capnp::FlatArrayMessageReader and capnp::AnyStruct::Reader' "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=9' "$result_dir/metadata.txt"
grep -Fx "passes=$expected_passes" "$result_dir/metadata.txt"
native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi

test "$(wc -l < "$result_dir/results.tsv")" -eq "$expected_results"
test "$(wc -l < "$result_dir/summary.tsv")" -eq "$expected_summary"
test "$(wc -l < "$result_dir/comparison.tsv")" -eq "$expected_comparison"
test "$(wc -l < "$result_dir/incremental.tsv")" -eq 4
awk -F '\t' -v expected="$expected_results" -v passes="$expected_passes" '
  NR == 1 { if ($0 != "implementation\tcase\tsegments\tpasses\trun\telapsed_ns\tchecksum") exit 1; next }
  NF != 7 || $4 != passes || $6 <= 0 || $7 < 0 { exit 1 }
  $1 != "cpp" && $1 != "native" { exit 1 }
  $2 != "framing" && $2 != "root" && $2 != "isolated-root" && $2 != "scalars" && $2 != "isolated-scalars" { exit 1 }
  $3 != 1 && $3 != 2 && $3 != 64 { exit 1 }
  !($2 FS $3 in checksum) { checksum[$2 FS $3] = $7; next }
  $7 != checksum[$2 FS $3] { exit 1 }
  END { if (NR != expected || length(checksum) != workloads) exit 1 }
' workloads="$workload_count" "$result_dir/results.tsv"
awk -F '\t' -v expected="$expected_summary" 'NR == 1 { next } NF != 9 || $6 <= 0 || $7 <= 0 || $8 <= 0 || $9 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/summary.tsv"
awk -F '\t' -v expected="$expected_comparison" 'NR == 1 { next } NF != 5 || $3 <= 0 || $4 <= 0 || $5 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/comparison.tsv"
awk -F '\t' 'NR == 1 { next } NF != 6 || $2 <= 0 || $3 <= 0 || $4 <= 0 || $5 <= 0 || $6 <= 0 { exit 1 } END { if (NR != 4) exit 1 }' "$result_dir/incremental.tsv"
if test -e "$result_dir/scalar-incremental.tsv"; then
    test "$(wc -l < "$result_dir/scalar-incremental.tsv")" -eq 4
    awk -F '\t' 'NR == 1 { next } NF != 6 || $2 <= 0 || $3 <= 0 || $4 <= 0 || $5 <= 0 || $6 <= 0 { exit 1 } END { if (NR != 4) exit 1 }' "$result_dir/scalar-incremental.tsv"
fi
