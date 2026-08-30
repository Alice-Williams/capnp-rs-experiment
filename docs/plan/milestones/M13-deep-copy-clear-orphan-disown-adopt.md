# M13 — Deep copy, clear, orphan/disown/adopt

- Status: complete
- Phase: 2
- Depends on: M10, M12

## Outcome

Implement schema-independent copy/clear and safe zero-copy orphan movement within compatible arenas.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Cycles/overlap honor budgets; clear and abandoned storage zero safely; adoption validates type/arena; reference orphan cases pass.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
