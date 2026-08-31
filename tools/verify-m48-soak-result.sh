#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${1:?usage: verify-m48-soak-result.sh RESULT_DIRECTORY}
result="$result_dir/soak.txt"
metadata="$result_dir/metadata.txt"

for required in "$result" "$metadata" "$result_dir/console.txt"; do
  if [[ ! -f "$required" ]]; then
    printf 'M48 recorded soak file is missing: %s\n' "$required" >&2
    exit 1
  fi
done

metadata_value() {
  local key=$1
  local count
  count=$(grep -c "^${key}=" "$metadata" || true)
  if [[ $count != 1 ]]; then
    printf 'M48 metadata must contain exactly one %s entry\n' "$key" >&2
    exit 1
  fi
  sed -n "s/^${key}=//p" "$metadata"
}

grep -Fx 'status=PASS' "$result"
gate=$(grep -E '^gate=PASS: at least [0-9]+ sessions and [0-9]+ wall-clock seconds; each session exercised address-book persistence, calculator capabilities, streaming cancellation, authenticated handoff, distributed equality, and persistent restart$' "$result")
if [[ $(grep -c '^gate=PASS:' "$result") != 1 ]]; then
  printf 'M48 recorded soak must contain exactly one valid gate declaration\n' >&2
  exit 1
fi
summary=$(grep -E '^m48-full-platform-soak-ok sessions=[0-9]+ warmup_sessions=[0-9]+ seed=[0-9]+ elapsed_seconds=[0-9]+ baseline_rss_kib=[0-9]+ maximum_rss_kib=[0-9]+ final_rss_kib=[0-9]+$' "$result")
if [[ $(grep -c '^m48-full-platform-soak-ok ' "$result") != 1 ]]; then
  printf 'M48 recorded soak must contain exactly one valid summary\n' >&2
  exit 1
fi

sessions=$(sed -E 's/.* sessions=([0-9]+) .*/\1/' <<<"$summary")
warmup=$(sed -E 's/.* warmup_sessions=([0-9]+) .*/\1/' <<<"$summary")
seed=$(sed -E 's/.* seed=([0-9]+) .*/\1/' <<<"$summary")
elapsed=$(sed -E 's/.* elapsed_seconds=([0-9]+) .*/\1/' <<<"$summary")
baseline=$(sed -E 's/.* baseline_rss_kib=([0-9]+) .*/\1/' <<<"$summary")
maximum=$(sed -E 's/.* maximum_rss_kib=([0-9]+) .*/\1/' <<<"$summary")

minimum_sessions=$(metadata_value minimum_sessions)
duration_seconds=$(metadata_value duration_seconds)
metadata_warmup=$(metadata_value warmup_sessions)
max_rss_growth_kib=$(metadata_value max_rss_growth_kib)
metadata_seed=$(metadata_value seed)
gate_sessions=$(sed -E 's/^gate=PASS: at least ([0-9]+) sessions.*/\1/' <<<"$gate")
gate_seconds=$(sed -E 's/^gate=PASS: at least [0-9]+ sessions and ([0-9]+) wall-clock.*/\1/' <<<"$gate")
case "$minimum_sessions:$duration_seconds:$metadata_warmup:$max_rss_growth_kib:$metadata_seed" in
  *[!0-9:]* | :* | *::* | *:) printf 'M48 metadata settings must be unsigned integers\n' >&2; exit 1 ;;
esac
if (( minimum_sessions < 100000 || sessions < minimum_sessions )); then
  printf 'Recorded M48 soak has too few sessions: %s (configured %s)\n' "$sessions" "$minimum_sessions" >&2
  exit 1
fi
if (( duration_seconds < 172800 || elapsed < duration_seconds )); then
  printf 'Recorded M48 soak is too short: %s seconds (configured %s)\n' "$elapsed" "$duration_seconds" >&2
  exit 1
fi
if (( warmup != metadata_warmup )) || [[ $seed != "$metadata_seed" ]]; then
  printf 'M48 summary does not match its recorded warmup or seed\n' >&2
  exit 1
fi
if [[ $gate_sessions != "$minimum_sessions" || $gate_seconds != "$duration_seconds" ]]; then
  printf 'M48 gate declaration does not match its recorded settings\n' >&2
  exit 1
fi
if (( baseline != 0 && maximum > baseline + max_rss_growth_kib )); then
  printf 'Recorded M48 soak exceeded its RSS growth bound\n' >&2
  exit 1
fi

source_commit=$(metadata_value source_commit)
expected_tree=$(metadata_value source_tree_sha256)
if [[ ! $source_commit =~ ^[0-9a-f]{40}$ || ! $expected_tree =~ ^[0-9a-f]{64}$ ]]; then
  printf 'M48 source provenance metadata is malformed\n' >&2
  exit 1
fi
cd -- "$repo_root"
git cat-file -e "${source_commit}^{commit}"
git merge-base --is-ancestor "$source_commit" HEAD
actual_tree=$(git ls-tree -r "$source_commit" -- \
  Cargo.toml Cargo.lock crates .github/workflows/ci.yml \
  tools/run-m48-full-platform-soak.sh tools/run-m48-release-soak.sh | sha256sum | cut -d' ' -f1)
if [[ $actual_tree != "$expected_tree" ]]; then
  printf 'M48 recorded source tree does not match its source commit\n' >&2
  exit 1
fi
if ! git diff --quiet "$source_commit" -- \
  Cargo.toml Cargo.lock crates .github/workflows/ci.yml \
  tools/run-m48-full-platform-soak.sh tools/run-m48-release-soak.sh; then
  printf 'M48 source inputs differ from the recorded soak inputs\n' >&2
  exit 1
fi
metadata_value started_at > /dev/null
metadata_value completed_at > /dev/null

printf 'M48 recorded 48-hour full-platform soak result OK\n'
