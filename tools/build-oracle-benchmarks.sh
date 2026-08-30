#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
rust_commit=2228b71e55cee819c30450bb9bfd9c1f6a722429
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

cpp_root="$oracle_root/capnproto-$cpp_commit"
cpp_source="$cpp_root/source/c++/src/benchmark"
cpp_prefix="$cpp_root/install"
cpp_build="$cpp_root/benchmark"
rust_root="$oracle_root/capnproto-rust-$rust_commit"
rust_source="$rust_root/build-source"
rust_target="$rust_root/cargo-target"
compatibility_patch="$repo_root/benchmarks/patches/cpp-benchmark-kj-stream-api.patch"

for command_name in cargo clang++ cp git patch sha256sum; do
    if ! command -v "$command_name" >/dev/null; then
        printf 'required command is unavailable: %s\n' "$command_name" >&2
        exit 1
    fi
done

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
bash "$repo_root/tools/build-rust-oracle.sh" >/dev/null

test "$(git -C "$cpp_root/source" rev-parse HEAD)" = "$cpp_commit"
test "$(git -C "$rust_root/source" rev-parse HEAD)" = "$rust_commit"
test -z "$(git -C "$cpp_root/source" status --porcelain)"
test -z "$(git -C "$rust_root/source" status --porcelain)"

mkdir -p -- "$cpp_build"
cp -- "$cpp_source"/capnproto-*.c++ "$cpp_build"/
cp -- "$cpp_source"/capnproto-common.h "$cpp_source"/common.h "$cpp_build"/
patch --directory="$cpp_build" --strip=0 --forward --input="$compatibility_patch"

for case_name in carsales catrank eval; do
    "$cpp_prefix/bin/capnp" compile \
        --src-prefix="$cpp_source" \
        -o"$cpp_prefix/bin/capnpc-c++":"$cpp_build" \
        "$cpp_source/$case_name.capnp"
    clang++ \
        -std=c++23 \
        -O3 \
        -DNDEBUG \
        -pthread \
        -I"$cpp_build" \
        -I"$cpp_prefix/include" \
        "$cpp_build/capnproto-$case_name.c++" \
        "$cpp_build/$case_name.capnp.c++" \
        -L"$cpp_prefix/lib" \
        -lcapnp \
        -lkj \
        -o "$cpp_build/capnproto-$case_name"
done

PATH="$cpp_prefix/bin:$PATH" cargo build \
    --locked \
    --manifest-path "$rust_source/Cargo.toml" \
    --package benchmark \
    --bin benchmark \
    --release \
    --target-dir "$rust_target"

printf 'cpp_commit=%s\nrust_commit=%s\n' "$cpp_commit" "$rust_commit"
for executable in \
    "$cpp_build/capnproto-carsales" \
    "$cpp_build/capnproto-catrank" \
    "$cpp_build/capnproto-eval" \
    "$rust_target/release/benchmark"
do
    sha256sum "$executable"
done
