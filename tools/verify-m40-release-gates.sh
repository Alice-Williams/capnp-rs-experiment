#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"

bash tools/verify-m40-level1-interop.sh
bash tools/run-m40-rpc-fuzz.sh
if [[ ${M40_USE_RECORDED_SOAK:-0} == 1 ]]; then
  bash tools/verify-m40-soak-result.sh
else
  bash tools/run-m40-level1-soak.sh
fi

# Re-check every recorded parallel release ratio and latency artifact. Each
# verifier binds results to the checked-in source and rejects missing context.
bash benchmarks/verify-m29-parallel-results.sh \
  benchmarks/results/2026-08-31-m29-g-drive-docker
bash benchmarks/verify-m30-parallel-results.sh \
  benchmarks/results/2026-08-31-m30-g-drive-docker
bash benchmarks/verify-m31-batch-results.sh \
  benchmarks/results/2026-08-31-m31-g-drive-docker
bash benchmarks/verify-m39-server-scheduling-results.sh \
  benchmarks/results/2026-08-31-m39-g-drive-docker

# This workspace forbids unsafe Rust. The hostile decoders, exact shared
# budgets, bounded actor tables, disconnect cleanup, and shell/compile gates
# therefore make up the M40 security boundary.
if git grep -n -E 'unsafe[[:space:]]*\{' -- '*.rs'; then
  printf 'M40 security gate failed: unsafe block found\n' >&2
  exit 1
fi
cargo test --quiet -p capnp-message --features loom-tests \
  loom_proves_competing_charges_preserve_the_hard_limit
cargo test --quiet -p capnp-rpc-core actor::tests
cargo +nightly-2026-08-31 miri test --quiet -p capnp-wire
cargo +nightly-2026-08-31 miri test --quiet -p capnp-message \
  miri_disjoint_primitive_partitions_do_not_alias
bash tools/check-shell-syntax.sh

printf 'M40 release gates OK\n'
