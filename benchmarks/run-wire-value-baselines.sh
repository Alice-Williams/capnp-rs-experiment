#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-2}
runs=${BENCH_RUNS:-9}
words=${WIRE_BENCH_WORDS:-4096}
passes=${WIRE_BENCH_PASSES:-10000}

if [[ "$output" == "--syntax-check-only" ]]; then
    exit 0
fi
if [[ -z "$output" || -e "$output" ]]; then
    printf 'usage: %s NEW_OUTPUT_DIRECTORY\n' "$0" >&2
    exit 1
fi
if ! [[ "$warmups" =~ ^[0-9]+$ && "$runs" =~ ^[1-9][0-9]*$ \
    && "$words" =~ ^[1-9][0-9]*$ && "$passes" =~ ^[1-9][0-9]*$ ]]; then
    printf 'benchmark counts must be non-negative/positive integers\n' >&2
    exit 1
fi
if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
    printf 'refusing to benchmark a tracked dirty worktree\n' >&2
    exit 1
fi

cpp_source="$oracle_root/capnproto-$cpp_commit/source"
if [[ ! -f "$cpp_source/c++/src/capnp/endian.h" ]]; then
    bash "$repo_root/tools/build-oracle-benchmarks.sh" >/dev/null
fi
cpp_build="$oracle_root/capnproto-$cpp_commit/wire-value-benchmark"
mkdir -p -- "$cpp_build"
clang++ -O3 -DNDEBUG -std=c++23 -I "$cpp_source/c++/src" \
    "$repo_root/benchmarks/wire/cpp/main.c++" -o "$cpp_build/cpp-wire-value-benchmark"
cargo build --locked --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-wire --example wire_value_benchmark --release >/dev/null

cpp_benchmark="$cpp_build/cpp-wire-value-benchmark"
native_benchmark="$repo_root/target/release/examples/wire_value_benchmark"
native_commit=$(git -C "$repo_root" rev-parse HEAD)
mkdir -p -- "$output"
{
    printf 'format_version=1\n'
    printf 'generated_utc=%s\n' "$(date --utc +%Y-%m-%dT%H:%M:%SZ)"
    printf 'environment=Debian Trixie dev container under Docker Desktop\n'
    printf 'kernel=%s\n' "$(uname -srvmo)"
    printf 'cpu_model=%s\n' "$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n1)"
    printf 'logical_cpus=%s\n' "$(nproc)"
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'clang=%s\n' "$(clang++ --version | head -n1)"
    printf 'cpp_oracle_commit=%s\n' "$cpp_commit"
    printf 'cpp_primitive=capnp::_::WireValue<uint64_t> from capnp/endian.h\n'
    printf 'native_commit=%s\n' "$native_commit"
    printf 'cpp_binary_sha256=%s\n' "$(sha256sum "$cpp_benchmark" | cut -d ' ' -f1)"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$native_benchmark" | cut -d ' ' -f1)"
    printf 'warmups=%s\nrecorded_runs=%s\nwords=%s\npasses=%s\n' \
        "$warmups" "$runs" "$words" "$passes"
    printf 'order=alternating C++/native first for every sample\n'
    printf 'timer=steady monotonic clock inside each binary around the measured primitive loop\n'
} > "$output/metadata.txt"

printf 'implementation\tcase\twords\tpasses\trun\telapsed_ns\tchecksum\n' \
    > "$output/results.tsv"

workloads=(
    'checked-read read'
    'word-read read'
    'validated-read read'
    'word-array-read read'
    'checked-write write'
    'word-write write'
    'validated-write write'
    'word-array-write write'
)

run_workload() {
    local implementation=$1
    local case_name=$2
    local cpp_mode=$3
    local run=$4
    local measurement
    local elapsed_ns
    local checksum
    if [[ "$implementation" == cpp ]]; then
        measurement=$("$cpp_benchmark" "$cpp_mode" "$words" "$passes")
    else
        measurement=$("$native_benchmark" "$case_name" "$words" "$passes")
    fi
    IFS=$'\t' read -r elapsed_ns checksum <<< "$measurement"
    [[ "$elapsed_ns" =~ ^[1-9][0-9]*$ ]]
    [[ "$checksum" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$implementation" "$case_name" \
        "$words" "$passes" "$run" "$elapsed_ns" "$checksum" \
        >> "$output/results.tsv"
}

run_sample() {
    local label=$1
    local ordinal=$2
    local first=cpp
    local second=native
    if ((ordinal % 2 == 0)); then
        first=native
        second=cpp
    fi
    for workload in "${workloads[@]}"; do
        read -r case_name cpp_mode <<< "$workload"
        run_workload "$first" "$case_name" "$cpp_mode" "$label"
        run_workload "$second" "$case_name" "$cpp_mode" "$label"
    done
}

for ((warmup = 1; warmup <= warmups; warmup++)); do
    run_sample "warmup-$warmup" "$warmup"
done
for ((run = 1; run <= runs; run++)); do
    run_sample "$run" "$((warmups + run))"
done

python3 "$repo_root/tools/summarize-wire-value-benchmarks.py" \
    "$output/results.tsv" "$output/summary.tsv" "$output/comparison.tsv"

awk -F '\t' '
    NR == 1 { next }
    !($2 in checksum) { checksum[$2] = $7; next }
    $7 != checksum[$2] { exit 1 }
' "$output/results.tsv"

printf 'metadata=%s\nraw=%s\nsummary=%s\ncomparison=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv" \
    "$output/comparison.tsv"
