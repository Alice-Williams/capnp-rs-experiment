# M23 — Names, imports, IDs, constants, and type resolution

- Status: planned
- Phase: 4
- Depends on: M22

## Outcome

Resolve modules, names, aliases, imports, IDs, constants, annotations, cycles, and generic scopes deterministically.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Reference corpus semantics match; standard schemas are overridable by explicit paths; filesystem enumeration cannot change output.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

