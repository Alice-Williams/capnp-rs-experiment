#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
rust_commit=2228b71e55cee819c30450bb9bfd9c1f6a722429
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-1}
runs=${BENCH_RUNS:-5}

if [[ "$output" == "--syntax-check-only" ]]; then
    exit 0
fi

if [[ -z "$output" ]]; then
    printf 'usage: %s OUTPUT_DIRECTORY\n' "$0" >&2
    exit 1
fi
if [[ -e "$output" ]]; then
    printf 'refusing to overwrite benchmark output: %s\n' "$output" >&2
    exit 1
fi
if ! [[ "$warmups" =~ ^[0-9]+$ && "$runs" =~ ^[1-9][0-9]*$ ]]; then
    printf 'BENCH_WARMUPS and BENCH_RUNS must be non-negative/positive integers\n' >&2
    exit 1
fi

bash "$repo_root/tools/build-oracle-benchmarks.sh" >/dev/null

cpp_dir="$oracle_root/capnproto-$cpp_commit/benchmark"
rust_benchmark="$oracle_root/capnproto-rust-$rust_commit/cargo-target/release/benchmark"
cpp_capnp="$oracle_root/capnproto-$cpp_commit/install/bin/capnp"
rust_plugin="$oracle_root/capnproto-rust-$rust_commit/install/bin/capnpc-rust"

mkdir -p -- "$output"

{
    printf 'format_version=1\n'
    printf 'generated_utc=%s\n' "$(date --utc +%Y-%m-%dT%H:%M:%SZ)"
    printf 'environment=Debian Trixie dev container under Docker Desktop\n'
    printf 'kernel=%s\n' "$(uname -srvmo)"
    printf 'cpu_model=%s\n' "$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n1)"
    printf 'logical_cpus=%s\n' "$(nproc)"
    printf 'memory_bytes=%s\n' "$(awk '/^MemTotal:/ { print $2 * 1024 }' /proc/meminfo)"
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'clang=%s\n' "$(clang++ --version | head -n1)"
    printf 'cpp_oracle_commit=%s\n' "$cpp_commit"
    printf 'cpp_oracle_version=%s\n' "$($cpp_capnp --version)"
    printf 'rust_oracle_commit=%s\n' "$rust_commit"
    printf 'rust_oracle_capnpc_version=0.27.0\n'
    printf 'rust_oracle_binary_sha256=%s\n' "$(sha256sum "$rust_plugin" | cut -d ' ' -f1)"
    printf 'warmups=%s\n' "$warmups"
    printf 'recorded_runs=%s\n' "$runs"
    printf 'timer=GNU date nanosecond wall clock around one child process\n'
} > "$output/metadata.txt"

printf 'implementation\tcase\tmode\tscratch\tcompression\titerations\trun\telapsed_ns\n' \
    > "$output/results.tsv"

workloads=(
    'carsales object no-reuse none 500'
    'carsales bytes reuse none 500'
    'carsales bytes reuse packed 500'
    'catrank bytes no-reuse none 100'
    'eval bytes no-reuse packed 10000'
)

run_workload() {
    local implementation=$1
    local case_name=$2
    local mode=$3
    local scratch=$4
    local compression=$5
    local iterations=$6
    local run=$7
    local start
    local finish

    start=$(date +%s%N)
    if [[ "$implementation" == cpp ]]; then
        "$cpp_dir/capnproto-$case_name" \
            "$mode" "$scratch" "$compression" "$iterations" >/dev/null
    else
        "$rust_benchmark" \
            "$case_name" "$mode" "$scratch" "$compression" "$iterations" \
            >/dev/null
    fi
    finish=$(date +%s%N)
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$implementation" "$case_name" "$mode" "$scratch" "$compression" \
        "$iterations" "$run" "$((finish - start))" >> "$output/results.tsv"
}

for implementation in cpp rust; do
    for workload in "${workloads[@]}"; do
        read -r case_name mode scratch compression iterations <<< "$workload"
        for ((warmup = 1; warmup <= warmups; warmup++)); do
            run_workload "$implementation" "$case_name" "$mode" "$scratch" \
                "$compression" "$iterations" "warmup-$warmup"
        done
        for ((run = 1; run <= runs; run++)); do
            run_workload "$implementation" "$case_name" "$mode" "$scratch" \
                "$compression" "$iterations" "$run"
        done
    done
done

awk -F '\t' '
    BEGIN {
        OFS = "\t";
        print "#implementation", "case", "mode", "scratch", "compression", "iterations", "runs", "mean_ns_per_iteration";
    }
    NR > 1 && $7 !~ /^warmup-/ {
        key = $1 FS $2 FS $3 FS $4 FS $5 FS $6;
        total[key] += $8;
        count[key] += 1;
    }
    END {
        for (key in total) {
            split(key, fields, FS);
            printf "%s\t%s\t%s\t%s\t%s\t%s\t%d\t%.2f\n", fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], count[key], total[key] / count[key] / fields[6];
        }
    }
' "$output/results.tsv" | sort > "$output/summary.tsv"

printf 'metadata=%s\nraw=%s\nsummary=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv"
