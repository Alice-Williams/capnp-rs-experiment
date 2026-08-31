#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
build_root=${CAPNP_M44_CPP_BUILD:-"$cpp_root/m44-level3-tests"}
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

# Exact upstream basic, self, forwarding-enabled, forwarding-denied,
# reflected-forwarding, and third-party embargo corpus.
cpp_output=$($test_binary --filter=rpc-test.c++:2267-2625 2>&1)
printf '%s\n' "$cpp_output"
grep -Fq '6 test(s) passed' <<<"$cpp_output"

cargo test --quiet -p capnp-rpc-core level_three_
cargo test --quiet -p capnp-rpc-core third_party_descriptor_keeps_the_vine
cargo test --quiet -p capnp-rpc --lib level3::tests
printf 'M44 Level-3 handoffs: pinned C++ corpus and native authenticated simulations OK\n'
