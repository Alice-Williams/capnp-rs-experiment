#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
schema_root="$repo_root/conformance/schemas"

for schema in \
    wire-fixture.capnp \
    language-fixture.capnp \
    import-fixture.capnp \
    evolution-v1.capnp \
    evolution-v2.capnp
do
    capnp compile -I"$schema_root" -o- "$schema_root/$schema" >/dev/null
    printf 'syntax-ok  %s\n' "$schema"
done
