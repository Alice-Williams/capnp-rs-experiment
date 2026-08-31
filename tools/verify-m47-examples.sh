#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cpp_root="$oracle_root/capnproto-$cpp_commit"
sample_root="$cpp_root/source/c++/samples"
capnp="$cpp_root/install/bin/capnp"
work_root=$(mktemp -d)
trap 'rm -rf -- "$work_root"' EXIT

bash "$repo_root/tools/build-cpp-oracle.sh" >/dev/null

output=$(cargo run --quiet -p capnp-examples --bin m47_examples -- \
  --addressbook-frame "$work_root/addressbook.bin")
printf '%s\n' "$output"
grep -Fq 'address-book: 123|Alice|' <<<"$output"
grep -Fq 'address-book: 456|Bob|' <<<"$output"
grep -Fq 'calculator: operator=42 callback=42 defined=42 concurrent=[11.0, 31.0] callback-calls=2' <<<"$output"
grep -Fq 'platform: stream=ordered stream discarded ends=1 cancellations=1 handoff=true equality=true restart=44->900 object=7' <<<"$output"

decoded=$(
  "$capnp" decode \
    -I"$sample_root" \
    "$sample_root/addressbook.capnp" AddressBook \
    < "$work_root/addressbook.bin"
)
printf '%s\n' "$decoded"
grep -Fq 'name = "Alice"' <<<"$decoded"
grep -Fq 'name = "Bob"' <<<"$decoded"
grep -Fq 'school = "MIT"' <<<"$decoded"
grep -Fq 'type = work' <<<"$decoded"

bash "$repo_root/tools/verify-m35-calculator-pipeline.sh" >/dev/null
cargo test --quiet -p capnp-examples --all-targets
printf 'M47 examples: native scenarios and pinned C++ interop OK\n'
