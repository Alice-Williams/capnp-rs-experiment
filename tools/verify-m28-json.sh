#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
oracle="$oracle_root/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp"
oracle_include="$oracle_root/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/include"
wire_schema="$repo_root/conformance/schemas/wire-fixture.capnp"
json_schema="$repo_root/conformance/schemas/json-fixture.capnp"
cpp_frame="$repo_root/conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
wire_json="$repo_root/conformance/fixtures/json/wire-short.json"
annotation_json="$repo_root/conformance/fixtures/json/annotation-short.json"
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

cd -- "$repo_root"

cargo run -q -p capnp-cli -- convert --short binary:json \
    "$wire_schema" WireFixture <"$cpp_frame" >"$scratch/native-wire.json"
cmp -- "$wire_json" "$scratch/native-wire.json"

cargo run -q -p capnp-cli -- convert --short json:binary \
    "$wire_schema" WireFixture <"$wire_json" >"$scratch/native-wire.bin"
"$oracle" convert --short binary:json "$wire_schema" WireFixture \
    <"$scratch/native-wire.bin" >"$scratch/cpp-from-native.json"
cmp -- "$wire_json" "$scratch/cpp-from-native.json"

"$oracle" convert --short json:binary "$wire_schema" WireFixture \
    <"$wire_json" >"$scratch/cpp-from-json.bin"
cargo run -q -p capnp-cli -- convert --short binary:json \
    "$wire_schema" WireFixture <"$scratch/cpp-from-json.bin" \
    >"$scratch/native-from-cpp.json"
cmp -- "$wire_json" "$scratch/native-from-cpp.json"

cargo run -q -p capnp-cli -- convert --short json:binary \
    "$json_schema" JsonFixture <"$annotation_json" >"$scratch/native-annotations.bin"
"$oracle" convert --short binary:json -I"$oracle_include" \
    "$json_schema" JsonFixture <"$scratch/native-annotations.bin" \
    >"$scratch/cpp-annotations.json"
cmp -- "$annotation_json" "$scratch/cpp-annotations.json"

"$oracle" convert --short json:binary -I"$oracle_include" \
    "$json_schema" JsonFixture <"$annotation_json" >"$scratch/cpp-annotations.bin"
cargo run -q -p capnp-cli -- convert --short binary:json \
    "$json_schema" JsonFixture <"$scratch/cpp-annotations.bin" \
    >"$scratch/native-annotations.json"
cmp -- "$annotation_json" "$scratch/native-annotations.json"

printf 'm28-json-ok  default and annotated corpora agree in both directions\n'
