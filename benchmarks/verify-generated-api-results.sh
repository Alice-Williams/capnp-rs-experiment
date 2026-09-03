#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-03-m55-generated-reader-baseline-g-drive-docker"}
expected_passes=${2:-100000}

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' "$result_dir/metadata.txt"
grep -Fx 'schema=conformance/schemas/wire-fixture.capnp' "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=9' "$result_dir/metadata.txt"
grep -Fx "passes=$expected_passes" "$result_dir/metadata.txt"
native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi

test "$(wc -l < "$result_dir/results.tsv")" -eq 89
test "$(wc -l < "$result_dir/summary.tsv")" -eq 9
test "$(wc -l < "$result_dir/comparison.tsv")" -eq 5
test "$(wc -l < "$result_dir/incremental.tsv")" -eq 3
awk -F '\t' -v passes="$expected_passes" '
  NR == 1 {
    if ($0 != "implementation\tcase\tpasses\trun\telapsed_ns\tchecksum") exit 1
    next
  }
  NF != 6 || $3 != passes || $5 <= 0 || $6 < 0 { exit 1 }
  $1 != "cpp" && $1 != "native" { exit 1 }
  $2 != "direct-scalars" && $2 != "generated-scalars" &&
      $2 != "direct-blobs" && $2 != "generated-blobs" { exit 1 }
  {
    shape = $2
    sub(/^(direct|generated)-/, "", shape)
  }
  !(shape in checksum) { checksum[shape] = $6; next }
  $6 != checksum[shape] { exit 1 }
  END { if (NR != 89 || length(checksum) != 2) exit 1 }
' "$result_dir/results.tsv"
awk -F '\t' 'NR == 1 { next } NF != 8 || $4 != 9 || $5 <= 0 || $6 <= 0 || $7 <= 0 || $8 <= 0 { exit 1 } END { if (NR != 9) exit 1 }' "$result_dir/summary.tsv"
awk -F '\t' 'NR == 1 { next } NF != 4 || $2 <= 0 || $3 <= 0 || $4 <= 0 { exit 1 } END { if (NR != 5) exit 1 }' "$result_dir/comparison.tsv"
awk -F '\t' 'NR == 1 { next } NF != 6 || $5 <= 0 || $6 <= 0 { exit 1 } END { if (NR != 3) exit 1 }' "$result_dir/incremental.tsv"
