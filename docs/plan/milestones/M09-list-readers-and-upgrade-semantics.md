# M09 — List readers and upgrade semantics

- Status: planned
- Phase: 1
- Depends on: M08

## Outcome

Implement typed primitive, enum, pointer, nested-list, and struct-list readers including legal upgrades.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Every element encoding and inline-composite bound is covered; reference upgrade behavior matches; iteration and indexing charge equally.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

