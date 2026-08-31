#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_prefix="$oracle_root/capnproto-$cpp_commit/install"
build_root="$oracle_root/capnproto-$cpp_commit/m36-promise-resolution"
source_file="$repo_root/conformance/oracle/m36_promise_resolution.c++"
fixture="$repo_root/conformance/fixtures/cpp/$cpp_commit/rpc-promise-resolution.bin"

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
mkdir -p -- "$build_root"
clang++ -std=c++23 -O2 -pthread \
  -I"$cpp_prefix/include" "$source_file" \
  -L"$cpp_prefix/lib" -lcapnp-rpc -lcapnp -lkj \
  -o "$build_root/m36-promise-resolution"

generated="$build_root/rpc-promise-resolution.bin"
"$build_root/m36-promise-resolution" generate > "$generated"
cmp -- "$generated" "$fixture"
cargo run --quiet -p capnp-rpc-core --example m36_promise_resolution < "$fixture" \
  | "$build_root/m36-promise-resolution" verify

printf 'M36 promise resolution: pinned C++ -> native -> pinned C++ OK\n'
