# M28 — C++-parity JSON codec

- Status: complete
- Phase: 4
- Depends on: M18, M25

## Outcome

Implement reflection-based JSON conversion with explicit defaults and extension hooks.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Every value kind and ambiguity has policy tests; handlers/renaming/annotations/base64/depth limits and C++ corpus interoperate.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
