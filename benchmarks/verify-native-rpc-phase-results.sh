#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-02-native-rpc-phase-g-drive-docker"}

native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi
grep -Fx 'method=Instant around nine native sequential RPC phases; wall timer around child process' \
    "$result_dir/metadata.txt"
grep -Fx 'iterations=100000' "$result_dir/metadata.txt"
test "$(wc -l < "$result_dir/phases.tsv")" -eq 10
test "$(wc -l < "$result_dir/totals.tsv")" -eq 2

awk -F '\t' '
    NR == 1 {
        if ($0 != "phase\ttotal_ns\tns_per_call") exit 1;
        next;
    }
    NF != 3 || $2 <= 0 || $3 <= 0 { exit 1 }
    { seen[$1] += 1 }
    END {
        if (NR != 10 || length(seen) != 9) exit 1;
        for (phase in seen) if (seen[phase] != 1) exit 1;
    }
' "$result_dir/phases.tsv"

phase_ns=$(awk -F '\t' 'NR > 1 { total += $2 } END { print total }' \
    "$result_dir/phases.tsv")
awk -F '\t' -v expected_phase_ns="$phase_ns" '
    NR == 1 {
        if ($0 != "iterations\twall_ns\tphase_ns\tunattributed_ns\twall_ns_per_call") exit 1;
        next;
    }
    NF != 5 || $1 != 100000 || $2 <= 0 || $3 != expected_phase_ns || $4 < 0 || $5 <= 0 { exit 1 }
    $2 != $3 + $4 { exit 1 }
    END { if (NR != 2) exit 1 }
' "$result_dir/totals.tsv"
