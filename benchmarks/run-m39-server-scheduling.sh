#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
jobs=${M39_BENCH_JOBS:-64}
rounds=${M39_BENCH_ROUNDS:-5000000}
samples=${M39_BENCH_SAMPLES:-7}

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
if ! [[ "$jobs" =~ ^[1-9][0-9]*$ && "$rounds" =~ ^[1-9][0-9]*$ && \
        "$samples" =~ ^[1-9][0-9]*$ ]] || (( samples % 2 == 0 )); then
    printf 'jobs and rounds must be positive; samples must be positive and odd\n' >&2
    exit 1
fi

mkdir -p -- "$output"
cargo build --release -p capnp-rpc --example server_scheduling
binary="$repo_root/target/release/examples/server_scheduling"

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
    printf 'scheduler_module_sha256=%s\n' "$(sha256sum "$repo_root/crates/capnp-rpc/src/scheduler.rs" | cut -d ' ' -f1)"
    printf 'benchmark_example_sha256=%s\n' "$(sha256sum "$repo_root/crates/capnp-rpc/examples/server_scheduling.rs" | cut -d ' ' -f1)"
    printf 'jobs=%s\n' "$jobs"
    printf 'rounds_per_job=%s\n' "$rounds"
    printf 'samples_per_configuration=%s\n' "$samples"
    printf 'timer=Rust Instant; latency begins before same-connection dispatch burst and ends inside each handler\n'
    printf 'fairness=max consecutive completions for one of four evenly loaded keys; lower is fairer\n'
} > "$output/metadata.txt"

printf 'policy\tworkers\tjobs\trounds\telapsed_ns\tjobs_per_second\tp50_us\tp99_us\tmax_key_run\n' \
  > "$output/raw.tsv"
for configuration in 'concurrent 1' 'concurrent 4' 'serial 4' 'keyed 4'; do
    set -- $configuration
    "$binary" "$1" "$2" "$jobs" "$rounds" >/dev/null
    for ((sample = 0; sample < samples; sample += 1)); do
        "$binary" "$1" "$2" "$jobs" "$rounds" >> "$output/raw.tsv"
    done
done

middle=$((samples / 2 + 1))
median_field() {
    local policy=$1 workers=$2 field=$3
    awk -F '\t' -v p="$policy" -v w="$workers" -v f="$field" \
      'NR > 1 && $1 == p && $2 == w { print $f }' "$output/raw.tsv" \
      | sort -n | sed -n "${middle}p"
}

printf 'policy\tworkers\tmedian_jobs_per_second\tmedian_p50_us\tmedian_p99_us\tmedian_max_key_run\n' \
  > "$output/summary.tsv"
for configuration in 'concurrent 1' 'concurrent 4' 'serial 4' 'keyed 4'; do
    set -- $configuration
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$1" "$2" \
      "$(median_field "$1" "$2" 6)" \
      "$(median_field "$1" "$2" 7)" \
      "$(median_field "$1" "$2" 8)" \
      "$(median_field "$1" "$2" 9)" >> "$output/summary.tsv"
done

one=$(median_field concurrent 1 6)
four=$(median_field concurrent 4 6)
ratio=$(awk -v one="$one" -v four="$four" 'BEGIN { printf "%.3f", four / one }')
printf 'concurrent_four_to_one_ratio=%s\n' "$ratio" >> "$output/metadata.txt"
awk -v ratio="$ratio" 'BEGIN { if (ratio < 3.0) exit 1 }'

printf 'metadata=%s\nraw=%s\nsummary=%s\n' \
  "$output/metadata.txt" "$output/raw.tsv" "$output/summary.tsv"
