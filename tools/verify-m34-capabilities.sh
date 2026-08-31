#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_prefix="$oracle_root/capnproto-$cpp_commit/install"
build_root="$oracle_root/capnproto-$cpp_commit/m34-capability-interop"
source_file="$repo_root/conformance/oracle/m34_capability_interop.c++"
fixture="$repo_root/conformance/fixtures/cpp/$cpp_commit/rpc-capability-call.bin"

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
mkdir -p -- "$build_root"
clang++ -std=c++23 -O2 -pthread \
  -I"$cpp_prefix/include" "$source_file" \
  -L"$cpp_prefix/lib" -lcapnp-rpc -lcapnp -lkj \
  -o "$build_root/m34-capability-interop"

generated="$build_root/rpc-capability-call.bin"
"$build_root/m34-capability-interop" generate > "$generated"
cmp -- "$generated" "$fixture"
"$build_root/m34-capability-interop" verify < "$fixture"
cargo run --quiet -p capnp-rpc-core --example m34_capability_frame < "$fixture" \
  | "$build_root/m34-capability-interop" verify

printf 'M34 capability payload interop: pinned C++ -> native -> pinned C++ OK\n'
