# M16 — Sync, async, mmap, and no-allocation adapters

- Status: complete
- Phase: 2
- Depends on: M04, M10, M15

## Outcome

Layer sync and executor-neutral async framing, bounded writers, mmap borrowing, and caller-buffer/no-allocation reads.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Every partial-I/O boundary and cancellation is tested; order/backpressure hold; mmap is zero-copy; std/no_std+alloc/no-alloc matrix builds.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
