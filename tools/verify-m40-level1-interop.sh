#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"

# Serialization and generated-data directions use independently produced
# fixtures. Together these prove C++ -> Rust, Rust -> C++, and Rust -> Rust.
bash tools/verify-upstream-pins.sh
bash conformance/verify-upstream-schema-files.sh
bash tools/verify-m11-cpp-decode.sh
bash tools/verify-m14-canonical.sh
bash tools/verify-m15-packed.sh
bash tools/verify-m19-generated-cpp-decode.sh
cargo test --quiet -p capnp-message -p capnp-schema -p capnp-generated-fixture \
  -p capnp-generated-import-fixture

# Level-1 RPC wire transcripts are emitted and consumed independently in both
# languages. The pinned C++ behavioral slices cover flow/lifecycle semantics;
# native actor/driver/scheduler suites cover the corresponding Rust endpoint.
bash tools/verify-m34-capabilities.sh
bash tools/verify-m35-calculator-pipeline.sh
bash tools/verify-m36-promise-resolution.sh
bash tools/verify-m37-flow-control.sh
bash tools/verify-m38-lifecycle.sh
cargo test --quiet -p capnp-rpc-core -p capnp-rpc

printf 'M40 Level-1 serialization/generated-data/RPC interoperability matrix OK\n'
