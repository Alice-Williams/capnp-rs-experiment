#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-08-31-m39-g-drive-docker"}

grep -Fx 'jobs=64' "$result_dir/metadata.txt"
grep -Fx 'rounds_per_job=5000000' "$result_dir/metadata.txt"
grep -Fx 'samples_per_configuration=7' "$result_dir/metadata.txt"
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    base_commit=$(sed -n 's/^base_git_commit=//p' "$result_dir/metadata.txt")
    git -C "$repo_root" cat-file -e "${base_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$base_commit" HEAD
    provenance_commit=$base_commit
    if ! git -C "$repo_root" cat-file -e "$provenance_commit:crates/capnp-rpc/src/scheduler.rs" 2>/dev/null; then
        metadata_path=$(realpath --relative-to="$repo_root" "$result_dir/metadata.txt")
        provenance_commit=$(git -C "$repo_root" log --diff-filter=A -1 --format=%H -- "$metadata_path")
    fi
    scheduler_hash=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-rpc/src/scheduler.rs" | sha256sum | cut -d ' ' -f1)
    example_hash=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-rpc/examples/server_scheduling.rs" | sha256sum | cut -d ' ' -f1)
else
    scheduler_hash=f69933c1c4c9fc5ef392a4d91c9b94e203ad7483ec4a8529542f5afb64730a53
    example_hash=633b3ca1f503e8985d8b3cbf8633ab071d01c3bf82f8c3dfd6838118ddbeeeeb
fi
grep -Fx "scheduler_module_sha256=$scheduler_hash" "$result_dir/metadata.txt"
grep -Fx "benchmark_example_sha256=$example_hash" "$result_dir/metadata.txt"
grep -E '^concurrent_four_to_one_ratio=([3-9]|[1-9][0-9]+)\.' "$result_dir/metadata.txt"

awk -F '\t' '
    NR == 1 {
        if ($0 != "policy\tworkers\tmedian_jobs_per_second\tmedian_p50_us\tmedian_p99_us\tmedian_max_key_run") exit 1;
        next;
    }
    NF != 6 || $3 <= 0 || $4 <= 0 || $5 <= 0 || $6 <= 0 { exit 1 }
    $1 == "concurrent" && $2 == 1 { one = $3 }
    $1 == "concurrent" && $2 == 4 { four = $3 }
    $1 == "serial" && $2 == 4 { serial += 1 }
    $1 == "keyed" && $2 == 4 { keyed += 1; if ($6 > 8) exit 1 }
    END { if (NR != 5 || four / one < 3.0 || serial != 1 || keyed != 1) exit 1 }
' "$result_dir/summary.tsv"

awk -F '\t' '
    NR == 1 { next }
    NF != 9 || $3 != 64 || $4 != 5000000 || $5 <= 0 || $6 <= 0 || $7 <= 0 || $8 <= 0 || $9 <= 0 { exit 1 }
    END { if (NR != 29) exit 1 }
' "$result_dir/raw.tsv"
