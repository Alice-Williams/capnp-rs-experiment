#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_root=${1:-"$repo_root/benchmarks/results/2026-08-31-m48-g-drive-docker"}

bash "$repo_root/benchmarks/verify-m29-parallel-results.sh" "$result_root/m29"
bash "$repo_root/benchmarks/verify-m30-parallel-results.sh" "$result_root/m30"
bash "$repo_root/benchmarks/verify-m31-batch-results.sh" "$result_root/m31"
grep -Fx 'samples_per_mode=31' "$result_root/m31/metadata.txt"
bash "$repo_root/benchmarks/verify-m39-server-scheduling-results.sh" "$result_root/m39"

assert_current_sources() {
  local metadata=$1
  shift
  local base_commit
  base_commit=$(sed -n 's/^base_git_commit=//p' "$metadata")
  if ! git -C "$repo_root" diff --quiet "$base_commit" -- "$@"; then
    printf 'M48 performance source differs from recorded commit: %s\n' "$metadata" >&2
    exit 1
  fi
}

assert_current_sources "$result_root/m29/metadata.txt" \
  crates/capnp-message/src/parallel.rs crates/capnp-message/examples/parallel_read.rs
assert_current_sources "$result_root/m30/metadata.txt" \
  crates/capnp-message/src/parallel_builder.rs crates/capnp-message/examples/parallel_build.rs
assert_current_sources "$result_root/m31/metadata.txt" \
  crates/capnp-async/src/batch.rs crates/capnp-async/examples/batch_pipeline.rs
assert_current_sources "$result_root/m39/metadata.txt" \
  crates/capnp-rpc/src/scheduler.rs crates/capnp-rpc/examples/server_scheduling.rs

printf 'M48 release-commit parallel and scheduling performance results OK\n'
