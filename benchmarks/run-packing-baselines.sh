#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-2}
runs=${BENCH_RUNS:-9}
words=${PACKING_BENCH_WORDS:-4096}
passes=${PACKING_BENCH_PASSES:-1000}

if [[ "$output" == "--syntax-check-only" ]]; then exit 0; fi
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

cpp_root="$oracle_root/capnproto-$cpp_commit"
bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
cpp_build="$cpp_root/packing-benchmark"
mkdir -p -- "$cpp_build"
clang++ -std=c++23 -O3 -DNDEBUG -pthread \
    -I"$cpp_root/install/include" "$repo_root/benchmarks/packing/cpp/main.c++" \
    -L"$cpp_root/install/lib" -lcapnp -lkj -o "$cpp_build/cpp-packing-benchmark"
cargo build --locked --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-io --example packing_benchmark --release >/dev/null

cpp_benchmark="$cpp_build/cpp-packing-benchmark"
native_benchmark="$repo_root/target/release/examples/packing_benchmark"
fixture="$repo_root/conformance/fixtures/cpp/$cpp_commit/wire-unpacked.bin"
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
    printf 'cpp_primitive=capnp::_::PackedOutputStream and capnp::_::PackedInputStream over in-memory KJ streams\n'
    printf 'native_primitive=capnp_io::pack and capnp_io::unpack\n'
    printf 'native_stream_decode=PackedDecoder with caller-provided exact output capacity\n'
    printf 'cpp_stream_decode=PackedInputStream with exact output array; pull reads may bypass the input buffer for raw runs\n'
    printf 'native_commit=%s\n' "$native_commit"
    printf 'cpp_binary_sha256=%s\n' "$(sha256sum "$cpp_benchmark" | cut -d ' ' -f1)"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$native_benchmark" | cut -d ' ' -f1)"
    printf 'warmups=%s\nrecorded_runs=%s\nwords=%s\npasses=%s\n' \
        "$warmups" "$runs" "$words" "$passes"
    printf 'cases=24\n'
    printf 'shapes=long zero runs, long raw runs, deterministic mixed sparse words, repeated pinned C++ wire fixture\n'
    printf 'stream_chunks=zero/raw 256 words; mixed 8 words; realistic 100 words; decode input feed 1025 bytes\n'
    printf 'allocation=fresh output per pass; C++ packed VectorOutputStream starts at 8 bytes\n'
    printf 'order=alternating C++/native first for every sample\n'
    printf 'timer=steady monotonic clock inside each binary around complete operation loop\n'
} > "$output/metadata.txt"

printf 'implementation\tcase\tshape\twords\tpasses\trun\telapsed_ns\tchecksum\n' > "$output/results.tsv"
modes=(copy-unpacked copy-packed pack unpack pack-stream unpack-stream)
shapes=(zero raw mixed realistic)

run_workload() {
    local implementation=$1 case_name=$2 shape=$3 run=$4
    local measurement elapsed_ns checksum
    if [[ "$implementation" == cpp ]]; then
        measurement=$("$cpp_benchmark" "$case_name" "$shape" "$words" "$passes" "$fixture")
    else
        measurement=$("$native_benchmark" "$case_name" "$shape" "$words" "$passes")
    fi
    IFS=$'\t' read -r elapsed_ns checksum <<< "$measurement"
    [[ "$elapsed_ns" =~ ^[1-9][0-9]*$ && "$checksum" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$implementation" "$case_name" \
        "$shape" "$words" "$passes" "$run" "$elapsed_ns" "$checksum" >> "$output/results.tsv"
}

run_sample() {
    local label=$1 ordinal=$2 first=cpp second=native
    if ((ordinal % 2 == 0)); then first=native; second=cpp; fi
    for mode in "${modes[@]}"; do
        for shape in "${shapes[@]}"; do
            run_workload "$first" "$mode" "$shape" "$label"
            run_workload "$second" "$mode" "$shape" "$label"
        done
    done
}

for ((warmup = 1; warmup <= warmups; warmup++)); do run_sample "warmup-$warmup" "$warmup"; done
for ((run = 1; run <= runs; run++)); do run_sample "$run" "$((warmups + run))"; done

python3 "$repo_root/tools/summarize-packing-benchmarks.py" \
    "$output/results.tsv" "$output/summary.tsv" "$output/comparison.tsv" "$output/incremental.tsv"
awk -F '\t' 'NR == 1 { next } !($2 FS $3 in sum) { sum[$2 FS $3] = $8; next } $8 != sum[$2 FS $3] { exit 1 }' "$output/results.tsv"
printf 'metadata=%s\nraw=%s\nsummary=%s\ncomparison=%s\nincremental=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv" \
    "$output/comparison.tsv" "$output/incremental.tsv"
