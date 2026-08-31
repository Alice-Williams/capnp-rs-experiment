#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
schema_root="$repo_root/conformance/schemas"
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
default_oracle="$oracle_root/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp"

if [[ -n "${CAPNP:-}" ]]; then
    capnp_binary=$CAPNP
elif [[ -x "$default_oracle" ]]; then
    capnp_binary=$default_oracle
else
    capnp_binary=$(command -v capnp)
fi

for schema in \
    builder-fixture.capnp \
    orphan-fixture.capnp \
    wire-fixture.capnp \
    language-fixture.capnp \
    import-fixture.capnp \
    evolution-v1.capnp \
    evolution-v2.capnp \
    evolution-v3.capnp \
    streaming-fixture.capnp
do
    "$capnp_binary" compile -I"$schema_root" -o- "$schema_root/$schema" >/dev/null
    printf 'syntax-ok  %s\n' "$schema"
done
