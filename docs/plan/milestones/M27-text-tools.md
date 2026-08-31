# M27 — Text decode, encode, and eval

- Status: complete
- Phase: 4
- Depends on: M18, M25

## Outcome

Implement human-readable value parsing/printing, binary/packed modes, and imported constant evaluation.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Reference text corpus round-trips; diagnostics have locations; schema code order is preserved; imported/nested constants evaluate.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
