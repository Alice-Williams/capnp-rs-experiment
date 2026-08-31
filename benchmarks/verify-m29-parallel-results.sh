#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir="$repo_root/benchmarks/results/2026-08-31-m29-g-drive-docker"

grep -Fx 'workers=4' "$result_dir/metadata.txt"
grep -Fx 'samples_per_mode=7' "$result_dir/metadata.txt"
grep -Fx 'parallel_item_threshold=16384' "$result_dir/metadata.txt"
grep -Fx 'parallel_module_sha256=4cffe096966efe48133977c61b415c339b6c538f8f89fd2cd55b06b67b4c1ddd' \
    "$result_dir/metadata.txt"
grep -Fx 'benchmark_example_sha256=e8de6e4e62669678396ed6b2d691f27606bd4cd87fb1ebcc3a9e621236bf3b41' \
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
