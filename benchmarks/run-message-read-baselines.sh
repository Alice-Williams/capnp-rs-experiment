#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-2}
runs=${BENCH_RUNS:-9}
passes=${MESSAGE_READ_BENCH_PASSES:-100000}

if [[ "$output" == "--syntax-check-only" ]]; then exit 0; fi
if [[ -z "$output" || -e "$output" ]]; then
    printf 'usage: %s NEW_OUTPUT_DIRECTORY\n' "$0" >&2
    exit 1
fi
if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
    printf 'refusing to benchmark a tracked dirty worktree\n' >&2
    exit 1
fi

cpp_root="$oracle_root/capnproto-$cpp_commit"
bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
cpp_build="$cpp_root/message-read-benchmark"
mkdir -p -- "$cpp_build"
clang++ -std=c++23 -O3 -DNDEBUG -pthread \
    -I"$cpp_root/install/include" "$repo_root/benchmarks/message-read/cpp/main.c++" \
    -L"$cpp_root/install/lib" -lcapnp -lkj -o "$cpp_build/cpp-message-read"
cargo build --locked --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-native-benchmark --bin message_read --release >/dev/null

cpp_benchmark="$cpp_build/cpp-message-read"
native_benchmark="$repo_root/target/release/message_read"
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
    printf 'cpp_primitive=capnp::FlatArrayMessageReader and capnp::AnyStruct::Reader\n'
    printf 'native_commit=%s\n' "$native_commit"
    printf 'cpp_binary_sha256=%s\n' "$(sha256sum "$cpp_benchmark" | cut -d ' ' -f1)"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$native_benchmark" | cut -d ' ' -f1)"
    printf 'warmups=%s\nrecorded_runs=%s\npasses=%s\n' "$warmups" "$runs" "$passes"
    printf 'segment_shapes=1:[8],2:[3,5],64:[1x64] words\n'
    printf 'timer=steady monotonic clock inside each binary\n'
} > "$output/metadata.txt"

printf 'implementation\tcase\tsegments\tpasses\trun\telapsed_ns\tchecksum\n' > "$output/results.tsv"
workloads=(
    'framing 1'
    'framing 2'
    'framing 64'
    'root 1'
    'root 2'
    'root 64'
)

run_workload() {
    local implementation=$1 case_name=$2 segments=$3 run=$4
    local executable measurement elapsed_ns checksum
    if [[ "$implementation" == cpp ]]; then executable=$cpp_benchmark; else executable=$native_benchmark; fi
    measurement=$("$executable" "$case_name" "$segments" "$passes")
    IFS=$'\t' read -r elapsed_ns checksum <<< "$measurement"
    [[ "$elapsed_ns" =~ ^[1-9][0-9]*$ && "$checksum" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$implementation" "$case_name" \
        "$segments" "$passes" "$run" "$elapsed_ns" "$checksum" >> "$output/results.tsv"
}

run_sample() {
    local label=$1 ordinal=$2 first=cpp second=native
    if ((ordinal % 2 == 0)); then first=native; second=cpp; fi
    for workload in "${workloads[@]}"; do
        read -r case_name segments <<< "$workload"
        run_workload "$first" "$case_name" "$segments" "$label"
        run_workload "$second" "$case_name" "$segments" "$label"
    done
}

for ((warmup = 1; warmup <= warmups; warmup++)); do run_sample "warmup-$warmup" "$warmup"; done
for ((run = 1; run <= runs; run++)); do run_sample "$run" "$((warmups + run))"; done

python3 "$repo_root/tools/summarize-message-read-benchmarks.py" \
    "$output/results.tsv" "$output/summary.tsv" "$output/comparison.tsv" "$output/incremental.tsv"
awk -F '\t' 'NR == 1 { next } !($2 FS $3 in sum) { sum[$2 FS $3] = $7; next } $7 != sum[$2 FS $3] { exit 1 }' "$output/results.tsv"
printf 'metadata=%s\nraw=%s\nsummary=%s\ncomparison=%s\nincremental=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv" \
    "$output/comparison.tsv" "$output/incremental.tsv"
