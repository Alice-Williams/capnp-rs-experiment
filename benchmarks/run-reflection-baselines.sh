#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
warmups=${BENCH_WARMUPS:-2}
runs=${BENCH_RUNS:-9}
passes=${REFLECTION_BENCH_PASSES:-100000}

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
cd "$repo_root"
bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
cpp_build="$cpp_root/reflection-benchmark"
mkdir -p -- "$cpp_build"
"$cpp_root/install/bin/capnp" compile \
    -I"$cpp_root/install/include" \
    -o"$cpp_root/install/bin/capnpc-c++:$cpp_build" \
    conformance/schemas/wire-fixture.capnp
clang++ -std=c++23 -O3 -DNDEBUG \
    -I"$cpp_root/install/include" -I"$cpp_build" \
    "$repo_root/benchmarks/reflection/cpp/main.c++" \
    "$cpp_build/conformance/schemas/wire-fixture.capnp.c++" \
    -L"$cpp_root/install/lib" -lcapnp-rpc -lcapnp -lkj-async -lkj \
    -o "$cpp_build/cpp-reflection"
cargo build --locked --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-generated-fixture --example reflection_benchmark --release >/dev/null

cpp_benchmark="$cpp_build/cpp-reflection"
native_benchmark="$repo_root/target/release/examples/reflection_benchmark"
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
    printf 'schema=conformance/schemas/wire-fixture.capnp\n'
    printf 'fixture=wire-unpacked.bin\n'
    printf 'fields=uint8Value,uint16Value,uint32Value,uint64Value,text,data,uint16s,node,structs,nestedLists\n'
    printf 'native_commit=%s\n' "$native_commit"
    printf 'cpp_binary_sha256=%s\n' "$(sha256sum "$cpp_benchmark" | cut -d ' ' -f1)"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$native_benchmark" | cut -d ' ' -f1)"
    printf 'warmups=%s\nrecorded_runs=%s\npasses=%s\n' "$warmups" "$runs" "$passes"
    printf 'timer=steady monotonic clock inside each binary\n'
} > "$output/metadata.txt"

printf 'implementation\tcase\tpasses\trun\telapsed_ns\tchecksum\n' > "$output/results.tsv"
workloads=(
    schema-name schema-index dynamic-name dynamic-index dynamic-field
    dynamic-blobs-borrowed dynamic-blobs-owned
    dynamic-primitive-list dynamic-nested-struct dynamic-struct-list dynamic-nested-list
    dynamic-enum dynamic-default dynamic-union-active
    dynamic-union-unknown
)

run_workload() {
    local implementation=$1 case_name=$2 run=$3 executable measurement elapsed_ns checksum
    if [[ "$implementation" == cpp ]]; then executable=$cpp_benchmark; else executable=$native_benchmark; fi
    if [[ "$implementation" == cpp ]]; then
        measurement=$("$executable" "$case_name" "$passes" "$fixture")
    else
        measurement=$("$executable" "$case_name" "$passes")
    fi
    IFS=$'\t' read -r elapsed_ns checksum <<< "$measurement"
    [[ "$elapsed_ns" =~ ^[1-9][0-9]*$ && "$checksum" =~ ^[0-9]+$ ]]
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$implementation" "$case_name" \
        "$passes" "$run" "$elapsed_ns" "$checksum" >> "$output/results.tsv"
}

run_sample() {
    local label=$1 ordinal=$2 first=cpp second=native
    if ((ordinal % 2 == 0)); then first=native; second=cpp; fi
    for workload in "${workloads[@]}"; do
        run_workload "$first" "$workload" "$label"
        run_workload "$second" "$workload" "$label"
    done
}

for ((warmup = 1; warmup <= warmups; warmup++)); do run_sample "warmup-$warmup" "$warmup"; done
for ((run = 1; run <= runs; run++)); do run_sample "$run" "$((warmups + run))"; done

python3 "$repo_root/tools/summarize-reflection-benchmarks.py" \
    "$output/results.tsv" "$output/summary.tsv" "$output/comparison.tsv"
awk -F '\t' '
  NR == 1 { next }
  !($2 in checksum) { checksum[$2] = $6; next }
  $6 != checksum[$2] { exit 1 }
' "$output/results.tsv"
printf 'metadata=%s\nraw=%s\nsummary=%s\ncomparison=%s\n' \
    "$output/metadata.txt" "$output/results.tsv" "$output/summary.tsv" \
    "$output/comparison.tsv"
