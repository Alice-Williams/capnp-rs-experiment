# Cap'n Proto in native Rust

This repository is a fresh experiment in implementing Cap'n Proto directly in
Rust. It intentionally starts from a minimal crate instead of importing the
previous prototype's implementation.

## Approach

- Treat the official Cap'n Proto specification and reference tooling as the
  compatibility oracle.
- Add behavior only with conformance fixtures, including cross-implementation
  tests where practical.
- Design readers for untrusted input from the beginning, with checked offsets,
  traversal limits, nesting limits, and allocation limits.
- Keep encoding, schema handling, code generation, and RPC as distinct layers.
- Prefer a small, correct subset over broad APIs that only round-trip against
  themselves.

## Plan and current scope

The original engineering dossier has been decomposed into 49 dependency-ordered
milestones under [`docs/plan`](docs/plan/README.md). The M01 oracle corpus and
baseline harnesses and the M02 safety/concurrency ADRs are technically complete.
M00 still awaits the owner's license choice and confirmation of the first hosted
CI run before wire implementation begins.

The repository is now an eleven-crate workspace matching the intended
architecture. Only `capnp-wire` preserves the initial word-size smoke test;
the other crates are explicit ownership boundaries, not implemented features.

## Development

On non-Linux hosts, run commands in the repository's Dev Container:

```console
cargo test --workspace --all-targets
bazel test //...
```

The project intentionally supports both build systems: Cargo is the native
Rust package/MSRV interface, while Bazel is the pinned Linux orchestration and
conformance environment.

This is experimental software and is not yet suitable for production use.

## License

Licensed under the [MIT License](LICENSE).
