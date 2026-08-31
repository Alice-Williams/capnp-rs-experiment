# M41 — Mature local capability utilities

- Status: implementation candidate; activation awaits M40 completion
- Phase: 7
- Depends on: M40

## Outcome

Implement promise clients/pipelines, broken/disabled clients, tail calls, provisional pipelines, dynamic capabilities, server sets, and local unwrap.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Pinned capability behaviors pass for inheritance/generics/pipelines/tail races/clone; unwrap cannot bypass embargo.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
