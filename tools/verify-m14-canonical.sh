#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
oracle="$oracle_root/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp"
fixture="$repo_root/conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/builder-canonical.bin"
framed=$(mktemp)
cpp=$(mktemp)
native=$(mktemp)
second=$(mktemp)
trap 'rm -f -- "$framed" "$cpp" "$native" "$second"' EXIT

cd -- "$repo_root"
cargo run -q -p capnp-io --example m12_builder_fixture >"$framed"
"$oracle" convert binary:canonical <"$framed" >"$cpp"
cargo run -q -p capnp-io --example m14_canonicalize <"$framed" >"$native"
cmp -- "$fixture" "$cpp"
cmp -- "$cpp" "$native"
"$oracle" convert canonical:canonical <"$native" >"$second"
cmp -- "$native" "$second"
printf 'm14-canonical-ok  %s bytes\n' "$(wc -c <"$native")"
