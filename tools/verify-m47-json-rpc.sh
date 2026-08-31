#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
source_root="$cpp_root/source/c++/src"
install_root="$cpp_root/install"
support_root=${CAPNP_M47_JSON_RPC_SUPPORT:-"$cpp_root/m47-json-rpc-support"}
build_root=${CAPNP_M47_JSON_RPC_BUILD:-"$cpp_root/m47-json-rpc-tests"}
generated_tests="$support_root/c++/src/capnp/test_capnp"
test_binary="$build_root/json-rpc-tests"

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null

if [[ ! -f "$generated_tests/capnp/test.capnp.c++" ]]; then
    cxx=$(command -v clang++-19 || command -v clang++)
    cc=$(command -v clang-19 || command -v clang)
    CC="$cc" CXX="$cxx" cmake \
      -S "$cpp_root/source" \
      -B "$support_root" \
      -DBUILD_TESTING=ON \
      -DBUILD_SHARED_LIBS=OFF \
      -DCMAKE_BUILD_TYPE=Release >/dev/null
    cmake --build "$support_root" --target test_capnp -j4 >/dev/null
fi

if [[ ! -x "$test_binary" ]]; then
    cxx=$(command -v clang++-19 || command -v clang++)
    mkdir -p -- "$build_root"
    (
      cd -- "$source_root"
      "$install_root/bin/capnp" compile -I. \
        -o"$install_root/bin/capnpc-c++":"$build_root" \
        capnp/compat/json-rpc.capnp
    )
    "$cxx" -std=c++23 -O2 -pthread \
      -I"$generated_tests" -I"$build_root" \
      -I"$source_root" -I"$install_root/include" \
      "$source_root/capnp/compat/json-rpc-test.c++" \
      "$source_root/capnp/compat/json-rpc.c++" \
      "$build_root/capnp/compat/json-rpc.capnp.c++" \
      "$source_root/capnp/test-util.c++" \
      "$generated_tests/capnp/test.capnp.c++" \
      "$generated_tests/capnp/test-import.capnp.c++" \
      "$generated_tests/capnp/test-import2.capnp.c++" \
      "$generated_tests/capnp/compat/json-test.capnp.c++" \
      -L"$install_root/lib" \
      -lcapnp-websocket -lcapnp-json -lcapnp-rpc -lcapnp -lcapnpc \
      -lkj-http -lkj-async -lkj-test -lkj -lz \
      -o "$test_binary"
fi

cpp_output=$($test_binary --filter=json-rpc-test.c++ 2>&1)
printf '%s\n' "$cpp_output"
grep -Fq '3 test(s) passed' <<<"$cpp_output"

cargo test --quiet -p capnp-compat --lib json_rpc::tests
cargo test --quiet -p capnp-compat --doc json_rpc
printf 'M47 JSON-RPC: all pinned C++ cases and native mapping/framing cases OK\n'
