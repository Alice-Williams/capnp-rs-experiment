#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir="$repo_root/benchmarks/results/2026-08-31-m30-g-drive-docker"

grep -Fx 'workers=4' "$result_dir/metadata.txt"
grep -Fx 'samples_per_mode=7' "$result_dir/metadata.txt"
grep -Fx 'parallel_item_threshold=16384' "$result_dir/metadata.txt"
grep -Fx 'parallel_builder_sha256=2987e18be49c75589f0071584132c3369001051b29cdf31341de44934d7abd95' \
    "$result_dir/metadata.txt"
grep -Fx 'benchmark_example_sha256=ef87f082da82e686c4e9ad84e554b22202a2e75f250b2dc93a09be8ba262f2a0' \
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
    $3 == 4 && $4 == 4 && $7 >= 2.5 { qualifying += 1 }
    END {
        if (NR != 7 || below != 2 || qualifying < 1) exit 1;
    }
' "$result_dir/results.tsv"
