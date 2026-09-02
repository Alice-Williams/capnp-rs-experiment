#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-2}
runs=${BENCH_RUNS:-9}
iterations=${RPC_BENCH_ITERS:-10000}

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
if ! [[ "$warmups" =~ ^[0-9]+$ && "$runs" =~ ^[1-9][0-9]*$ \
    && "$iterations" =~ ^[1-9][0-9]*$ ]]; then
    printf 'benchmark counts must be non-negative/positive integers\n' >&2
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

bash "$repo_root/tools/build-rpc-oracle-benchmarks.sh" >/dev/null
cargo build --locked --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-native-benchmark --bin native_rpc --release >/dev/null

cpp_benchmark="$oracle_root/capnproto-$cpp_commit/rpc-benchmark/cpp-rpc-benchmark"
native_benchmark="$repo_root/target/release/native_rpc"
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
    printf 'schema=benchmarks/rpc/ping.capnp\n'
    printf 'schema_sha256=bd64cf8c596d3b2644af04e3b9417349a339928268134c8ba1d4c62f0512e9ba\n'
    printf 'pattern=one bootstrap followed by sequential UInt64 request/reply calls\n'
    printf 'cpp_transport=KJ in-memory bidirectional byte pipe\n'
    printf 'native_transport=direct in-memory owned-message envelopes\n'
    printf 'transport_caveat=native path does not encode or parse standard framing\n'
    printf 'cpp_oracle_commit=%s\n' "$cpp_commit"
    printf 'native_commit=%s\n' "$native_commit"
    printf 'cpp_binary_sha256=%s\n' "$(sha256sum "$cpp_benchmark" | cut -d ' ' -f1)"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$native_benchmark" | cut -d ' ' -f1)"
    printf 'warmups=%s\nrecorded_runs=%s\niterations=%s\n' \
        "$warmups" "$runs" "$iterations"
    printf 'order=alternating C++/native first for every sample\n'
    printf 'timer=GNU date nanosecond wall clock around one child process\n'
} > "$output/metadata.txt"

printf 'implementation\ttransport\titerations\trun\telapsed_ns\tchecksum\n' \
    > "$output/results.tsv"

run_workload() {
    local implementation=$1
    local run=$2
    local executable
    local transport
    local start
    local finish
    local checksum
    if [[ "$implementation" == cpp ]]; then
        executable=$cpp_benchmark
        transport=byte-pipe
    else
        executable=$native_benchmark
        transport=message-envelopes
    fi
    start=$(date +%s%N)
    checksum=$("$executable" "$iterations")
    finish=$(date +%s%N)
    [[ "$checksum" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$implementation" "$transport" \
        "$iterations" "$run" "$((finish - start))" "$checksum" \
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
    run_workload "$first" "$label"
    run_workload "$second" "$label"
}

for ((warmup = 1; warmup <= warmups; warmup++)); do
    run_sample "warmup-$warmup" "$warmup"
done
for ((run = 1; run <= runs; run++)); do
    run_sample "$run" "$((warmups + run))"
done

python3 "$repo_root/tools/summarize-native-cpp-rpc.py" \
    "$output/results.tsv" "$output/summary.tsv" "$output/comparison.tsv"

printf 'metadata=%s\nraw=%s\nsummary=%s\ncomparison=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv" \
    "$output/comparison.tsv"
