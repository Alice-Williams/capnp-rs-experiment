#!/usr/bin/env bash
set -euo pipefail

commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
oracle="$oracle_root/capnproto-$commit/install/bin/capnp"
fixture_parent=conformance/fixtures/cpp
destination="$fixture_parent/$commit"

if [[ ! -x "$oracle" ]]; then
    printf 'pinned C++ oracle is unavailable; run tools/build-cpp-oracle.sh\n' >&2
    exit 1
fi

reported_version=$($oracle --version)
if [[ "$reported_version" != "Cap'n Proto version 2.0-dev" ]]; then
    printf 'unexpected oracle version: %s\n' "$reported_version" >&2
    exit 1
fi

actual_commit=$(git -C "$oracle_root/capnproto-$commit/source" rev-parse HEAD)
if [[ "$actual_commit" != "$commit" ]]; then
    printf 'oracle checkout mismatch: expected %s, got %s\n' \
        "$commit" "$actual_commit" >&2
    exit 1
fi

mkdir -p -- "$fixture_parent"
staging=$(mktemp -d "$fixture_parent/.cpp-$commit.XXXXXX")
cleanup() {
    if [[ -n "${staging:-}" && -d "$staging" ]]; then
        rm -rf -- "$staging"
    fi
}
trap cleanup EXIT

wire_schema=conformance/schemas/wire-fixture.capnp
wire_input=conformance/fixtures/source/wire-fixture.txt
language_schema=conformance/schemas/language-fixture.capnp
language_input=conformance/fixtures/source/language-fixture.txt
evolution_schema=conformance/schemas/evolution-v1.capnp
evolution_input=conformance/fixtures/source/evolution-v1.txt

"$oracle" encode "$wire_schema" WireFixture \
    < "$wire_input" > "$staging/wire-unpacked.bin"
"$oracle" encode --packed "$wire_schema" WireFixture \
    < "$wire_input" > "$staging/wire-packed.bin"
"$oracle" encode --flat "$wire_schema" WireFixture \
    < "$wire_input" > "$staging/wire-flat.bin"
"$oracle" encode --segment-size=1 "$wire_schema" WireFixture \
    < "$wire_input" > "$staging/wire-multisegment.bin"
"$oracle" encode "$language_schema" LanguageFixture \
    < "$language_input" > "$staging/language-unpacked.bin"
"$oracle" encode "$evolution_schema" Record \
    < "$evolution_input" > "$staging/evolution-v1-unpacked.bin"

schemas=(
    conformance/schemas/evolution-v1.capnp
    conformance/schemas/evolution-v2.capnp
    conformance/schemas/import-fixture.capnp
    conformance/schemas/language-fixture.capnp
    conformance/schemas/wire-fixture.capnp
)
for schema in "${schemas[@]}"; do
    schema_name=$(basename -- "$schema" .capnp)
    "$oracle" compile \
        --src-prefix=conformance/schemas \
        -Iconformance/schemas \
        -o- \
        "$schema" > "$staging/compiler-request-$schema_name.bin"
done

schema_sha=$(sha256sum "$wire_schema" | cut -d ' ' -f 1)
input_sha=$(sha256sum "$wire_input" | cut -d ' ' -f 1)
language_schema_sha=$(sha256sum "$language_schema" | cut -d ' ' -f 1)
language_input_sha=$(sha256sum "$language_input" | cut -d ' ' -f 1)
evolution_schema_sha=$(sha256sum "$evolution_schema" | cut -d ' ' -f 1)
evolution_input_sha=$(sha256sum "$evolution_input" | cut -d ' ' -f 1)

(
    cd "$staging"
    sha256sum -- *.bin > SHA256SUMS
    sha256sum --check SHA256SUMS
)

{
    printf 'producer = "capnproto-c++"\n'
    printf 'repository = "https://github.com/capnproto/capnproto.git"\n'
    printf 'commit = "%s"\n' "$commit"
    printf 'reported_version = "%s"\n' "$reported_version"
    printf 'generator = "tools/generate-cpp-fixtures.sh"\n'
    printf 'wire_schema = "%s"\n' "$wire_schema"
    printf 'wire_schema_sha256 = "%s"\n' "$schema_sha"
    printf 'wire_input = "%s"\n' "$wire_input"
    printf 'wire_input_sha256 = "%s"\n' "$input_sha"
    printf 'language_schema = "%s"\n' "$language_schema"
    printf 'language_schema_sha256 = "%s"\n' "$language_schema_sha"
    printf 'language_input = "%s"\n' "$language_input"
    printf 'language_input_sha256 = "%s"\n' "$language_input_sha"
    printf 'evolution_schema = "%s"\n' "$evolution_schema"
    printf 'evolution_schema_sha256 = "%s"\n' "$evolution_schema_sha"
    printf 'evolution_input = "%s"\n' "$evolution_input"
    printf 'evolution_input_sha256 = "%s"\n' "$evolution_input_sha"
    printf '\ncommands = [\n'
    printf '  "capnp encode wire-fixture.capnp WireFixture",\n'
    printf '  "capnp encode --packed wire-fixture.capnp WireFixture",\n'
    printf '  "capnp encode --flat wire-fixture.capnp WireFixture",\n'
    printf '  "capnp encode --segment-size=1 wire-fixture.capnp WireFixture",\n'
    printf '  "capnp encode language-fixture.capnp LanguageFixture",\n'
    printf '  "capnp encode evolution-v1.capnp Record",\n'
    printf '  "capnp compile --src-prefix=conformance/schemas -Iconformance/schemas -o- <schema>",\n'
    printf ']\n'
    for schema in "${schemas[@]}"; do
        schema_name=$(basename -- "$schema" .capnp)
        compiler_schema_sha=$(sha256sum "$schema" | cut -d ' ' -f 1)
        printf '\n[[compiler_request]]\n'
        printf 'schema = "%s"\n' "$schema"
        printf 'schema_sha256 = "%s"\n' "$compiler_schema_sha"
        printf 'output = "compiler-request-%s.bin"\n' "$schema_name"
    done
    while read -r output_sha output_name; do
        printf '\n[[output]]\n'
        printf 'path = "%s"\n' "$output_name"
        printf 'sha256 = "%s"\n' "$output_sha"
    done < "$staging/SHA256SUMS"
} > "$staging/PROVENANCE.toml"

if [[ -e "$destination" ]]; then
    backup="$fixture_parent/.previous-$commit-$$"
    if [[ -e "$backup" ]]; then
        printf 'refusing to overwrite unexpected backup: %s\n' "$backup" >&2
        exit 1
    fi
    mv -- "$destination" "$backup"
    if mv -- "$staging" "$destination"; then
        staging=
        rm -rf -- "$backup"
    else
        mv -- "$backup" "$destination"
        exit 1
    fi
else
    mv -- "$staging" "$destination"
    staging=
fi

printf 'generated=%s\ncommit=%s\n' "$destination" "$commit"
