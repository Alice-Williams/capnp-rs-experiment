#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-2}
runs=${BENCH_RUNS:-9}

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
for command_name in cargo clang++ git python3 sha256sum; do
    if ! command -v "$command_name" >/dev/null; then
        printf 'required command is unavailable: %s\n' "$command_name" >&2
        exit 1
    fi
done
if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
    printf 'refusing to benchmark a tracked dirty worktree\n' >&2
    exit 1
fi

bash "$repo_root/tools/build-oracle-benchmarks.sh" >/dev/null
cargo build \
    --locked \
    --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-native-benchmark \
    --release >/dev/null

cpp_dir="$oracle_root/capnproto-$cpp_commit/benchmark"
native_benchmark="$repo_root/target/release/capnp-native-benchmark"
native_commit=$(git -C "$repo_root" rev-parse HEAD)

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
    printf 'native_commit=%s\n' "$native_commit"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$native_benchmark" | cut -d ' ' -f1)"
    printf 'warmups=%s\nrecorded_runs=%s\n' "$warmups" "$runs"
    printf 'order=alternating C++/native first for every sample\n'
    printf 'timer=GNU date nanosecond wall clock around one child process\n'
    printf 'allocation=no-reuse on both implementations; native arena reset is unavailable\n'
} > "$output/metadata.txt"

printf 'implementation\tcase\tmode\tscratch\tcompression\titerations\trun\telapsed_ns\toutput_bytes\n' \
    > "$output/results.tsv"

workloads=(
    'carsales object no-reuse none 500'
    'carsales bytes no-reuse none 500'
    'carsales bytes no-reuse packed 500'
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
    local executable
    local start
    local finish
    local output_bytes

    if [[ "$implementation" == cpp ]]; then
        executable="$cpp_dir/capnproto-$case_name"
        start=$(date +%s%N)
        output_bytes=$("$executable" "$mode" "$scratch" "$compression" "$iterations")
        finish=$(date +%s%N)
    else
        start=$(date +%s%N)
        output_bytes=$("$native_benchmark" \
            "$case_name" "$mode" "$scratch" "$compression" "$iterations")
        finish=$(date +%s%N)
    fi
    [[ "$output_bytes" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$implementation" "$case_name" "$mode" "$scratch" "$compression" \
        "$iterations" "$run" "$((finish - start))" "$output_bytes" \
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
        read -r case_name mode scratch compression iterations <<< "$workload"
        run_workload "$first" "$case_name" "$mode" "$scratch" \
            "$compression" "$iterations" "$label"
        run_workload "$second" "$case_name" "$mode" "$scratch" \
            "$compression" "$iterations" "$label"
    done
}

for ((warmup = 1; warmup <= warmups; warmup++)); do
    run_sample "warmup-$warmup" "$warmup"
done
for ((run = 1; run <= runs; run++)); do
    run_sample "$run" "$((warmups + run))"
done

python3 "$repo_root/tools/summarize-native-cpp-benchmarks.py" \
    "$output/results.tsv" "$output/summary.tsv" "$output/comparison.tsv"

printf 'metadata=%s\nraw=%s\nsummary=%s\ncomparison=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv" \
    "$output/comparison.tsv"
