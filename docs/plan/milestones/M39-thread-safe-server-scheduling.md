# M39 — Thread-safe server scheduling policies

- Status: complete
- Phase: 6
- Depends on: M33

## Outcome

Provide Concurrent, Serial, Keyed, LocalServer, Tokio, and generic-executor adapters.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Overlap/order policies prove their contracts; local state stays isolated behind Send clients; throughput, fairness, and p99 are recorded.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
