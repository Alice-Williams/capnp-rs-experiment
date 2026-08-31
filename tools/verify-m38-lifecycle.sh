#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
build_root=${CAPNP_M38_CPP_BUILD:-"$cpp_root/m38-lifecycle-tests"}
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

# Exact tests covering cancellation/release, tail cancellation races,
# disconnect error handling, retained disconnect details, and clean shutdown.
for line in 891 1010 1040 1075 1135 1274 1837 1966 2235; do
    output=$("$test_binary" --filter="rpc-test.c++:$line" 2>&1)
    printf '%s\n' "$output"
    grep -Fq '1 test(s) passed' <<<"$output"
done

# reconnect-test.c++ is not linked into the upstream CMake test binaries. Pin
# its six public behavior cases as the source oracle; the native tests below
# execute the corresponding disconnect-only recreation and stale-generation
# behavior.
reconnect_source="$cpp_root/source/c++/src/capnp/reconnect-test.c++"
for line in 198 205 221 228 244 251; do
    sed -n "${line}p" "$reconnect_source" | grep -Fq 'KJ_TEST('
done

cargo test --quiet -p capnp-rpc-core actor::tests
cargo test --quiet -p capnp-rpc reconnect::tests
cargo test --quiet -p capnp-rpc driver::tests::shutdown_future
printf 'M38 lifecycle: pinned C++ corpus and native race/reconnect suites OK\n'
