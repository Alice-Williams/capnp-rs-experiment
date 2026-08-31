#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
source_root="$cpp_root/source/c++/src"
install_root="$cpp_root/install"
build_root=${CAPNP_M47_BYTE_STREAM_BUILD:-"$cpp_root/m47-byte-stream-tests"}
test_binary="$build_root/byte-stream-tests"

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null

if [[ ! -x "$test_binary" ]]; then
    cxx=$(command -v clang++-19 || command -v clang++)
    mkdir -p -- "$build_root"
    (
      cd -- "$source_root"
      "$install_root/bin/capnp" compile -I. \
        -o"$install_root/bin/capnpc-c++":"$build_root" \
        capnp/compat/byte-stream.capnp
    )
    "$cxx" -std=c++23 -O2 -pthread \
      -I"$install_root/include" -I"$build_root" -I"$source_root" \
      "$source_root/capnp/compat/byte-stream-test.c++" \
      "$source_root/capnp/compat/byte-stream.c++" \
      "$build_root/capnp/compat/byte-stream.capnp.c++" \
      -L"$install_root/lib" \
      -lcapnp-websocket -lcapnp-json -lcapnp-rpc -lcapnp \
      -lkj-http -lkj-async -lkj-test -lkj \
      -o "$test_binary"
fi

cpp_output=$($test_binary --filter=byte-stream-test.c++ 2>&1)
printf '%s\n' "$cpp_output"
grep -Fq '12 test(s) passed' <<<"$cpp_output"

cargo test --quiet -p capnp-compat --lib byte_stream::tests
cargo test --quiet -p capnp-compat --doc byte_stream
printf 'M47 ByteStream: all pinned C++ cases and native lifecycle/shortening cases OK\n'
