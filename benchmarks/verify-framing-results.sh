#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-02-m51-framing-baseline-g-drive-docker"}

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' "$result_dir/metadata.txt"
grep -Fx 'cpp_primitive=capnp::FlatArrayMessageReader and capnp::messageToFlatArray' "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=9' "$result_dir/metadata.txt"
native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi

test "$(wc -l < "$result_dir/results.tsv")" -eq 133
test "$(wc -l < "$result_dir/summary.tsv")" -eq 13
test "$(wc -l < "$result_dir/comparison.tsv")" -eq 7
awk -F '\t' '
  NR == 1 { if ($0 != "implementation\tcase\tsegments\tpasses\trun\telapsed_ns\tchecksum") exit 1; next }
  NF != 7 || $4 != 50000 || $6 <= 0 || $7 < 0 { exit 1 }
  $1 != "cpp" && $1 != "native" { exit 1 }
  $2 != "parse" && $2 != "encode" { exit 1 }
  $3 != 1 && $3 != 2 && $3 != 64 { exit 1 }
  !($2 FS $3 in checksum) { checksum[$2 FS $3] = $7; next }
  $7 != checksum[$2 FS $3] { exit 1 }
  END { if (NR != 133 || length(checksum) != 6) exit 1 }
' "$result_dir/results.tsv"
awk -F '\t' 'NR == 1 { next } NF != 9 || $4 != 50000 || $5 != 9 || $6 <= 0 || $7 <= 0 || $8 <= 0 || $9 <= 0 { exit 1 } END { if (NR != 13) exit 1 }' "$result_dir/summary.tsv"
awk -F '\t' 'NR == 1 { next } NF != 5 || $3 <= 0 || $4 <= 0 || $5 <= 0 { exit 1 } END { if (NR != 7) exit 1 }' "$result_dir/comparison.tsv"
