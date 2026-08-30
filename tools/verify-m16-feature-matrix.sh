#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"

cargo check -p capnp-wire
cargo check -p capnp-message --no-default-features --features alloc
cargo check -p capnp-io --no-default-features
cargo check -p capnp-io --no-default-features --features alloc
cargo check -p capnp-io --features std
cargo check -p capnp-async
printf 'm16-feature-matrix-ok\n'
