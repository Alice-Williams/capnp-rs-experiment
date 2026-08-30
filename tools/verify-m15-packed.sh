#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle=/opt/capnp-oracles/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp
unpacked="$repo_root/conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
cpp_packed="$repo_root/conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-packed.bin"
native_packed=$(mktemp)
decoded=$(mktemp)
trap 'rm -f -- "$native_packed" "$decoded"' EXIT

cd -- "$repo_root"
cargo run -q -p capnp-io --example m15_pack <"$unpacked" >"$native_packed"
cmp -- "$cpp_packed" "$native_packed"
"$oracle" convert packed:binary <"$native_packed" >"$decoded"
cmp -- "$unpacked" "$decoded"
cargo run -q -p capnp-io --example m15_unpack <"$cpp_packed" >"$decoded"
cmp -- "$unpacked" "$decoded"
printf 'm15-packed-ok  %s -> %s bytes\n' \
    "$(wc -c <"$unpacked")" "$(wc -c <"$native_packed")"
