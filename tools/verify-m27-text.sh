#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
oracle="$oracle_root/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install/bin/capnp"
schema="$repo_root/conformance/schemas/wire-fixture.capnp"
language="$repo_root/conformance/schemas/language-fixture.capnp"
imports="$repo_root/conformance/schemas/import-fixture.capnp"
cpp_frame="$repo_root/conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
reference="$repo_root/conformance/fixtures/text/wire-short.txt"
source_text="$repo_root/conformance/fixtures/source/wire-fixture.txt"
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

cd -- "$repo_root"

cargo run -q -p capnp-cli -- decode --short "$schema" WireFixture \
    <"$cpp_frame" >"$scratch/native-from-cpp.txt"
cmp -- "$reference" "$scratch/native-from-cpp.txt"

cargo run -q -p capnp-cli -- encode "$schema" WireFixture \
    <"$source_text" >"$scratch/native-from-source.bin"
cmp -- "$cpp_frame" "$scratch/native-from-source.bin"

cargo run -q -p capnp-cli -- decode "$schema" WireFixture \
    <"$cpp_frame" >"$scratch/native-pretty.txt"
"$oracle" encode "$schema" WireFixture \
    <"$scratch/native-pretty.txt" >"$scratch/cpp-from-native-pretty.bin"
"$oracle" decode --short "$schema" WireFixture \
    <"$scratch/cpp-from-native-pretty.bin" >"$scratch/cpp-from-native-pretty.txt"
"$oracle" encode "$schema" WireFixture \
    <"$reference" >"$scratch/cpp-reference.bin"
"$oracle" decode --short "$schema" WireFixture \
    <"$scratch/cpp-reference.bin" >"$scratch/cpp-reference-normalized.txt"
cmp -- "$scratch/cpp-reference-normalized.txt" "$scratch/cpp-from-native-pretty.txt"

cargo run -q -p capnp-cli -- encode "$schema" WireFixture \
    <"$reference" >"$scratch/native.bin"
"$oracle" decode --short "$schema" WireFixture \
    <"$scratch/native.bin" >"$scratch/cpp-from-native.txt"
cmp -- "$reference" "$scratch/cpp-from-native.txt"

"$oracle" encode "$schema" WireFixture \
    <"$reference" >"$scratch/cpp.bin"
"$oracle" decode --short "$schema" WireFixture \
    <"$scratch/cpp.bin" >"$scratch/cpp-text-round-trip.txt"
cargo run -q -p capnp-cli -- decode --short "$schema" WireFixture \
    <"$scratch/cpp.bin" >"$scratch/native-from-cpp-text.txt"
cmp -- "$scratch/cpp-text-round-trip.txt" "$scratch/native-from-cpp-text.txt"

cargo run -q -p capnp-cli -- encode --packed "$schema" WireFixture \
    <"$reference" >"$scratch/native.packed"
"$oracle" decode --packed --short "$schema" WireFixture \
    <"$scratch/native.packed" >"$scratch/cpp-from-native-packed.txt"
cmp -- "$reference" "$scratch/cpp-from-native-packed.txt"

"$oracle" encode --packed "$schema" WireFixture \
    <"$reference" >"$scratch/cpp.packed"
"$oracle" decode --packed --short "$schema" WireFixture \
    <"$scratch/cpp.packed" >"$scratch/cpp-packed-round-trip.txt"
cargo run -q -p capnp-cli -- decode --packed --short "$schema" WireFixture \
    <"$scratch/cpp.packed" >"$scratch/native-from-cpp-packed.txt"
cmp -- "$scratch/cpp-packed-round-trip.txt" "$scratch/native-from-cpp-packed.txt"

for expression in \
    LanguageFixture.answer \
    'LanguageFixture.primes[3]' \
    LanguageFixture.sampleBox.value
do
    "$oracle" eval --short "$language" "$expression" >"$scratch/cpp-eval.txt"
    cargo run -q -p capnp-cli -- eval --short "$language" "$expression" \
        >"$scratch/native-eval.txt"
    cmp -- "$scratch/cpp-eval.txt" "$scratch/native-eval.txt"
done

"$oracle" eval --short "$imports" Language.LanguageFixture.answer \
    >"$scratch/cpp-import-eval.txt"
cargo run -q -p capnp-cli -- eval --short "$imports" \
    Language.LanguageFixture.answer >"$scratch/native-import-eval.txt"
cmp -- "$scratch/cpp-import-eval.txt" "$scratch/native-import-eval.txt"

cargo run -q -p capnp-cli -- eval -b "$language" LanguageFixture.sampleBox \
    >"$scratch/native-eval.bin"
"$oracle" decode --short "$language" 'Box(Text)' \
    <"$scratch/native-eval.bin" >"$scratch/native-eval-binary.txt"
"$oracle" eval -b "$language" LanguageFixture.sampleBox \
    >"$scratch/cpp-eval.bin"
"$oracle" decode --short "$language" 'Box(Text)' \
    <"$scratch/cpp-eval.bin" >"$scratch/cpp-eval-binary.txt"
cmp -- "$scratch/cpp-eval-binary.txt" "$scratch/native-eval-binary.txt"

cargo run -q -p capnp-cli -- eval -p "$language" LanguageFixture.sampleBox \
    >"$scratch/native-eval.packed"
"$oracle" decode --packed --short "$language" 'Box(Text)' \
    <"$scratch/native-eval.packed" >"$scratch/native-eval-packed.txt"
cmp -- "$scratch/cpp-eval-binary.txt" "$scratch/native-eval-packed.txt"

printf 'm27-text-ok  standard packed flat and text/binary eval agree\n'
