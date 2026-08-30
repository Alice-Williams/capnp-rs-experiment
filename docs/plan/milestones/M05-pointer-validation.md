# M05 — Struct, list, far, and capability pointer validation

- Status: planned
- Phase: 1
- Depends on: M03, M04

## Outcome

Follow every pointer kind lazily with bounds-checked concrete wire reader results.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Null, struct, all list sizes, inline composite, far/double-far, capability, reserved, and malformed cases are tested; fuzzing cannot escape segments.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

