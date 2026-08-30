#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
schema_dir="$repo_root/conformance/upstream/capnproto/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b"

cd -- "$schema_dir"
printf '%s  %s\n' 3557ff301cec23ef90f59ba2265a741f8fe59ba63bf0e5d12b1619cfc8d74c8d schema.capnp | sha256sum --check
printf '%s  %s\n' 2ecc3049d4f7f2d48a3a368dbb9ef4b97b31c1365996d615bd19c267983a1931 rpc.capnp | sha256sum --check
printf '%s  %s\n' 22680f70c56e3c44dc73b52bf8dfd2838a5ea44249be01609be2d362d308b518 rpc-twoparty.capnp | sha256sum --check
printf '%s  %s\n' d77d4d2e2c1e9c42ded13de54ed11b535b076ac210945b53ecf76fd7648a867a persistent.capnp | sha256sum --check
printf '%s  %s\n' 5b0656ca3daca9ef28740c14813d5dd474fd0f9991ce99f652838f4cccf6fb30 stream.capnp | sha256sum --check
