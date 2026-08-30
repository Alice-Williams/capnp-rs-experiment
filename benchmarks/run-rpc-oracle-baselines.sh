#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
rust_commit=2228b71e55cee819c30450bb9bfd9c1f6a722429
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-1}
runs=${BENCH_RUNS:-5}
iterations=${RPC_BENCH_ITERS:-10000}

if [[ "$output" == "--syntax-check-only" ]]; then
    exit 0
fi
if [[ -z "$output" || ! -d "$output" ]]; then
    printf 'usage: %s EXISTING_BASELINE_DIRECTORY\n' "$0" >&2
    exit 1
fi
for result_file in rpc-metadata.txt rpc-results.tsv rpc-summary.tsv; do
    if [[ -e "$output/$result_file" ]]; then
        printf 'refusing to overwrite benchmark output: %s\n' "$output/$result_file" >&2
        exit 1
    fi
done
if ! [[ "$warmups" =~ ^[0-9]+$ && "$runs" =~ ^[1-9][0-9]*$ \
    && "$iterations" =~ ^[1-9][0-9]*$ ]]; then
    printf 'benchmark counts must be non-negative/positive integers\n' >&2
    exit 1
fi

bash "$repo_root/tools/build-rpc-oracle-benchmarks.sh" >/dev/null

cpp_benchmark="$oracle_root/capnproto-$cpp_commit/rpc-benchmark/cpp-rpc-benchmark"
rust_benchmark="$oracle_root/capnproto-rust-$rust_commit/rpc-benchmark/target/release/capnp-oracle-rpc-benchmark"

{
    printf 'format_version=1\n'
    printf 'generated_utc=%s\n' "$(date --utc +%Y-%m-%dT%H:%M:%SZ)"
    printf 'schema=benchmarks/rpc/ping.capnp\n'
    printf 'schema_sha256=bd64cf8c596d3b2644af04e3b9417349a339928268134c8ba1d4c62f0512e9ba\n'
    printf 'pattern=one bootstrap followed by sequential UInt64 request/reply calls\n'
    printf 'execution=single-thread event loop with in-memory bidirectional byte stream\n'
    printf 'cpp_transport=KJ newTwoWayPipe\n'
    printf 'rust_transport=Tokio duplex 1 MiB buffer\n'
    printf 'cpp_oracle_commit=%s\n' "$cpp_commit"
    printf 'rust_oracle_commit=%s\n' "$rust_commit"
    printf 'warmups=%s\nrecorded_runs=%s\niterations=%s\n' \
        "$warmups" "$runs" "$iterations"
} > "$output/rpc-metadata.txt"

printf 'implementation\titerations\trun\telapsed_ns\tchecksum\n' \
    > "$output/rpc-results.tsv"

run_workload() {
    local implementation=$1
    local run=$2
    local executable
    local start
    local finish
    local checksum

    if [[ "$implementation" == cpp ]]; then
        executable=$cpp_benchmark
    else
        executable=$rust_benchmark
    fi
    start=$(date +%s%N)
    checksum=$("$executable" "$iterations")
    finish=$(date +%s%N)
    [[ "$checksum" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\n' \
        "$implementation" "$iterations" "$run" "$((finish - start))" \
        "$checksum" >> "$output/rpc-results.tsv"
}

for implementation in cpp rust; do
    for ((warmup = 1; warmup <= warmups; warmup++)); do
        run_workload "$implementation" "warmup-$warmup"
    done
    for ((run = 1; run <= runs; run++)); do
        run_workload "$implementation" "$run"
    done
done

awk -F '\t' '
    BEGIN { OFS = "\t"; print "#implementation", "iterations", "runs", "mean_ns_per_call"; }
    NR > 1 && $3 !~ /^warmup-/ {
        total[$1] += $4;
        count[$1] += 1;
        iterations[$1] = $2;
    }
    END {
        for (implementation in total) {
            printf "%s\t%s\t%d\t%.2f\n", implementation, iterations[implementation], count[implementation], total[implementation] / count[implementation] / iterations[implementation];
        }
    }
' "$output/rpc-results.tsv" | sort > "$output/rpc-summary.tsv"

printf 'metadata=%s\nraw=%s\nsummary=%s\n' \
    "$output/rpc-metadata.txt" "$output/rpc-results.tsv" "$output/rpc-summary.tsv"
