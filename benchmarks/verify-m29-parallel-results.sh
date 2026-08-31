#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-08-31-m29-g-drive-docker"}

grep -Fx 'workers=4' "$result_dir/metadata.txt"
grep -Fx 'samples_per_mode=7' "$result_dir/metadata.txt"
grep -Fx 'parallel_item_threshold=16384' "$result_dir/metadata.txt"
base_commit=$(sed -n 's/^base_git_commit=//p' "$result_dir/metadata.txt")
git -C "$repo_root" cat-file -e "${base_commit}^{commit}"
git -C "$repo_root" merge-base --is-ancestor "$base_commit" HEAD
provenance_commit=$base_commit
if ! git -C "$repo_root" cat-file -e "$provenance_commit:crates/capnp-message/src/parallel.rs" 2>/dev/null; then
    metadata_path=$(realpath --relative-to="$repo_root" "$result_dir/metadata.txt")
    provenance_commit=$(git -C "$repo_root" log --diff-filter=A -1 --format=%H -- "$metadata_path")
fi
grep -Fx "parallel_module_sha256=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-message/src/parallel.rs" | sha256sum | cut -d ' ' -f1)" \
    "$result_dir/metadata.txt"
grep -Fx "benchmark_example_sha256=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-message/examples/parallel_read.rs" | sha256sum | cut -d ' ' -f1)" \
    "$result_dir/metadata.txt"

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
    $3 == 4 && $4 == 4 && $7 >= 3.0 { qualifying += 1 }
    END {
        if (NR != 7 || below != 2 || qualifying < 1) exit 1;
    }
' "$result_dir/results.tsv"
