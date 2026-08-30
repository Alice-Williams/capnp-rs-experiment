# M08 — Struct readers and evolution semantics

- Status: complete
- Phase: 1
- Depends on: M07

## Outcome

Implement struct data/pointer access, absent defaults, groups, and union wire semantics.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Old/new schemas cross-read both ways; short sections default safely; unknown unions survive; pointer defaults are followed under limits.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
