#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
oracle_root=/opt/capnp-oracles/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

cd -- "$repo_root"
cargo run -q -p capnp-io --example m30_parallel_builder_fixture >"$work/frame.bin"

cat >"$work/verify.c++" <<'EOF'
#include <capnp/serialize.h>
#include <kj/io.h>

#include <iostream>

int main() {
  kj::FdInputStream input(0);
  capnp::InputStreamMessageReader message(input);
  auto items = message.getRoot<capnp::List<capnp::Text>>();
  for (auto item: items) {
    std::cout << item.cStr() << '\n';
  }
}
EOF

c++ -std=c++20 -O2 \
    -I"$oracle_root/install/include" \
    "$work/verify.c++" \
    -L"$oracle_root/install/lib" \
    -Wl,-rpath,"$oracle_root/install/lib" \
    -lcapnp -lkj -pthread \
    -o "$work/verify"

"$work/verify" <"$work/frame.bin" >"$work/actual.txt"
for index in 0 1 2 3 4 5; do
    printf 'worker-item-%s\n' "$index"
done >"$work/expected.txt"
diff -u -- "$work/expected.txt" "$work/actual.txt"
printf 'm30-cpp-decode-ok  %s bytes\n' "$(wc -c <"$work/frame.bin")"
