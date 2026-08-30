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
    import_fixture_capnp.rs
    language_fixture_capnp.rs
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
printf '%s  %s\n' 77b63f2c548c62f7ff30b971561b6659fe9a3aba0c115f0241c26a671a54116b conformance/schemas/import-fixture.capnp | sha256sum --check
printf '%s  %s\n' 9866cc6d7246b8520f48e55ca542a42b020b3cd371965569945964c098d64816 conformance/schemas/language-fixture.capnp | sha256sum --check
printf '%s  %s\n' 425508a1fa43e56660a78f30605e1f083c7d9f49579b7b3979204ab30d2972f6 conformance/schemas/wire-fixture.capnp | sha256sum --check
