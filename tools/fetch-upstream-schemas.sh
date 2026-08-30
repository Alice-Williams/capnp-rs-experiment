#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
destination="$repo_root/conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b"
mkdir -p -- "$destination"

fetch_schema() {
    local name=$1
    local expected=$2
    local url=$3
    local temporary
    local actual

    temporary=$(mktemp)
    trap 'rm -f -- "$temporary"' RETURN
    curl --fail --silent --show-error --location --output "$temporary" "$url"
    actual=$(sha256sum "$temporary" | awk '{print $1}')
    if [[ "$actual" != "$expected" ]]; then
        printf 'hash mismatch for %s: expected %s, got %s\n' "$name" "$expected" "$actual" >&2
        return 1
    fi
    install -m 0644 "$temporary" "$destination/$name"
    printf '%s  %s\n' "$actual" "$name"
    rm -f -- "$temporary"
    trap - RETURN
}

base='https://raw.githubusercontent.com/capnproto/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/c%2B%2B/src/capnp'

fetch_schema schema.capnp 3557ff301cec23ef90f59ba2265a741f8fe59ba63bf0e5d12b1619cfc8d74c8d "$base/schema.capnp"
fetch_schema rpc.capnp 2ecc3049d4f7f2d48a3a368dbb9ef4b97b31c1365996d615bd19c267983a1931 "$base/rpc.capnp"
fetch_schema rpc-twoparty.capnp 22680f70c56e3c44dc73b52bf8dfd2838a5ea44249be01609be2d362d308b518 "$base/rpc-twoparty.capnp"
fetch_schema persistent.capnp d77d4d2e2c1e9c42ded13de54ed11b535b076ac210945b53ecf76fd7648a867a "$base/persistent.capnp"
