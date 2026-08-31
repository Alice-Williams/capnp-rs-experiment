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
cargo run -q -p capnp-io --example m13_orphan_fixture >"$framed"
"$oracle" decode conformance/schemas/orphan-fixture.capnp OrphanFixture \
    <"$framed" >"$decoded"

cat >"$expected" <<'EOF'
( newChild = (
    value = 4242,
    note = "moved without copying" ),
  newValues = [13, 21, 34] )
EOF

diff -u -- "$expected" "$decoded"
printf 'm13-cpp-decode-ok  %s bytes\n' "$(wc -c <"$framed")"
