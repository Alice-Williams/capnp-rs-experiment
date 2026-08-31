#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
minimum_sessions=${M48_SOAK_MIN_SESSIONS:-100000}
duration_seconds=${M48_SOAK_SECONDS:-172800}
warmup_sessions=${M48_SOAK_WARMUP_SESSIONS:-100}
max_rss_growth_kib=${M48_SOAK_MAX_RSS_GROWTH_KIB:-65536}
seed=${M48_SOAK_SEED:-7868976431532826987}
result_file=${M48_SOAK_RESULT_FILE:-}

case "$minimum_sessions:$duration_seconds:$warmup_sessions:$max_rss_growth_kib:$seed" in
  *[!0-9:]* | :* | *::* | *:) printf 'M48 soak settings must be unsigned integers\n' >&2; exit 2 ;;
esac

cd -- "$repo_root"
result_arguments=()
if [[ -n "$result_file" ]]; then
  result_arguments=(--result-file "$result_file")
fi
output=$(timeout --signal=TERM --kill-after=30 "$((duration_seconds + 600))" \
  cargo run --release --quiet -p capnp-examples --bin m48_soak -- \
  --minimum-sessions "$minimum_sessions" \
  --duration-seconds "$duration_seconds" \
  --warmup-sessions "$warmup_sessions" \
  --max-rss-growth-kib "$max_rss_growth_kib" \
  --seed "$seed" \
  "${result_arguments[@]}")
printf '%s\n' "$output"

grep -Eq '^m48-full-platform-soak-ok sessions=[0-9]+ warmup_sessions=[0-9]+ seed=[0-9]+ elapsed_seconds=[0-9]+ baseline_rss_kib=[0-9]+ maximum_rss_kib=[0-9]+ final_rss_kib=[0-9]+$' <<<"$output"
sessions=$(sed -E 's/.* sessions=([0-9]+) .*/\1/' <<<"$output")
elapsed=$(sed -E 's/.* elapsed_seconds=([0-9]+) .*/\1/' <<<"$output")
if (( sessions < minimum_sessions )); then
  printf 'M48 soak session gate failed: %s < %s sessions\n' "$sessions" "$minimum_sessions" >&2
  exit 1
fi
if (( elapsed < duration_seconds )); then
  printf 'M48 soak duration gate failed: %s < %s seconds\n' "$elapsed" "$duration_seconds" >&2
  exit 1
fi
