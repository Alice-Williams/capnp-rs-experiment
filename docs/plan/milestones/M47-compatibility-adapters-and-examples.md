# M47 — C++ compatibility adapters and examples

- Status: complete
- Phase: 7
- Depends on: M28, M41, M42, M43, M44, M45, M46

## Outcome

Add byte streams, JSON-RPC, HTTP/WebSocket adapters, and substantial address-book/calculator examples.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Pinned adapter behaviors and interop pass; examples cover pipelines, callbacks, streaming, cancellation, concurrency, handoff, and persistence.

## Implementation evidence

All four `tools/verify-m47-*.sh` gates pass against pinned C++ commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The native examples additionally
cover distributed equality and decode a native address-book frame with the
pinned C++ tool. Workspace all-target tests, doctests, strict Clippy, rustdoc,
shell syntax, and all 30 Bazel tests pass. The M40 release gate and all M41-M46
dependencies are complete.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
