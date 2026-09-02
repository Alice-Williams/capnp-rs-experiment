#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}

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
if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
    printf 'refusing to profile a tracked dirty worktree\n' >&2
    exit 1
fi

cargo build \
    --locked \
    --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-native-benchmark \
    --release >/dev/null

benchmark="$repo_root/target/release/capnp-native-benchmark"
temporary=$(mktemp)
trap 'rm -f -- "$temporary"' EXIT
mkdir -p -- "$output"

{
    printf 'format_version=1\n'
    printf 'generated_utc=%s\n' "$(date --utc +%Y-%m-%dT%H:%M:%SZ)"
    printf 'native_commit=%s\n' "$(git -C "$repo_root" rev-parse HEAD)"
    printf 'native_binary_sha256=%s\n' "$(sha256sum "$benchmark" | cut -d ' ' -f1)"
    printf 'environment=Debian Trixie dev container under Docker Desktop\n'
    printf 'kernel=%s\n' "$(uname -srvmo)"
    printf 'cpu_model=%s\n' "$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -n1)"
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'method=Instant around seven native phases; wall timer around child process\n'
} > "$output/metadata.txt"
printf 'case\tmode\tcompression\titerations\tphase\ttotal_ns\tns_per_iteration\n' \
    > "$output/phases.tsv"
printf 'case\tmode\tcompression\titerations\twall_ns\tphase_ns\tunattributed_ns\n' \
    > "$output/totals.tsv"

workloads=(
    'carsales object none 2000'
    'carsales bytes none 2000'
    'carsales bytes packed 2000'
    'catrank bytes none 500'
    'eval bytes packed 50000'
)

for workload in "${workloads[@]}"; do
    read -r case_name mode compression iterations <<< "$workload"
    "$benchmark" "$case_name" "$mode" no-reuse "$compression" "$iterations" >/dev/null
    started=$(date +%s%N)
    CAPNP_BENCH_PROFILE=1 "$benchmark" \
        "$case_name" "$mode" no-reuse "$compression" "$iterations" \
        >/dev/null 2> "$temporary"
    finished=$(date +%s%N)
    test "$(wc -l < "$temporary")" -eq 8
    grep -Fx $'phase\ttotal_ns\tns_per_iteration' "$temporary"
    awk -F '\t' -v case_name="$case_name" -v mode="$mode" \
        -v compression="$compression" -v iterations="$iterations" \
        'NR > 1 { print case_name "\t" mode "\t" compression "\t" iterations "\t" $0 }' \
        "$temporary" >> "$output/phases.tsv"
    phase_ns=$(awk -F '\t' 'NR > 1 { total += $2 } END { print total }' "$temporary")
    wall_ns=$((finished - started))
    unattributed_ns=$((wall_ns - phase_ns))
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$case_name" "$mode" "$compression" "$iterations" "$wall_ns" \
        "$phase_ns" "$unattributed_ns" >> "$output/totals.tsv"
done

printf 'metadata=%s\nphases=%s\ntotals=%s\n' \
    "$output/metadata.txt" "$output/phases.tsv" "$output/totals.tsv"
