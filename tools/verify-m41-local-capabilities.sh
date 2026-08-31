#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
build_root=${CAPNP_M41_CPP_BUILD:-"$cpp_root/m41-capability-tests"}
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

# The exact upstream capability corpus from basic local clients through clone
# covers capability lists, inheritance/generics/implicit parameters, response
# and provisional pipelines, tail calls, dynamic clients/servers, ServerSet,
# this/transfer, nested RemotePromise reduction, and capability-aware clone.
# RevocableServer begins at line 1420 and remains explicitly owned by M42.
output=$("$test_binary" --filter=capability-test.c++:44-1210 2>&1)
printf '%s\n' "$output"
grep -Fq '28 test(s) passed' <<<"$output"

cargo test --quiet -p capnp-generated-fixture \
  promise_clients_queue_in_order_and_fail_stably
cargo test --quiet -p capnp-generated-fixture \
  response_and_provisional_pipelines_preserve_local_client_identity
cargo test --quiet -p capnp-generated-fixture \
  dynamic_inheritance_pipeline_tail_call_and_server_set_obey_capability_rules

printf 'M41 local capabilities: pinned C++ corpus and native behavioral ports OK\n'
