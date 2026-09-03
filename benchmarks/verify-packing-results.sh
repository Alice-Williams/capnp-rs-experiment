#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:?usage: verify-packing-results.sh RESULT_DIRECTORY [final]}
mode=${2:-baseline}
warmups=$(sed -n 's/^warmups=//p' "$result_dir/metadata.txt")
recorded_runs=$(sed -n 's/^recorded_runs=//p' "$result_dir/metadata.txt")
words=$(sed -n 's/^words=//p' "$result_dir/metadata.txt")
passes=$(sed -n 's/^passes=//p' "$result_dir/metadata.txt")
cases=16
expected_results=$((1 + cases * 2 * (warmups + recorded_runs)))

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=9' "$result_dir/metadata.txt"
native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi

test "$(wc -l < "$result_dir/results.tsv")" -eq "$expected_results"
test "$(wc -l < "$result_dir/summary.tsv")" -eq 33
test "$(wc -l < "$result_dir/comparison.tsv")" -eq 17
test "$(wc -l < "$result_dir/incremental.tsv")" -eq 9

awk -F '\t' -v expected="$expected_results" -v words="$words" -v passes="$passes" '
    NR == 1 {
        if ($0 != "implementation\tcase\tshape\twords\tpasses\trun\telapsed_ns\tchecksum") exit 1;
        next;
    }
    NF != 8 || $4 != words || $5 != passes || $7 <= 0 || $8 < 0 { exit 1 }
    $1 != "cpp" && $1 != "native" { exit 1 }
    !($2 FS $3 in checksum) { checksum[$2 FS $3] = $8; next }
    $8 != checksum[$2 FS $3] { exit 1 }
    END { if (NR != expected || length(checksum) != 16) exit 1 }
' "$result_dir/results.tsv"

awk -F '\t' -v runs="$recorded_runs" '
    NR == 1 { next }
    NF != 10 || $6 != runs || $7 <= 0 || $8 <= 0 || $9 <= 0 || $10 <= 0 { exit 1 }
    END { if (NR != 33) exit 1 }
' "$result_dir/summary.tsv"
awk -F '\t' 'NR == 1 { next } NF != 5 || $3 <= 0 || $4 <= 0 || $5 <= 0 { exit 1 } END { if (NR != 17) exit 1 }' "$result_dir/comparison.tsv"
awk -F '\t' -v final="$mode" '
    NR == 1 { next }
    NF != 8 || $4 <= 0 || $5 <= 0 || $6 <= 0 || $7 <= 0 || $8 <= 0 { exit 1 }
    final == "final" && ($5 > $4 * 1.03 || $8 > 1.03) { exit 1 }
    END { if (NR != 9) exit 1 }
' "$result_dir/incremental.tsv"
