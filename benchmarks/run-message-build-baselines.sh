#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-2}
runs=${BENCH_RUNS:-9}
passes=${MESSAGE_BUILD_BENCH_PASSES:-100000}

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
cpp_build="$cpp_root/message-build-benchmark"
mkdir -p -- "$cpp_build"
"$cpp_root/install/bin/capnp" compile \
    -o"$cpp_root/install/bin/capnpc-c++":"$cpp_build" \
    --src-prefix="$repo_root/benchmarks/message-build" \
    "$repo_root/benchmarks/message-build/message_build.capnp"
clang++ -std=c++23 -O3 -DNDEBUG -pthread \
    -I"$cpp_root/install/include" -I"$cpp_build" \
    "$repo_root/benchmarks/message-build/cpp/main.c++" \
    "$cpp_build/message_build.capnp.c++" \
    -L"$cpp_root/install/lib" -lcapnp -lkj -o "$cpp_build/cpp-message-build"
cargo build --locked --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-native-benchmark --bin message_build --release >/dev/null

cpp_benchmark="$cpp_build/cpp-message-build"
native_benchmark="$repo_root/target/release/message_build"
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
    printf 'cpp_primitive=capnp::MallocMessageBuilder and capnp::AnyStruct::Builder\n'
    printf 'native_primitive=capnp_message::ExclusiveArena and StructBuilder\n'
    printf 'native_commit=%s\n' "$native_commit"
    printf 'cpp_binary_sha256=%s\n' "$(sha256sum "$cpp_benchmark" | cut -d ' ' -f1)"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$native_benchmark" | cut -d ' ' -f1)"
    printf 'warmups=%s\nrecorded_runs=%s\npasses=%s\n' "$warmups" "$runs" "$passes"
    printf 'shapes=direct:[3] words,far:[1,3] words,double-far:[1,2,2] words\n'
    printf 'timer=steady monotonic clock inside each binary\n'
} > "$output/metadata.txt"

printf 'implementation\tcase\tshape\tpasses\trun\telapsed_ns\tsemantic_checksum\twire_checksum\n' > "$output/results.tsv"
workloads=(
    'prepared direct'
    'fresh direct'
    'prepared far'
    'fresh far'
    'prepared double-far'
    'fresh double-far'
    'reuse direct'
)

run_workload() {
    local implementation=$1 case_name=$2 shape=$3 run=$4
    local executable measurement elapsed_ns semantic_checksum wire_checksum
    if [[ "$implementation" == cpp ]]; then executable=$cpp_benchmark; else executable=$native_benchmark; fi
    measurement=$("$executable" "$case_name" "$shape" "$passes")
    IFS=$'\t' read -r elapsed_ns semantic_checksum wire_checksum <<< "$measurement"
    [[ "$elapsed_ns" =~ ^[1-9][0-9]*$ && "$semantic_checksum" =~ ^[0-9]+$ && "$wire_checksum" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$implementation" "$case_name" \
        "$shape" "$passes" "$run" "$elapsed_ns" "$semantic_checksum" "$wire_checksum" \
        >> "$output/results.tsv"
}

run_sample() {
    local label=$1 ordinal=$2 first=cpp second=native
    if ((ordinal % 2 == 0)); then first=native; second=cpp; fi
    for workload in "${workloads[@]}"; do
        read -r case_name shape <<< "$workload"
        run_workload "$first" "$case_name" "$shape" "$label"
        run_workload "$second" "$case_name" "$shape" "$label"
    done
}

for ((warmup = 1; warmup <= warmups; warmup++)); do run_sample "warmup-$warmup" "$warmup"; done
for ((run = 1; run <= runs; run++)); do run_sample "$run" "$((warmups + run))"; done

python3 "$repo_root/tools/summarize-message-build-benchmarks.py" \
    "$output/results.tsv" "$output/summary.tsv" "$output/comparison.tsv" "$output/incremental.tsv"
awk -F '\t' 'NR == 1 { next } !($1 FS $2 FS $3 in semantic) { semantic[$1 FS $2 FS $3] = $7; wire[$1 FS $2 FS $3] = $8; next } $7 != semantic[$1 FS $2 FS $3] || $8 != wire[$1 FS $2 FS $3] { exit 1 }' "$output/results.tsv"
awk -F '\t' 'NR == 1 { next } !($2 FS $3 in semantic) { semantic[$2 FS $3] = $7; next } $7 != semantic[$2 FS $3] { exit 1 }' "$output/results.tsv"
printf 'metadata=%s\nraw=%s\nsummary=%s\ncomparison=%s\nincremental=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv" \
    "$output/comparison.tsv" "$output/incremental.tsv"
