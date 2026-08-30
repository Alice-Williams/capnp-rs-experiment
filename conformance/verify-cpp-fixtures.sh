#!/usr/bin/env bash
set -euo pipefail

commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixture_dir="$repo_root/conformance/fixtures/cpp/$commit"

cd "$fixture_dir"
sha256sum --check SHA256SUMS
grep -Fx 'producer = "capnproto-c++"' PROVENANCE.toml
grep -Fx "commit = \"$commit\"" PROVENANCE.toml

expected_files=(
    compiler-request-evolution-v1.bin
    compiler-request-evolution-v2.bin
    compiler-request-evolution-v3.bin
    compiler-request-import-fixture.bin
    compiler-request-language-fixture.bin
    compiler-request-streaming-fixture.bin
    compiler-request-wire-fixture.bin
    evolution-v1-unpacked.bin
    language-unpacked.bin
    wire-flat.bin
    wire-multisegment.bin
    wire-packed.bin
    wire-unpacked.bin
)

for fixture in "${expected_files[@]}"; do
    test -s "$fixture"
    grep -Eq "^[0-9a-f]{64}  ${fixture}$" SHA256SUMS
done

cd "$repo_root"
printf '%s  %s\n' 52e2aef150349e65e3cb53bc78a73f437656b5322bffcdb3cc5f223ec2c5fa3b conformance/schemas/evolution-v1.capnp | sha256sum --check
printf '%s  %s\n' 491d63466427cff4289234eae0dae073c3f4c1efdc7a476d77246aef22a80c12 conformance/schemas/evolution-v2.capnp | sha256sum --check
printf '%s  %s\n' 1813b83c6b1b437786bf9eb858ef0d3852037e7fba6c21aa43b366b01b9781c3 conformance/schemas/evolution-v3.capnp | sha256sum --check
printf '%s  %s\n' 77b63f2c548c62f7ff30b971561b6659fe9a3aba0c115f0241c26a671a54116b conformance/schemas/import-fixture.capnp | sha256sum --check
printf '%s  %s\n' f2514581e686efdf18a4bf33305f48531cbcdf70541a89750c282c79955968a5 conformance/schemas/language-fixture.capnp | sha256sum --check
printf '%s  %s\n' 60fd1f08e21660d58652a62d846995a1b330514595f588142563525abf5da8e4 conformance/schemas/streaming-fixture.capnp | sha256sum --check
printf '%s  %s\n' 90033dafffbf663a85c6091c89964078553b023bb023b16bef8917f17a3a57c9 conformance/schemas/wire-fixture.capnp | sha256sum --check
printf '%s  %s\n' f028bc19fcd6e6268a8593c747a6229ab2bbab94b660e74fc5425468f7eca20d conformance/fixtures/source/evolution-v1.txt | sha256sum --check
printf '%s  %s\n' e98ea1269a0b0204d57b24f2b94956132c3aa0567c477a3ef802c43aace0fd15 conformance/fixtures/source/language-fixture.txt | sha256sum --check
printf '%s  %s\n' 216d5390813d8bf13838460cd81161bbb72b91c16c8bf85458fb835537c6f4fc conformance/fixtures/source/wire-fixture.txt | sha256sum --check
