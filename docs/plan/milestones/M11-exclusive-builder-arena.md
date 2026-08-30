# M11 — Exclusive builder arena

- Status: complete
- Phase: 2
- Depends on: M03, M05

## Outcome

Implement an exclusive typed-offset arena with root initialization, zeroed allocation, growth, and pointer emission.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

All base shapes build and cross-read in C++; overflow is an error; aliasing builders fail to compile; no uninitialized bytes escape.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
