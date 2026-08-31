#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
minimum_sessions=${M40_SOAK_MIN_SESSIONS:-100000}
duration_seconds=${M40_SOAK_SECONDS:-86400}
seed=${M40_SOAK_SEED:-7868967635439804779}
result_file=${M40_SOAK_RESULT_FILE:-}

case "$minimum_sessions:$duration_seconds:$seed" in
  *[!0-9:]* | :* | *::* | *:) printf 'M40 soak settings must be unsigned integers\n' >&2; exit 2 ;;
esac

cd -- "$repo_root"
result_arguments=()
if [[ -n "$result_file" ]]; then
  result_arguments=(--result-file "$result_file")
fi
output=$(timeout --signal=TERM --kill-after=30 "$((duration_seconds + 300))" \
  cargo run --release --quiet -p capnp-rpc --example level1_soak -- \
  --minimum-sessions "$minimum_sessions" \
  --duration-seconds "$duration_seconds" \
  --seed "$seed" \
  "${result_arguments[@]}")
printf '%s\n' "$output"

grep -Eq '^m40-level1-soak-ok sessions=[0-9]+ warmup_sessions=[0-9]+ seed=[0-9]+ elapsed_seconds=[0-9]+ baseline_rss_kib=[0-9]+ maximum_rss_kib=[0-9]+ final_rss_kib=[0-9]+$' <<<"$output"
sessions=$(sed -E 's/.* sessions=([0-9]+) .*/\1/' <<<"$output")
elapsed=$(sed -E 's/.* elapsed_seconds=([0-9]+) .*/\1/' <<<"$output")
if (( sessions < minimum_sessions )); then
  printf 'M40 soak session gate failed: %s < %s sessions\n' "$sessions" "$minimum_sessions" >&2
  exit 1
fi
if (( elapsed < duration_seconds )); then
  printf 'M40 soak duration gate failed: %s < %s seconds\n' "$elapsed" "$duration_seconds" >&2
  exit 1
fi
