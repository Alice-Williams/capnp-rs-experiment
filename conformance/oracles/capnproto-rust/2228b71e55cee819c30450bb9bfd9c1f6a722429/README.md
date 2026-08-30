# Pinned current-Rust oracle

- Repository: <https://github.com/capnproto/capnproto-rust.git>
- Commit: `2228b71e55cee819c30450bb9bfd9c1f6a722429`
- `capnp` / `capnpc` version: 0.27.0
- Upstream MSRV: Rust 1.81.0
- Reference build toolchain: Rust 1.98.0 on Debian Trixie, Linux x86-64
- Build script: `tools/build-rust-oracle.sh`
- Dependency lock SHA-256:
  `b2ed64ce3f34009fe0822b4b86d7fd6ace0e7766eff5d516bd93907bea621c94`
- Persistent install:
  `/opt/capnp-oracles/capnproto-rust-2228b71e55cee819c30450bb9bfd9c1f6a722429/install`

This is a secondary regression and Rust-ergonomics oracle. It does not define
the product target. The C++ implementation remains the primary compatibility
oracle.

The checked-in `Cargo.lock` freezes the otherwise-unlocked upstream workspace.
The build uses it in a detached worktree and installs only the pinned
`capnpc-rust` plugin. Source, Cargo output, and the installation remain in the
persistent oracle volume and are excluded from Git and the Docker image.
