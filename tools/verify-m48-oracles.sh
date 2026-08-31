#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"

bash tools/verify-upstream-pins.sh
bash conformance/verify-upstream-schema-files.sh
bash tools/build-cpp-oracle.sh >/dev/null

checks=(
  tools/verify-m11-cpp-decode.sh
  tools/verify-m12-cpp-decode.sh
  tools/verify-m13-cpp-decode.sh
  tools/verify-m14-canonical.sh
  tools/verify-m15-packed.sh
  tools/verify-m16-feature-matrix.sh
  tools/verify-m19-generated-cpp-decode.sh
  tools/verify-m22-parser.sh
  tools/verify-m27-text.sh
  tools/verify-m28-json.sh
  tools/verify-m30-cpp-decode.sh
  tools/verify-m34-capabilities.sh
  tools/verify-m35-calculator-pipeline.sh
  tools/verify-m36-promise-resolution.sh
  tools/verify-m37-flow-control.sh
  tools/verify-m38-lifecycle.sh
  tools/verify-m40-level1-interop.sh
  tools/verify-m41-local-capabilities.sh
  tools/verify-m42-membranes.sh
  tools/verify-m43-attached-resources.sh
  tools/verify-m44-level3-handoffs.sh
  tools/verify-m45-level4-join.sh
  tools/verify-m46-persistent-capabilities.sh
  tools/verify-m47-byte-stream.sh
  tools/verify-m47-json-rpc.sh
  tools/verify-m47-http-websocket.sh
  tools/verify-m47-examples.sh
)
for check in "${checks[@]}"; do
  bash "$check"
done

printf 'M48 complete pinned-oracle suite OK\n'
