# M31 — Batch codec and pipeline scheduling

- Status: planned
- Phase: 5
- Depends on: M15, M16

## Outcome

Process independent message read/build/pack work concurrently while preserving stream order and bounded memory.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Slow writers remain bounded; work stealing never reorders output/RPC; multi-message throughput scales; single-message paths avoid pools.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

