# M06 — Exact traversal and nesting limits

- Status: planned
- Phase: 1
- Depends on: M02, M05

## Outcome

Provide local and shared exact traversal budgets plus deterministic nesting limits and iterative hostile-depth walkers.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Amplification, cycles, and overlaps are charged; Loom proves concurrent hard limits; malicious depth terminates without recursion hazards.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

