#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-08-31-m31-g-drive-docker"}

grep -Fx 'workers=4' "$result_dir/metadata.txt"
samples=$(sed -n 's/^samples_per_mode=//p' "$result_dir/metadata.txt")
if ! [[ "$samples" =~ ^[1-9][0-9]*$ ]] || (( samples < 7 || samples % 2 == 0 )); then
    printf 'M31 samples must be odd and at least 7: %s\n' "$samples" >&2
    exit 1
fi
grep -Fx 'parallel_message_threshold=2' "$result_dir/metadata.txt"
if git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    base_commit=$(sed -n 's/^base_git_commit=//p' "$result_dir/metadata.txt")
    git -C "$repo_root" cat-file -e "${base_commit}^{commit}"
    git -C "$repo_root" merge-base --is-ancestor "$base_commit" HEAD
    provenance_commit=$base_commit
    if ! git -C "$repo_root" cat-file -e "$provenance_commit:crates/capnp-async/src/batch.rs" 2>/dev/null; then
        metadata_path=$(realpath --relative-to="$repo_root" "$result_dir/metadata.txt")
        provenance_commit=$(git -C "$repo_root" log --diff-filter=A -1 --format=%H -- "$metadata_path")
    fi
    batch_hash=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-async/src/batch.rs" | sha256sum | cut -d ' ' -f1)
    example_hash=$(git -C "$repo_root" show "$provenance_commit:crates/capnp-async/examples/batch_pipeline.rs" | sha256sum | cut -d ' ' -f1)
else
    batch_hash=a27edde047e3fdaf320e06d9af703a4b0a3207f2e9387fd276c53db4935414ef
    example_hash=0e5b6201ddbfdae439292b8110956e655063a419a254a1df1cc8422210085d7c
fi
grep -Fx "batch_module_sha256=$batch_hash" "$result_dir/metadata.txt"
grep -Fx "benchmark_example_sha256=$example_hash" "$result_dir/metadata.txt"

awk -F '\t' '
    NR == 1 {
        if ($0 != "messages\twords\trounds\tworkers\tworkers_used\tserial_ns\tparallel_ns\tspeedup\tchecksum") exit 1;
        next;
    }
    NF != 9 || $1 !~ /^[0-9]+$/ || $6 <= 0 || $7 <= 0 || $8 <= 0 { exit 1 }
    $1 == 1 {
        single += 1;
        if ($5 != 1 || $7 > $6 * 1.05) exit 1;
    }
    $1 >= 16 && $4 == 4 && $5 == 4 && $8 >= 3.0 { qualifying += 1 }
    END {
        if (NR != 8 || single != 1 || qualifying < 1) exit 1;
    }
' "$result_dir/results.tsv"
