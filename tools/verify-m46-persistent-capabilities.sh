#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
schema="$repo_root/conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/persistent.capnp"

grep -Fq 'interface Persistent@0xc8cb212fcd9f5691(SturdyRef, Owner)' "$schema"
grep -Fq 'save @0 SaveParams -> SaveResults;' "$schema"
grep -Fq 'sealFor @0 :Owner;' "$schema"
grep -Fq 'sturdyRef @0 :SturdyRef;' "$schema"
grep -Fq 'annotation persistent(interface, field) :Void;' "$schema"

cargo test --quiet -p capnp-rpc --lib persistence::tests
cargo test --quiet -p capnp-rpc --doc persistence
printf 'M46 persistent capabilities: pinned schema and native restart/security model OK\n'
