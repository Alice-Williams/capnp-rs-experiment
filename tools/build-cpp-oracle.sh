#!/usr/bin/env bash
set -euo pipefail

commit=e7c9cd96f1505b5ae486db7821006c2f5dce5b5b
repository=https://github.com/capnproto/capnproto.git
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
checkout="$oracle_root/capnproto-$commit/source"

for command_name in cmake git ninja; do
    if ! command -v "$command_name" >/dev/null; then
        printf 'required command is unavailable: %s\n' "$command_name" >&2
        exit 1
    fi
done

if [[ -n "${CXX:-}" ]]; then
    cxx=$CXX
elif command -v clang++ >/dev/null; then
    cxx=$(command -v clang++)
elif command -v c++ >/dev/null; then
    cxx=$(command -v c++)
else
    printf 'no C++ compiler is available\n' >&2
    exit 1
fi

compiler_name=$(basename -- "$cxx")
compiler_version=$($cxx -dumpversion)
build="$oracle_root/capnproto-$commit/build-$compiler_name-$compiler_version"
prefix="$oracle_root/capnproto-$commit/install"

mkdir -p -- "$oracle_root"

if [[ ! -d "$checkout/.git" ]]; then
    git clone --filter=blob:none "$repository" "$checkout"
else
    actual_remote=$(git -C "$checkout" remote get-url origin)
    if [[ "$actual_remote" != "$repository" ]]; then
        printf 'unexpected oracle remote: %s\n' "$actual_remote" >&2
        exit 1
    fi
fi

if [[ -n "$(git -C "$checkout" status --porcelain)" ]]; then
    printf 'refusing to change dirty oracle checkout: %s\n' "$checkout" >&2
    exit 1
fi

git -C "$checkout" fetch --depth=1 origin "$commit"
git -C "$checkout" checkout --detach "$commit"

actual_commit=$(git -C "$checkout" rev-parse HEAD)
if [[ "$actual_commit" != "$commit" ]]; then
    printf 'oracle checkout mismatch: expected %s, got %s\n' "$commit" "$actual_commit" >&2
    exit 1
fi

cmake \
    -S "$checkout" \
    -B "$build" \
    -G Ninja \
    -DCMAKE_CXX_COMPILER="$cxx" \
    -DBUILD_TESTING=OFF \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$prefix"
cmake --build "$build" --parallel
cmake --install "$build"

"$prefix/bin/capnp" --version
printf 'commit=%s\ncompiler=%s %s\ninstall=%s\n' \
    "$actual_commit" "$compiler_name" "$compiler_version" "$prefix"
