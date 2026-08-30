#!/usr/bin/env bash
set -euo pipefail

commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
oracle="$oracle_root/capnproto-$commit/install/bin/capnp"
oracle_include="$oracle_root/capnproto-$commit/install/include"
upstream="conformance/upstream/capnproto/$commit"

if [[ ! -x "$oracle" ]]; then
    printf 'missing pinned compiler oracle: %s\n' "$oracle" >&2
    exit 1
fi

valid=(
    conformance/schemas/builder-fixture.capnp
    conformance/schemas/evolution-v1.capnp
    conformance/schemas/evolution-v2.capnp
    conformance/schemas/evolution-v3.capnp
    conformance/schemas/import-fixture.capnp
    conformance/schemas/language-fixture.capnp
    conformance/schemas/orphan-fixture.capnp
    conformance/schemas/streaming-fixture.capnp
    conformance/schemas/wire-fixture.capnp
    "$upstream/persistent.capnp"
    "$upstream/rpc-twoparty.capnp"
    "$upstream/rpc.capnp"
    "$upstream/schema.capnp"
    "$upstream/stream.capnp"
)
invalid=(conformance/syntax/invalid-*.capnp)

for schema in "${valid[@]}"; do
    "$oracle" compile \
        -I conformance/schemas -I "$upstream" -I "$oracle_include" \
        -o- "$schema" >/dev/null 2>&1
    cargo run --quiet -p capnp-compiler --example m22_parse -- "$schema" >/dev/null
done

for schema in "${invalid[@]}"; do
    if "$oracle" compile \
        -I conformance/schemas -I "$upstream" -I "$oracle_include" \
        -o- "$schema" >/dev/null 2>&1; then
        printf 'pinned compiler unexpectedly accepted %s\n' "$schema" >&2
        exit 1
    fi
    if cargo run --quiet -p capnp-compiler --example m22_parse -- "$schema" >/dev/null 2>&1; then
        printf 'native parser unexpectedly accepted %s\n' "$schema" >&2
        exit 1
    fi
done

printf 'M22 parser accept/reject parity: %d accepted, %d rejected\n' \
    "${#valid[@]}" "${#invalid[@]}"
