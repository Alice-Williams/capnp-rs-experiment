#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
rounds=${M30_BENCH_ROUNDS:-128}
workers=${M30_BENCH_WORKERS:-4}
samples=${M30_BENCH_SAMPLES:-7}
threshold=${M30_BENCH_THRESHOLD:-16384}
sizes=${M30_BENCH_SIZES:-"1024 8192 16384 65536 262144 1048576"}

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
if ! [[ "$rounds" =~ ^[1-9][0-9]*$ && "$workers" =~ ^[1-9][0-9]*$ && \
        "$samples" =~ ^[1-9][0-9]*$ && "$threshold" =~ ^[0-9]+$ ]]; then
    printf 'rounds, workers, samples, and threshold must be valid integers\n' >&2
    exit 1
fi
for size in $sizes; do
    if ! [[ "$size" =~ ^[1-9][0-9]*$ ]]; then
        printf 'invalid M30_BENCH_SIZES entry: %s\n' "$size" >&2
        exit 1
    fi
done

mkdir -p -- "$output"
cargo build --release -p capnp-message --example parallel_build
binary="$repo_root/target/release/examples/parallel_build"

{
    printf 'format_version=1\n'
    printf 'generated_utc=%s\n' "$(date --utc +%Y-%m-%dT%H:%M:%SZ)"
    printf 'environment=Debian Trixie dev container under Docker Desktop\n'
    printf 'kernel=%s\n' "$(uname -srvmo)"
    printf 'cpu_model=%s\n' "$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n1)"
    printf 'logical_cpus=%s\n' "$(nproc)"
    printf 'memory_bytes=%s\n' "$(awk '/^MemTotal:/ { print $2 * 1024 }' /proc/meminfo)"
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'base_git_commit=%s\n' "$(git -C "$repo_root" rev-parse HEAD)"
    printf 'parallel_builder_sha256=%s\n' "$(sha256sum "$repo_root/crates/capnp-message/src/parallel_builder.rs" | cut -d ' ' -f1)"
    printf 'benchmark_example_sha256=%s\n' "$(sha256sum "$repo_root/crates/capnp-message/examples/parallel_build.rs" | cut -d ' ' -f1)"
    printf 'rounds_per_item=%s\n' "$rounds"
    printf 'workers=%s\n' "$workers"
    printf 'samples_per_mode=%s\n' "$samples"
    printf 'parallel_item_threshold=%s\n' "$threshold"
    printf 'timer=Rust Instant; median includes arena creation, writing, and finalization\n'
} > "$output/metadata.txt"

first=1
for size in $sizes; do
    result=$("$binary" "$size" "$rounds" "$workers" "$samples" "$threshold")
    if [[ "$first" -eq 1 ]]; then
        printf '%s\n' "$result" > "$output/results.tsv"
        first=0
    else
        printf '%s\n' "$result" | tail -n1 >> "$output/results.tsv"
    fi
done

awk -F '\t' -v threshold="$threshold" '
    NR == 1 { next }
    $1 < threshold && ($4 != 1 || $6 > $5 * 1.05) {
        print "below-threshold partition or regression gate failed: " $0 > "/dev/stderr";
        exit 1;
    }
    $3 == 4 && $4 == 4 && $7 >= 2.5 { qualifying = 1 }
    END {
        if (!qualifying) {
            print "no four-worker qualifying build reached 2.5x" > "/dev/stderr";
            exit 1;
        }
    }
' "$output/results.tsv"

printf 'metadata=%s\nresults=%s\n' "$output/metadata.txt" "$output/results.tsv"
