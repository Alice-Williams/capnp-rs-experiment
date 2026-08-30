# M15 — Packed codec

- Status: planned
- Phase: 2
- Depends on: M03, M04

## Outcome

Implement streaming packed encode/decode for ordinary tags, zero runs, and raw runs.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Reference packed data interops; arbitrary chunks work; output is bounded; round-trip properties pass; malformed streams cannot loop/overallocate.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

