#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
minimum_cases=${M40_FUZZ_MIN_CASES:-100000}
duration_seconds=${M40_FUZZ_SECONDS:-60}
seed=${M40_FUZZ_SEED:-7868967635423093585}

case "$minimum_cases:$duration_seconds:$seed" in
  *[!0-9:]* | :* | *::* | *:) printf 'M40 fuzz settings must be unsigned integers\n' >&2; exit 2 ;;
esac

cd -- "$repo_root"
output=$(cargo run --release --quiet -p capnp-rpc-core --example rpc_decoder_fuzz -- \
  --minimum-cases "$minimum_cases" \
  --duration-seconds "$duration_seconds" \
  --seed "$seed")
printf '%s\n' "$output"

grep -Eq '^m40-rpc-decoder-fuzz-ok cases=[0-9]+ accepted=[0-9]+ rejected=[0-9]+ seed=[0-9]+ elapsed_ms=[0-9]+$' <<<"$output"
cases=$(sed -E 's/.* cases=([0-9]+) .*/\1/' <<<"$output")
if (( cases < minimum_cases )); then
  printf 'M40 fuzz corpus gate failed: %s < %s cases\n' "$cases" "$minimum_cases" >&2
  exit 1
fi
