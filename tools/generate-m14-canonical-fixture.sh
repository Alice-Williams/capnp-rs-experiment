#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle=/opt/capnp-oracles/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp
output="$repo_root/conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/builder-canonical.bin"
framed=$(mktemp)
generated=$(mktemp)
trap 'rm -f -- "$framed" "$generated"' EXIT

cd -- "$repo_root"
cargo run -q -p capnp-io --example m12_builder_fixture >"$framed"
"$oracle" convert binary:canonical <"$framed" >"$generated"
install -m 0644 -- "$generated" "$output"
sha256sum -- "$output"
