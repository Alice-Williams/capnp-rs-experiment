#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
schema="$repo_root/conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/rpc.capnp"
twoparty_schema="$repo_root/conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/rpc-twoparty.capnp"

# This pinned C++ revision specifies Join in schemas and pseudo-interfaces but
# has no Join runtime implementation or test corpus. Verify the normative wire
# declarations and the concrete two-party example before native model tests.
grep -Fq 'join @12 :Join;' "$schema"
grep -Fq 'struct Join {' "$schema"
grep -Fq 'using JoinKeyPart = AnyPointer;' "$schema"
grep -Fq 'using JoinResult = AnyPointer;' "$schema"
grep -Fq 'newJoiner(count :UInt32) :NewJoinerResponse;' "$schema"
grep -Fq 'struct JoinKeyPart {' "$twoparty_schema"
grep -Fq 'struct JoinResult {' "$twoparty_schema"

cargo test --quiet -p capnp-rpc-core level_four_join_and_join_result_round_trip_opaque_network_values
cargo test --quiet -p capnp-rpc --lib join::tests
cargo test --quiet -p capnp-rpc --doc join
printf 'M45 Level-4 Join: pinned schema surface and native authenticated threat model OK\n'
