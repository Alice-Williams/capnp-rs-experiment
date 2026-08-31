#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir="$repo_root/release/results/2026-08-31-m40-g-drive-docker"
result="$result_dir/soak.txt"
metadata="$result_dir/metadata.txt"

grep -Fx 'status=PASS' "$result"
summary=$(grep -E '^m40-level1-soak-ok sessions=[0-9]+ warmup_sessions=[0-9]+ seed=[0-9]+ elapsed_seconds=[0-9]+ baseline_rss_kib=[0-9]+ maximum_rss_kib=[0-9]+ final_rss_kib=[0-9]+$' "$result")
sessions=$(sed -E 's/.* sessions=([0-9]+) .*/\1/' <<<"$summary")
elapsed=$(sed -E 's/.* elapsed_seconds=([0-9]+) .*/\1/' <<<"$summary")
if (( sessions < 100000 )); then
  printf 'Recorded M40 soak has too few sessions: %s\n' "$sessions" >&2
  exit 1
fi
if (( elapsed < 86400 )); then
  printf 'Recorded M40 soak is shorter than 24 hours: %s seconds\n' "$elapsed" >&2
  exit 1
fi

expected_example=$(sed -n 's/^level1_soak_sha256=//p' "$metadata")
expected_runner=$(sed -n 's/^soak_runner_sha256=//p' "$metadata")
printf '%s  %s\n' "$expected_example" \
  "$repo_root/crates/capnp-rpc/examples/level1_soak.rs" | sha256sum --check
printf '%s  %s\n' "$expected_runner" \
  "$repo_root/tools/run-m40-level1-soak.sh" | sha256sum --check

printf 'M40 recorded 24-hour Level-1 soak result OK\n'
