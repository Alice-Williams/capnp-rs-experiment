#!/usr/bin/env bash
set -euo pipefail

cpp_commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
rust_commit=2228b71e55cee819c30450bb9bfd9c1f6a722429
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
cpp_source="$oracle_root/capnproto-$cpp_commit/source"
cpp_oracle="$oracle_root/capnproto-$cpp_commit/install/bin/capnp"
rust_source="$oracle_root/capnproto-rust-$rust_commit/source"
rust_oracle="$oracle_root/capnproto-rust-$rust_commit/install/bin/capnpc-rust"
fixture_parent=conformance/fixtures/capnproto-rust
destination="$fixture_parent/$rust_commit"

for executable in "$cpp_oracle" "$rust_oracle"; do
    if [[ ! -x "$executable" ]]; then
        printf 'oracle executable is unavailable: %s\n' "$executable" >&2
        exit 1
    fi
done

actual_cpp_commit=$(git -C "$cpp_source" rev-parse HEAD)
actual_rust_commit=$(git -C "$rust_source" rev-parse HEAD)
if [[ "$actual_cpp_commit" != "$cpp_commit" ]]; then
    printf 'C++ oracle mismatch: expected %s, got %s\n' \
        "$cpp_commit" "$actual_cpp_commit" >&2
    exit 1
fi
if [[ "$actual_rust_commit" != "$rust_commit" ]]; then
    printf 'Rust oracle mismatch: expected %s, got %s\n' \
        "$rust_commit" "$actual_rust_commit" >&2
    exit 1
fi

mkdir -p -- "$fixture_parent"
staging=$(mktemp -d "$fixture_parent/.rust-$rust_commit.XXXXXX")
cleanup() {
    if [[ -n "${staging:-}" && -d "$staging" ]]; then
        rm -rf -- "$staging"
    fi
}
trap cleanup EXIT

schemas=(
    conformance/schemas/evolution-v1.capnp
    conformance/schemas/evolution-v2.capnp
    conformance/schemas/import-fixture.capnp
    conformance/schemas/language-fixture.capnp
    conformance/schemas/wire-fixture.capnp
)

for schema in "${schemas[@]}"; do
    "$cpp_oracle" compile \
        --src-prefix=conformance/schemas \
        -Iconformance/schemas \
        -o"$rust_oracle":"$staging" \
        "$schema"
done

(
    cd "$staging"
    sha256sum -- *.rs > SHA256SUMS
    sha256sum --check SHA256SUMS
)

cpp_version=$($cpp_oracle --version)
rust_binary_sha=$(sha256sum "$rust_oracle" | cut -d ' ' -f 1)
{
    printf 'producer = "capnproto-rust-capnpc"\n'
    printf 'repository = "https://github.com/capnproto/capnproto-rust.git"\n'
    printf 'commit = "%s"\n' "$rust_commit"
    printf 'capnpc_version = "0.27.0"\n'
    printf 'binary_sha256 = "%s"\n' "$rust_binary_sha"
    printf 'request_producer = "capnproto-c++"\n'
    printf 'request_producer_commit = "%s"\n' "$cpp_commit"
    printf 'request_producer_version = "%s"\n' "$cpp_version"
    printf 'generator = "tools/generate-rust-fixtures.sh"\n'
    printf 'command = "capnp compile --src-prefix=conformance/schemas -Iconformance/schemas -o<capnpc-rust>:<output> <schema>"\n'
    for schema in "${schemas[@]}"; do
        schema_name=$(basename -- "$schema" .capnp | tr - _)
        schema_sha=$(sha256sum "$schema" | cut -d ' ' -f 1)
        printf '\n[[schema]]\n'
        printf 'path = "%s"\n' "$schema"
        printf 'sha256 = "%s"\n' "$schema_sha"
        printf 'output = "%s_capnp.rs"\n' "$schema_name"
    done
    while read -r output_sha output_name; do
        printf '\n[[output]]\n'
        printf 'path = "%s"\n' "$output_name"
        printf 'sha256 = "%s"\n' "$output_sha"
    done < "$staging/SHA256SUMS"
} > "$staging/PROVENANCE.toml"

if [[ -e "$destination" ]]; then
    backup="$fixture_parent/.previous-$rust_commit-$$"
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

printf 'generated=%s\ncommit=%s\n' "$destination" "$rust_commit"
