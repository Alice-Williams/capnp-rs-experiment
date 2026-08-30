# M22 — Lexer and lossless syntax tree

- Status: complete
- Phase: 4
- Depends on: M00, M02

## Outcome

Implement a native bounded lexer/parser retaining source ranges and documentation comments.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

All language constructs parse; diagnostics recover with ranges; reference accept/reject corpus matches; fuzzing cannot panic or go superlinear.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
