#!/usr/bin/env bash
set -euo pipefail

commit=2228b71e55cee819c30450bb9bfd9c1f6a722429
repository=https://github.com/capnproto/capnproto-rust.git
oracle_root=${CAPNP_ORACLE_ROOT:-/opt/capnp-oracles}
checkout="$oracle_root/capnproto-rust-$commit/source"
build_worktree="$oracle_root/capnproto-rust-$commit/build-source"
build="$oracle_root/capnproto-rust-$commit/cargo-target"
prefix="$oracle_root/capnproto-rust-$commit/install"
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
lockfile="$repo_root/conformance/oracles/capnproto-rust/$commit/Cargo.lock"
lockfile_sha=b2ed64ce3f34009fe0822b4b86d7fd6ace0e7766eff5d516bd93907bea621c94

for command_name in cargo cp git install sha256sum; do
    if ! command -v "$command_name" >/dev/null; then
        printf 'required command is unavailable: %s\n' "$command_name" >&2
        exit 1
    fi
done

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
    printf 'oracle checkout mismatch: expected %s, got %s\n' \
        "$commit" "$actual_commit" >&2
    exit 1
fi

actual_lockfile_sha=$(sha256sum "$lockfile" | cut -d ' ' -f 1)
if [[ "$actual_lockfile_sha" != "$lockfile_sha" ]]; then
    printf 'oracle Cargo.lock mismatch: expected %s, got %s\n' \
        "$lockfile_sha" "$actual_lockfile_sha" >&2
    exit 1
fi

if [[ ! -e "$build_worktree/.git" ]]; then
    git -C "$checkout" worktree add --detach "$build_worktree" "$commit"
fi

worktree_commit=$(git -C "$build_worktree" rev-parse HEAD)
if [[ "$worktree_commit" != "$commit" ]]; then
    printf 'oracle build worktree mismatch: expected %s, got %s\n' \
        "$commit" "$worktree_commit" >&2
    exit 1
fi

cp -- "$lockfile" "$build_worktree/Cargo.lock"

cargo build \
    --locked \
    --manifest-path "$build_worktree/Cargo.toml" \
    --package capnpc \
    --bin capnpc-rust \
    --release \
    --target-dir "$build"

mkdir -p -- "$prefix/bin"
install -m 0755 "$build/release/capnpc-rust" "$prefix/bin/capnpc-rust"

binary_sha=$(sha256sum "$prefix/bin/capnpc-rust" | cut -d ' ' -f 1)
printf 'commit=%s\ncapnpc_version=0.27.0\nrustc=%s\nsha256=%s\ninstall=%s\n' \
    "$actual_commit" "$(rustc --version)" "$binary_sha" "$prefix"
