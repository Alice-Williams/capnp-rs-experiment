#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:-${M40_SOAK_RESULT_DIR:-"$repo_root/release/results/2026-08-31-m40-g-drive-docker"}}
result="$result_dir/soak.txt"
metadata="$result_dir/metadata.txt"

for required in "$result" "$metadata" "$result_dir/console.txt"; do
  if [[ ! -f "$required" ]]; then
    printf 'M40 recorded soak file is missing: %s\n' "$required" >&2
    exit 1
  fi
done

metadata_value() {
  local key=$1
  local count
  count=$(grep -c "^${key}=" "$metadata" || true)
  if [[ $count != 1 ]]; then
    printf 'M40 metadata must contain exactly one %s entry\n' "$key" >&2
    exit 1
  fi
  sed -n "s/^${key}=//p" "$metadata"
}

if [[ $(grep -c '^status=PASS$' "$result") != 1 ]]; then
  printf 'M40 recorded soak must contain exactly one PASS status\n' >&2
  exit 1
fi
summary=$(grep -E '^m40-level1-soak-ok sessions=[0-9]+ warmup_sessions=[0-9]+ seed=[0-9]+ elapsed_seconds=[0-9]+ baseline_rss_kib=[0-9]+ maximum_rss_kib=[0-9]+ final_rss_kib=[0-9]+$' "$result")
if [[ $(grep -c '^m40-level1-soak-ok ' "$result") != 1 ]]; then
  printf 'M40 recorded soak must contain exactly one valid summary\n' >&2
  exit 1
fi
gate=$(grep -E '^gate=PASS: at least [0-9]+ sessions and [0-9]+ wall-clock seconds; every disconnected session released all connection-owned state$' "$result")
if [[ $(grep -c '^gate=PASS:' "$result") != 1 ]]; then
  printf 'M40 recorded soak must contain exactly one valid gate declaration\n' >&2
  exit 1
fi

sessions=$(sed -E 's/.* sessions=([0-9]+) .*/\1/' <<<"$summary")
seed=$(sed -E 's/.* seed=([0-9]+) .*/\1/' <<<"$summary")
elapsed=$(sed -E 's/.* elapsed_seconds=([0-9]+) .*/\1/' <<<"$summary")
baseline=$(sed -E 's/.* baseline_rss_kib=([0-9]+) .*/\1/' <<<"$summary")
maximum=$(sed -E 's/.* maximum_rss_kib=([0-9]+) .*/\1/' <<<"$summary")

minimum_sessions=$(metadata_value minimum_sessions)
duration_seconds=$(metadata_value duration_seconds)
max_rss_growth_kib=$(metadata_value max_rss_growth_kib)
metadata_seed=$(metadata_value seed)
case "$minimum_sessions:$duration_seconds:$max_rss_growth_kib:$metadata_seed" in
  *[!0-9:]* | :* | *::* | *:) printf 'M40 metadata settings must be unsigned integers\n' >&2; exit 1 ;;
esac
if (( minimum_sessions < 100000 || sessions < minimum_sessions )); then
  printf 'Recorded M40 soak has too few sessions: %s (configured %s)\n' "$sessions" "$minimum_sessions" >&2
  exit 1
fi
if (( duration_seconds < 86400 || elapsed < duration_seconds )); then
  printf 'Recorded M40 soak is too short: %s seconds (configured %s)\n' "$elapsed" "$duration_seconds" >&2
  exit 1
fi
if [[ $seed != "$metadata_seed" ]]; then
  printf 'M40 summary does not match its recorded seed\n' >&2
  exit 1
fi
gate_sessions=$(sed -E 's/^gate=PASS: at least ([0-9]+) sessions.*/\1/' <<<"$gate")
gate_seconds=$(sed -E 's/^gate=PASS: at least [0-9]+ sessions and ([0-9]+) wall-clock.*/\1/' <<<"$gate")
if [[ $gate_sessions != "$minimum_sessions" || $gate_seconds != "$duration_seconds" ]]; then
  printf 'M40 gate declaration does not match its recorded settings\n' >&2
  exit 1
fi
if (( baseline != 0 && maximum > baseline + max_rss_growth_kib )); then
  printf 'Recorded M40 soak exceeded its RSS growth bound\n' >&2
  exit 1
fi

source_commit=$(metadata_value source_commit)
expected_tree=$(metadata_value source_tree_sha256)
if [[ ! $source_commit =~ ^[0-9a-f]{40}$ || ! $expected_tree =~ ^[0-9a-f]{64}$ ]]; then
  printf 'M40 source provenance metadata is malformed\n' >&2
  exit 1
fi
cd -- "$repo_root"
git cat-file -e "${source_commit}^{commit}"
git merge-base --is-ancestor "$source_commit" HEAD
actual_tree=$(git ls-tree -r "$source_commit" -- \
  Cargo.toml Cargo.lock crates .github/workflows/ci.yml \
  tools/run-m40-level1-soak.sh tools/run-m40-release-soak.sh | sha256sum | cut -d' ' -f1)
if [[ $actual_tree != "$expected_tree" ]]; then
  printf 'M40 recorded source tree does not match its source commit\n' >&2
  exit 1
fi
if ! git diff --quiet "$source_commit" -- \
  Cargo.toml Cargo.lock crates .github/workflows/ci.yml \
  tools/run-m40-level1-soak.sh tools/run-m40-release-soak.sh; then
  printf 'M40 source inputs differ from the recorded soak inputs\n' >&2
  exit 1
fi
metadata_value started_at > /dev/null
metadata_value completed_at > /dev/null

printf 'M40 recorded 24-hour Level-1 soak result OK\n'
