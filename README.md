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

## Initial scope

The crate currently defines only the wire word size and a smoke test. The first
implementation milestone is message framing and bounded segment-table parsing,
validated against bytes produced by the reference implementation. Pointer and
layout support comes next; schema/code generation and RPC remain out of scope
until the wire layer is demonstrably compatible.

## Development

```console
cargo test
```

This is experimental software and is not yet suitable for production use.
