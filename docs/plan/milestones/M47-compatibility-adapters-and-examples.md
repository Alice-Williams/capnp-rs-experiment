# M47 — C++ compatibility adapters and examples

- Status: in progress on `dev/m47-compatibility-adapters`
- Phase: 7
- Depends on: M28, M41, M42, M43, M44, M45, M46

## Outcome

Add byte streams, JSON-RPC, HTTP/WebSocket adapters, and substantial address-book/calculator examples.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Pinned adapter behaviors and interop pass; examples cover pipelines, callbacks, streaming, cancellation, concurrency, handoff, and persistence.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
