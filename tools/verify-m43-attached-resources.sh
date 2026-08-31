#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
build_root=${CAPNP_M43_CPP_BUILD:-"$cpp_root/m43-attached-resource-tests"}
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

# Exact send/receive, per-message-limit, and queued-write lifetime corpus.
cpp_output=$("$test_binary" --filter=rpc-twoparty-test.c++:549-795 2>&1)
printf '%s\n' "$cpp_output"
grep -Fq '3 test(s) passed' <<<"$cpp_output"

cargo test --quiet -p capnp-rpc-core attached
cargo test --quiet -p capnp-rpc-core resource_binding
cargo test --quiet -p capnp-rpc-core actor_sends_and_binds_attached_capability_resources
cargo test --quiet -p capnp-rpc --lib unix_transport
printf 'M43 attached resources: pinned C++ corpus and native ownership/Unix ports OK\n'
