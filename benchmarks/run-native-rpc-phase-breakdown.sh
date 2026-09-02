#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${1:-}
iterations=${RPC_PROFILE_ITERS:-100000}

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
if ! [[ "$iterations" =~ ^[1-9][0-9]*$ ]]; then
    printf 'RPC_PROFILE_ITERS must be a positive integer\n' >&2
    exit 1
fi
if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
    printf 'refusing to profile a tracked dirty worktree\n' >&2
    exit 1
fi

cargo build --locked --manifest-path "$repo_root/Cargo.toml" \
    --package capnp-native-benchmark --bin native_rpc --release >/dev/null

benchmark="$repo_root/target/release/native_rpc"
profile_output=$(mktemp)
checksum_output=$(mktemp)
trap 'rm -f -- "$profile_output" "$checksum_output"' EXIT
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
    printf 'transport=direct in-memory owned-message envelopes\n'
    printf 'method=Instant around nine native sequential RPC phases; wall timer around child process\n'
    printf 'iterations=%s\n' "$iterations"
} > "$output/metadata.txt"

"$benchmark" 10000 >/dev/null
started=$(date +%s%N)
CAPNP_BENCH_PROFILE=1 "$benchmark" "$iterations" \
    > "$checksum_output" 2> "$profile_output"
finished=$(date +%s%N)

test "$(wc -l < "$profile_output")" -eq 1
grep -Eq "^profile iterations=${iterations}( [a-z_]+_ns=[0-9]+){9}$" "$profile_output"
case $((iterations % 4)) in
    0) expected_checksum=$iterations ;;
    1) expected_checksum=1 ;;
    2) expected_checksum=$((iterations + 1)) ;;
    3) expected_checksum=0 ;;
esac
test "$(cat "$checksum_output")" = "$expected_checksum"

printf 'phase\ttotal_ns\tns_per_call\n' > "$output/phases.tsv"
phase_ns=0
for token in $(cut -d ' ' -f3- "$profile_output"); do
    name=${token%%=*}
    value=${token#*=}
    phase=${name%_ns}
    phase_ns=$((phase_ns + value))
    awk -v phase="$phase" -v total="$value" -v count="$iterations" \
        'BEGIN { printf "%s\t%s\t%.2f\n", phase, total, total / count }' \
        >> "$output/phases.tsv"
done

wall_ns=$((finished - started))
printf 'iterations\twall_ns\tphase_ns\tunattributed_ns\twall_ns_per_call\n' \
    > "$output/totals.tsv"
awk -v count="$iterations" -v wall="$wall_ns" -v phases="$phase_ns" \
    'BEGIN { printf "%s\t%s\t%s\t%s\t%.2f\n", count, wall, phases, wall - phases, wall / count }' \
    >> "$output/totals.tsv"

printf 'metadata=%s\nphases=%s\ntotals=%s\n' \
    "$output/metadata.txt" "$output/phases.tsv" "$output/totals.tsv"
