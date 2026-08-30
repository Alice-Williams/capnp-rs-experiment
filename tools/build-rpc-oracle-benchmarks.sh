#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
rust_commit=2228b71e55cee819c30450bb9bfd9c1f6a722429
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

cpp_root="$oracle_root/capnproto-$cpp_commit"
cpp_prefix="$cpp_root/install"
cpp_build="$cpp_root/rpc-benchmark"
rust_root="$oracle_root/capnproto-rust-$rust_commit"
rust_build="$rust_root/rpc-benchmark"
rust_oracle="$rust_root/install/bin/capnpc-rust"

schema="$repo_root/benchmarks/rpc/ping.capnp"
cpp_main="$repo_root/benchmarks/rpc/cpp/main.c++"
rust_manifest="$repo_root/benchmarks/rpc/rust/Cargo.toml.in"
rust_lock="$repo_root/benchmarks/rpc/rust/Cargo.lock"
rust_main="$repo_root/benchmarks/rpc/rust/main.rs"

verify_hash() {
    local expected=$1
    local path=$2
    local actual
    actual=$(sha256sum "$path" | cut -d ' ' -f1)
    if [[ "$actual" != "$expected" ]]; then
        printf 'benchmark input hash mismatch: %s\n' "$path" >&2
        exit 1
    fi
}

verify_hash bd64cf8c596d3b2644af04e3b9417349a339928268134c8ba1d4c62f0512e9ba "$schema"
verify_hash e01c931d092d5c5fad83aeee1e7c252db3cbb4df6d5a4e1e80f15b77159f3b58 "$cpp_main"
verify_hash 94a57d3a39e53c1744a849beb00e577de867bf013d56c9c9b7f9487663aa088f "$rust_main"
verify_hash 520816d193b93f3d308eeceaf8f626a24dd1e0eca3632a5614ed98c8f7515792 "$rust_lock"

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
bash "$repo_root/tools/build-rust-oracle.sh" >/dev/null

test -z "$(git -C "$cpp_root/source" status --porcelain)"
test -z "$(git -C "$rust_root/source" status --porcelain)"

mkdir -p -- "$cpp_build"
"$cpp_prefix/bin/capnp" compile \
    --src-prefix="$repo_root/benchmarks/rpc" \
    -o"$cpp_prefix/bin/capnpc-c++":"$cpp_build" \
    "$schema"
clang++ \
    -std=c++23 \
    -O3 \
    -DNDEBUG \
    -pthread \
    -I"$cpp_build" \
    -I"$cpp_prefix/include" \
    "$cpp_main" \
    "$cpp_build/ping.capnp.c++" \
    -L"$cpp_prefix/lib" \
    -lcapnp-rpc \
    -lcapnp \
    -lkj-async \
    -lkj \
    -o "$cpp_build/cpp-rpc-benchmark"

mkdir -p -- "$rust_build/src"
cp -- "$rust_main" "$rust_build/src/main.rs"
cp -- "$rust_lock" "$rust_build/Cargo.lock"
sed "s|@RUST_ORACLE@|$rust_root/build-source|g" \
    "$rust_manifest" > "$rust_build/Cargo.toml"
"$cpp_prefix/bin/capnp" compile \
    --src-prefix="$repo_root/benchmarks/rpc" \
    -o"$rust_oracle":"$rust_build/src" \
    "$schema"
cargo build \
    --locked \
    --manifest-path "$rust_build/Cargo.toml" \
    --release

cpp_result=$("$cpp_build/cpp-rpc-benchmark" 100)
rust_result=$("$rust_build/target/release/capnp-oracle-rpc-benchmark" 100)
test "$cpp_result" = "$rust_result"

printf 'cpp_commit=%s\nrust_commit=%s\nsmoke_checksum=%s\n' \
    "$cpp_commit" "$rust_commit" "$cpp_result"
sha256sum \
    "$cpp_build/cpp-rpc-benchmark" \
    "$rust_build/target/release/capnp-oracle-rpc-benchmark"
