#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
oracle="$oracle_root/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp"
framed=$(mktemp)
decoded=$(mktemp)
expected=$(mktemp)
trap 'rm -f -- "$framed" "$decoded" "$expected"' EXIT

cd -- "$repo_root"
cargo run -q -p capnp-io --example m12_builder_fixture >"$framed"
"$oracle" decode conformance/schemas/builder-fixture.capnp BuilderFixture \
    <"$framed" >"$decoded"

cat >"$expected" <<'EOF'
( id = 81985529216486895,
  name = "native builder",
  payload = "\000\001\002\377",
  numbers = [10, 20, 30],
  labels = ["left", "right"],
  child = (value = 7, note = "only"),
  children = [
    (value = 11, note = "first"),
    (value = 22, note = "second") ],
  nested = [[1, 2], [3]] )
EOF

diff -u -- "$expected" "$decoded"
printf 'm12-cpp-decode-ok  %s bytes\n' "$(wc -c <"$framed")"
