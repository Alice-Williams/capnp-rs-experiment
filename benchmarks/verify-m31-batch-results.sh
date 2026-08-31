#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir="$repo_root/benchmarks/results/2026-08-31-m31-g-drive-docker"

grep -Fx 'workers=4' "$result_dir/metadata.txt"
grep -Fx 'samples_per_mode=7' "$result_dir/metadata.txt"
grep -Fx 'parallel_message_threshold=2' "$result_dir/metadata.txt"
grep -Fx 'batch_module_sha256=a27edde047e3fdaf320e06d9af703a4b0a3207f2e9387fd276c53db4935414ef' \
    "$result_dir/metadata.txt"
grep -Fx 'benchmark_example_sha256=0e5b6201ddbfdae439292b8110956e655063a419a254a1df1cc8422210085d7c' \
    "$result_dir/metadata.txt"

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
