#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-09-02-native-phase-g-drive-docker"}

native_commit=$(sed -n 's/^native_commit=//p' "$result_dir/metadata.txt")
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$repo_root" cat-file -e "${native_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$native_commit" HEAD
fi
grep -Fx 'method=Instant around seven native phases; wall timer around child process' \
    "$result_dir/metadata.txt"
test "$(wc -l < "$result_dir/phases.tsv")" -eq 36
test "$(wc -l < "$result_dir/totals.tsv")" -eq 6

awk -F '\t' '
    NR == 1 { next }
    NF != 7 || $4 <= 0 || $6 < 0 || $7 < 0 { exit 1 }
    { scenarios[$1 FS $2 FS $3 FS $4] += 1 }
    END {
        if (NR != 36 || length(scenarios) != 5) exit 1;
        for (scenario in scenarios) if (scenarios[scenario] != 7) exit 1;
    }
' "$result_dir/phases.tsv"

awk -F '\t' '
    NR == 1 { next }
    NF != 7 || $4 <= 0 || $5 <= 0 || $6 <= 0 || $7 < 0 { exit 1 }
    $5 != $6 + $7 { exit 1 }
    END { if (NR != 6) exit 1 }
' "$result_dir/totals.tsv"
