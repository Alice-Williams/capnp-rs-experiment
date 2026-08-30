#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
request="$repo_root/conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/compiler-request-import-fixture.bin"
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

cd -- "$repo_root"
cargo run -q -p capnp-codegen --example m20_generate -- "$request" >"$scratch/generated.rs"
mkdir -p -- "$scratch/src"
cat >"$scratch/Cargo.toml" <<EOF
[package]
name = "m20-compile-growth"
version = "0.0.0"
edition = "2024"

[dependencies]
capnp-message = { path = "$repo_root/crates/capnp-message" }
capnp-schema = { path = "$repo_root/crates/capnp-schema" }
EOF

target="$scratch/target"
: >"$scratch/src/lib.rs"
cargo check -q --manifest-path "$scratch/Cargo.toml" --target-dir "$target"
printf 'modules\tgenerated_lines\tgenerated_bytes\tcheck_milliseconds\n'
for modules in 1 2 4 8; do
    : >"$scratch/src/lib.rs"
    for ((index = 0; index < modules; index++)); do
        printf 'pub mod copy_%s { include!("%s/generated.rs"); }\n' "$index" "$scratch" \
            >>"$scratch/src/lib.rs"
    done
    started=$(date +%s%N)
    cargo check -q --manifest-path "$scratch/Cargo.toml" --target-dir "$target"
    finished=$(date +%s%N)
    milliseconds=$(((finished - started) / 1000000))
    lines=$(wc -l <"$scratch/generated.rs")
    bytes=$(wc -c <"$scratch/generated.rs")
    printf '%s\t%s\t%s\t%s\n' \
        "$modules" "$((lines * modules))" "$((bytes * modules))" "$milliseconds"
done
