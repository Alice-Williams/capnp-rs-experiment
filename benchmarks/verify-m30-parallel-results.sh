#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-08-31-m30-g-drive-docker"}

grep -Fx 'workers=4' "$result_dir/metadata.txt"
grep -Fx 'samples_per_mode=7' "$result_dir/metadata.txt"
grep -Fx 'parallel_item_threshold=16384' "$result_dir/metadata.txt"
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    base_commit=$(sed -n 's/^base_git_commit=//p' "$result_dir/metadata.txt")
    git -C "$repo_root" cat-file -e "${base_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$base_commit" HEAD
    provenance_commit=$base_commit
    if ! git -C "$repo_root" cat-file -e "$provenance_commit:crates/capnp-message/src/parallel_builder.rs" 2>/dev/null; then
        metadata_path=$(realpath --relative-to="$repo_root" "$result_dir/metadata.txt")
        provenance_commit=$(git -C "$repo_root" log --diff-filter=A -1 --format=%H -- "$metadata_path")
    fi
    builder_hash=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-message/src/parallel_builder.rs" | sha256sum | cut -d ' ' -f1)
    example_hash=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-message/examples/parallel_build.rs" | sha256sum | cut -d ' ' -f1)
else
    builder_hash=2987e18be49c75589f0071584132c3369001051b29cdf31341de44934d7abd95
    example_hash=ef87f082da82e686c4e9ad84e554b22202a2e75f250b2dc93a09be8ba262f2a0
fi
grep -Fx "parallel_builder_sha256=$builder_hash" "$result_dir/metadata.txt"
grep -Fx "benchmark_example_sha256=$example_hash" "$result_dir/metadata.txt"

awk -F '\t' '
    NR == 1 {
        if ($0 != "items\trounds\tworkers\tpartitions\tserial_ns\tparallel_ns\tspeedup\tchecksum") exit 1;
        next;
    }
    NF != 8 || $1 !~ /^[0-9]+$/ || $5 <= 0 || $6 <= 0 || $7 <= 0 { exit 1 }
    $1 < 16384 {
        below += 1;
        if ($4 != 1 || $6 > $5 * 1.05) exit 1;
    }
    $3 == 4 && $4 == 4 && $7 >= 2.5 { qualifying += 1 }
    END {
        if (NR != 7 || below != 2 || qualifying < 1) exit 1;
    }
' "$result_dir/results.tsv"
