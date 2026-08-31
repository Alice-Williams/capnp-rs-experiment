#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
build_root=${CAPNP_M37_CPP_BUILD:-"$cpp_root/m37-flow-control-tests"}
test_binary="$build_root/c++/src/capnp/capnp-heavy-tests"

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null

if [[ ! -x "$test_binary" ]]; then
    cxx=$(command -v clang++-19 || command -v clang++)
    cc=$(command -v clang-19 || command -v clang)
    CC="$cc" CXX="$cxx" cmake \
      -S "$cpp_root/source" \
      -B "$build_root" \
      -DBUILD_TESTING=ON \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_BUILD_TYPE=Release >/dev/null
    cmake --build "$build_root" --target capnp-heavy-tests -j4 >/dev/null
fi

output=$(
  "$test_binary" --filter=rpc-test.c++:2674-3060 2>&1
)
printf '%s\n' "$output"
grep -Fq '10 test(s) passed' <<<"$output"

cargo test --quiet -p capnp-rpc flow::tests
printf 'M37 flow control: pinned C++ corpus and native port OK\n'
