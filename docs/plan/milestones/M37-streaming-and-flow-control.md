# M37 — Streaming, adaptive flow control, and bounded backpressure

- Status: planned
- Phase: 6
- Depends on: M36

## Outcome

Add streaming semantics, fixed/adaptive windows, RTT/ack tracking, incoming flow limits, and quotas.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Per-stream order and cross-stream concurrency hold; slow peers bound memory; blocked senders wake; adaptive behavior matches pinned tests.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

