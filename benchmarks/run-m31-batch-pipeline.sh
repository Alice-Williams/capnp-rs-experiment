#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
words=${M31_BENCH_WORDS:-1024}
rounds=${M31_BENCH_ROUNDS:-128}
workers=${M31_BENCH_WORKERS:-4}
samples=${M31_BENCH_SAMPLES:-31}
threshold=${M31_BENCH_THRESHOLD:-2}
counts=${M31_BENCH_COUNTS:-"1 2 4 8 16 32 64"}

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
if ! [[ "$words" =~ ^[1-9][0-9]*$ && "$rounds" =~ ^[1-9][0-9]*$ && \
        "$workers" =~ ^[1-9][0-9]*$ && "$samples" =~ ^[1-9][0-9]*$ && \
        "$threshold" =~ ^[1-9][0-9]*$ ]] || (( samples % 2 == 0 )); then
    printf 'words, rounds, workers, and threshold must be positive; samples must be positive and odd\n' >&2
    exit 1
fi
for count in $counts; do
    if ! [[ "$count" =~ ^[1-9][0-9]*$ ]]; then
        printf 'invalid M31_BENCH_COUNTS entry: %s\n' "$count" >&2
        exit 1
    fi
done

mkdir -p -- "$output"
cargo build --release -p capnp-async --example batch_pipeline
binary="$repo_root/target/release/examples/batch_pipeline"

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
    printf 'batch_module_sha256=%s\n' "$(sha256sum "$repo_root/crates/capnp-async/src/batch.rs" | cut -d ' ' -f1)"
    printf 'benchmark_example_sha256=%s\n' "$(sha256sum "$repo_root/crates/capnp-async/examples/batch_pipeline.rs" | cut -d ' ' -f1)"
    printf 'words_per_message=%s\n' "$words"
    printf 'rounds_per_word=%s\n' "$rounds"
    printf 'workers=%s\n' "$workers"
    printf 'samples_per_mode=%s\n' "$samples"
    printf 'parallel_message_threshold=%s\n' "$threshold"
    printf 'timer=Rust Instant; median includes bounded scheduling, transform, pack, and ordered emit\n'
} > "$output/metadata.txt"

first=1
for count in $counts; do
    result=$("$binary" "$count" "$words" "$rounds" "$workers" "$samples" "$threshold")
    if [[ "$first" -eq 1 ]]; then
        printf '%s\n' "$result" > "$output/results.tsv"
        first=0
    else
        printf '%s\n' "$result" | tail -n1 >> "$output/results.tsv"
    fi
done

awk -F '\t' '
    NR == 1 { next }
    $1 == 1 && ($5 != 1 || $7 > $6 * 1.05) {
        print "single-message no-pool/regression gate failed: " $0 > "/dev/stderr";
        exit 1;
    }
    $1 >= 16 && $4 == 4 && $5 == 4 && $8 >= 3.0 { qualifying = 1 }
    END {
        if (!qualifying) {
            print "no qualifying four-worker batch reached 3.0x" > "/dev/stderr";
            exit 1;
        }
    }
' "$output/results.tsv"

printf 'metadata=%s\nresults=%s\n' "$output/metadata.txt" "$output/results.tsv"
