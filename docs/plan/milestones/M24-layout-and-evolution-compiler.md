# M24 — Struct layout, unions/groups, and evolution compiler

- Status: complete
- Phase: 4
- Depends on: M23

## Outcome

Compute data/pointer offsets, padding reuse, discriminants, groups, element preferences, and compiled nodes.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Layouts equal the pinned compiler; appended fields never move earlier fields; group/padding fixtures and invalid-declaration diagnostics pass.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
