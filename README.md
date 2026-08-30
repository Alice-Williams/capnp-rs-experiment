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
milestones under [`docs/plan`](docs/plan/README.md). The Phase 0 foundation,
oracle corpus, benchmark harnesses, and safety/concurrency ADRs are complete.
Implementation now proceeds through the conformance-first wire milestones.

The repository is an eleven-crate workspace matching the intended architecture.
`capnp-wire` implements M03's no_std words, little-endian scalar access, checked
ranges, and raw pointer bitfields. `capnp-io` implements M04's bounded standard
unpacked framing over immutable segment descriptors. `capnp-message` implements
M05's coordinate-only, bounds-checked pointer validation and M06's exact local
and concurrent traversal limits, copied nesting limits, amplification defense,
and iterative hostile-depth traversal. M07 adds default-aware primitive and
enum reads plus charged, zero-copy Text and Data views. The remaining crates
are explicit ownership boundaries rather than claimed protocol features.

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
