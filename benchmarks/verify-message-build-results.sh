#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-03-m53-build-baseline-g-drive-docker"}
expected_passes=${2:-100000}
gate_mode=${3:-structural}
workload_count=$(tail -n +2 "$result_dir/results.tsv" | cut -f2,3 | sort -u | wc -l)
shape_count=$(tail -n +2 "$result_dir/results.tsv" | cut -f3 | sort -u | wc -l)
expected_results=$((1 + workload_count * 2 * 11))
expected_summary=$((1 + workload_count * 2))
expected_comparison=$((1 + workload_count))

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' "$result_dir/metadata.txt"
grep -Fx 'cpp_primitive=capnp::MallocMessageBuilder and capnp::AnyStruct::Builder' "$result_dir/metadata.txt"
grep -Fx 'native_primitive=capnp_message::ExclusiveArena and StructBuilder' "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=9' "$result_dir/metadata.txt"
grep -Fx "passes=$expected_passes" "$result_dir/metadata.txt"
native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi

test "$workload_count" -ge "$((shape_count * 2))"
test "$(wc -l < "$result_dir/results.tsv")" -eq "$expected_results"
test "$(wc -l < "$result_dir/summary.tsv")" -eq "$expected_summary"
test "$(wc -l < "$result_dir/comparison.tsv")" -eq "$expected_comparison"
test "$(wc -l < "$result_dir/incremental.tsv")" -eq "$((shape_count + 1))"
awk -F '\t' -v expected="$expected_results" -v passes="$expected_passes" '
  NR == 1 { if ($0 != "implementation\tcase\tshape\tpasses\trun\telapsed_ns\tsemantic_checksum\twire_checksum") exit 1; next }
  NF != 8 || $4 != passes || $6 <= 0 || $7 < 0 || $8 < 0 { exit 1 }
  $1 != "cpp" && $1 != "native" { exit 1 }
  $2 != "prepared" && $2 != "fresh" && $2 != "reuse" { exit 1 }
  $3 != "direct" && $3 != "far" && $3 != "double-far" { exit 1 }
  !($1 FS $2 FS $3 in semantic) {
    semantic[$1 FS $2 FS $3] = $7
    wire[$1 FS $2 FS $3] = $8
    if (($2 FS $3 in cross) && $7 != cross[$2 FS $3]) exit 1
    cross[$2 FS $3] = $7
    next
  }
  $7 != semantic[$1 FS $2 FS $3] || $8 != wire[$1 FS $2 FS $3] { exit 1 }
  $7 != cross[$2 FS $3] { exit 1 }
  END { if (NR != expected || length(semantic) != workloads * 2 || length(cross) != workloads) exit 1 }
' workloads="$workload_count" "$result_dir/results.tsv"
awk -F '\t' -v expected="$expected_summary" 'NR == 1 { next } NF != 9 || $6 <= 0 || $7 <= 0 || $8 <= 0 || $9 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/summary.tsv"
awk -F '\t' -v expected="$expected_comparison" 'NR == 1 { next } NF != 5 || $3 <= 0 || $4 <= 0 || $5 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/comparison.tsv"
awk -F '\t' -v expected="$((shape_count + 1))" 'NR == 1 { next } NF != 6 || $2 <= 0 || $3 <= 0 || $4 <= 0 || $5 <= 0 || $6 <= 0 { exit 1 } END { if (NR != expected) exit 1 }' "$result_dir/incremental.tsv"

if [[ "$gate_mode" == final ]]; then
    awk -F '\t' '
      NR == 1 { next }
      $1 == "prepared" && $5 > 1.036 { exit 1 }
      $1 == "fresh" && $5 > 1.03 { exit 1 }
      $1 == "reuse" && $5 > 1.03 { exit 1 }
      END { if (NR < 5) exit 1 }
    ' "$result_dir/comparison.tsv"
    awk -F '\t' -v expected="$((shape_count + 1))" 'NR == 1 { next } $4 > 1.03 { exit 1 } END { if (NR != expected) exit 1 }' \
        "$result_dir/incremental.tsv"
fi
