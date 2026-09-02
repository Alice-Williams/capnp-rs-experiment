#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-02-native-cpp-rpc-g-drive-docker"}

grep -Fx 'cpp_oracle_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b' \
    "$result_dir/metadata.txt"
grep -Fx 'recorded_runs=9' "$result_dir/metadata.txt"
grep -Fx 'iterations=10000' "$result_dir/metadata.txt"
grep -Fx 'transport_caveat=native path does not encode or parse standard framing' \
    "$result_dir/metadata.txt"
native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi

test "$(wc -l < "$result_dir/results.tsv")" -eq 23
test "$(wc -l < "$result_dir/summary.tsv")" -eq 3
test "$(wc -l < "$result_dir/comparison.tsv")" -eq 2

awk -F '\t' '
    NR == 1 {
        if ($0 != "implementation\ttransport\titerations\trun\telapsed_ns\tchecksum") exit 1;
        next;
    }
    NF != 6 || $3 != 10000 || $5 <= 0 || $6 != 10000 { exit 1 }
    $1 != "cpp" && $1 != "native" { exit 1 }
    END { if (NR != 23) exit 1 }
' "$result_dir/results.tsv"

awk -F '\t' '
    NR == 1 { next }
    NF != 8 || $3 != 10000 || $4 != 9 || $5 <= 0 || $6 <= 0 || $7 <= 0 || $8 <= 0 { exit 1 }
    END { if (NR != 3) exit 1 }
' "$result_dir/summary.tsv"

awk -F '\t' '
    NR == 1 { next }
    NF != 3 || $1 <= 0 || $2 <= 0 || $3 <= 0 { exit 1 }
    END { if (NR != 2) exit 1 }
' "$result_dir/comparison.tsv"

for implementation in cpp native; do
    test "$(grep -c "^${implementation}"$'\t' "$result_dir/summary.tsv")" -eq 1
done
