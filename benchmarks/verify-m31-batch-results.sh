#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-"$repo_root/benchmarks/results/2026-08-31-m31-g-drive-docker"}

grep -Fx 'workers=4' "$result_dir/metadata.txt"
grep -Fx 'samples_per_mode=7' "$result_dir/metadata.txt"
grep -Fx 'parallel_message_threshold=2' "$result_dir/metadata.txt"
grep -Fx "batch_module_sha256=$(sha256sum "$repo_root/crates/capnp-async/src/batch.rs" | cut -d ' ' -f1)" \
    "$result_dir/metadata.txt"
grep -Fx "benchmark_example_sha256=$(sha256sum "$repo_root/crates/capnp-async/examples/batch_pipeline.rs" | cut -d ' ' -f1)" \
    "$result_dir/metadata.txt"
base_commit=$(sed -n 's/^base_git_commit=//p' "$result_dir/metadata.txt")
git -C "$repo_root" cat-file -e "${base_commit}^{commit}"
git -C "$repo_root" merge-base --is-ancestor "$base_commit" HEAD

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
