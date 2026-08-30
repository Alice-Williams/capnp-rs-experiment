#!/usr/bin/env bash
set -euo pipefail

rust_commit=2228b71e55cee819c30450bb9bfd9c1f6a722429
cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixture_dir="$repo_root/conformance/fixtures/capnproto-rust/$rust_commit"

cd "$fixture_dir"
sha256sum --check SHA256SUMS
grep -Fx 'producer = "capnproto-rust-capnpc"' PROVENANCE.toml
grep -Fx "commit = \"$rust_commit\"" PROVENANCE.toml
grep -Fx "request_producer_commit = \"$cpp_commit\"" PROVENANCE.toml

expected_files=(
    evolution_v1_capnp.rs
    evolution_v2_capnp.rs
    evolution_v3_capnp.rs
    import_fixture_capnp.rs
    language_fixture_capnp.rs
    streaming_fixture_capnp.rs
    wire_fixture_capnp.rs
)

for fixture in "${expected_files[@]}"; do
    test -s "$fixture"
    grep -Eq "^[0-9a-f]{64}  ${fixture}$" SHA256SUMS
done
while read -r output_sha output_name; do
    grep -F "path = \"$output_name\"" PROVENANCE.toml >/dev/null
    grep -F "sha256 = \"$output_sha\"" PROVENANCE.toml >/dev/null
done < SHA256SUMS

cd "$repo_root"
printf '%s  %s\n' 52e2aef150349e65e3cb53bc78a73f437656b5322bffcdb3cc5f223ec2c5fa3b conformance/schemas/evolution-v1.capnp | sha256sum --check
printf '%s  %s\n' 491d63466427cff4289234eae0dae073c3f4c1efdc7a476d77246aef22a80c12 conformance/schemas/evolution-v2.capnp | sha256sum --check
printf '%s  %s\n' 1813b83c6b1b437786bf9eb858ef0d3852037e7fba6c21aa43b366b01b9781c3 conformance/schemas/evolution-v3.capnp | sha256sum --check
printf '%s  %s\n' 77b63f2c548c62f7ff30b971561b6659fe9a3aba0c115f0241c26a671a54116b conformance/schemas/import-fixture.capnp | sha256sum --check
printf '%s  %s\n' f2514581e686efdf18a4bf33305f48531cbcdf70541a89750c282c79955968a5 conformance/schemas/language-fixture.capnp | sha256sum --check
printf '%s  %s\n' 60fd1f08e21660d58652a62d846995a1b330514595f588142563525abf5da8e4 conformance/schemas/streaming-fixture.capnp | sha256sum --check
printf '%s  %s\n' 90033dafffbf663a85c6091c89964078553b023bb023b16bef8917f17a3a57c9 conformance/schemas/wire-fixture.capnp | sha256sum --check
