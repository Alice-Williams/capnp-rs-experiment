#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir="$repo_root/benchmarks/results/2026-08-31-m39-g-drive-docker"

grep -Fx 'jobs=64' "$result_dir/metadata.txt"
grep -Fx 'rounds_per_job=5000000' "$result_dir/metadata.txt"
grep -Fx 'samples_per_configuration=7' "$result_dir/metadata.txt"
grep -Fx 'scheduler_module_sha256=f69933c1c4c9fc5ef392a4d91c9b94e203ad7483ec4a8529542f5afb64730a53' \
    "$result_dir/metadata.txt"
grep -Fx 'benchmark_example_sha256=633b3ca1f503e8985d8b3cbf8633ab071d01c3bf82f8c3dfd6838118ddbeeeeb' \
    "$result_dir/metadata.txt"
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
