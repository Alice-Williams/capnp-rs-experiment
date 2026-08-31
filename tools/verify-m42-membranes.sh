#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
build_root=${CAPNP_M42_CPP_BUILD:-"$cpp_root/m42-membrane-tests"}
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

revocable_output=$("$test_binary" --filter=capability-test.c++:1420 2>&1)
printf '%s\n' "$revocable_output"
grep -Fq '1 test(s) passed' <<<"$revocable_output"

# Exact local/remote object, promise, reflection, copy, concurrent-resolution,
# and revoke corpus. The next test begins outside this source range.
membrane_output=$("$test_binary" --filter=membrane-test.c++:189-398 2>&1)
printf '%s\n' "$membrane_output"
grep -Fq '17 test(s) passed' <<<"$membrane_output"

cargo test --quiet -p capnp-generated-fixture membrane_
cargo test --quiet -p capnp-generated-fixture revocable_server_
printf 'M42 membranes: pinned C++ revocation/membrane corpus and native ports OK\n'
