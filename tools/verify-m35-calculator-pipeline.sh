#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_prefix="$oracle_root/capnproto-$cpp_commit/install"
build_root="$oracle_root/capnproto-$cpp_commit/m35-calculator-pipeline"
source_file="$repo_root/conformance/oracle/m35_calculator_pipeline.c++"
fixture="$repo_root/conformance/fixtures/cpp/$cpp_commit/rpc-calculator-pipeline.bin"

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null
mkdir -p -- "$build_root"
clang++ -std=c++23 -O2 -pthread \
  -I"$cpp_prefix/include" "$source_file" \
  -L"$cpp_prefix/lib" -lcapnp-rpc -lcapnp -lkj \
  -o "$build_root/m35-calculator-pipeline"

generated="$build_root/rpc-calculator-pipeline.bin"
"$build_root/m35-calculator-pipeline" generate > "$generated"
cmp -- "$generated" "$fixture"
cargo run --quiet -p capnp-rpc-core --example m35_calculator_pipeline < "$fixture" \
  | "$build_root/m35-calculator-pipeline" verify

printf 'M35 calculator promise pipeline: pinned C++ -> native -> pinned C++ OK\n'
