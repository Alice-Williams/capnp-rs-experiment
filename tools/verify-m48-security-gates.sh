#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
performance_results=${M48_PERFORMANCE_RESULT_DIR:-"$repo_root/benchmarks/results/2026-08-31-m48-g-drive-docker"}
cd -- "$repo_root"

bash tools/verify-m48-inventory.sh
bash tools/verify-m48-oracles.sh

if git grep -n -E 'unsafe[[:space:]]*\{' -- '*.rs'; then
  printf 'M48 security gate failed: unsafe block found\n' >&2
  exit 1
fi
bash tools/check-shell-syntax.sh
cargo fmt --all -- --check
rustfmt --edition 2021 --check --config skip_children=true benchmarks/rpc/rust/main.rs
cargo test --workspace --all-targets
cargo test --workspace --doc
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
cargo +1.85.0 test --workspace --all-targets
bazel test //...

M40_FUZZ_MIN_CASES=100000 M40_FUZZ_SECONDS=60 bash tools/run-m40-rpc-fuzz.sh
cargo test -p capnp-message --features loom-tests \
  loom_proves_competing_charges_preserve_the_hard_limit
cargo test -p capnp-message --features loom-tests \
  loom_parallel_plan_preserves_precharge_and_nested_budget
cargo test -p capnp-message --features loom-tests \
  loom_lane_arrival_order_finalizes_deterministically
cargo +nightly-2026-08-31 miri test -p capnp-wire
cargo +nightly-2026-08-31 miri test -p capnp-message \
  miri_disjoint_primitive_partitions_do_not_alias

bash tools/verify-m48-performance-results.sh "$performance_results"
printf 'M48 security, conformance, build, and performance gates OK\n'
