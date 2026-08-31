#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
oracle="$oracle_root/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp"
frame=$(mktemp)
decoded=$(mktemp)
trap 'rm -f -- "$frame" "$decoded"' EXIT

cd -- "$repo_root"
cargo run -q -p capnp-generated-fixture --example m19_generated_fixture >"$frame"
"$oracle" decode conformance/schemas/wire-fixture.capnp WireFixture \
    <"$frame" >"$decoded"

grep -F 'uint32Value = 77' "$decoded" >/dev/null
grep -F 'color = (99)' "$decoded" >/dev/null
grep -F 'text = "native generated"' "$decoded" >/dev/null
grep -F 'uint16s = [2, 3, 5]' "$decoded" >/dev/null
grep -F 'number = 444' "$decoded" >/dev/null
grep -F 'node = (value = 88)' "$decoded" >/dev/null
printf 'm19-generated-cpp-decode-ok  %s bytes\n' "$(wc -c <"$frame")"
