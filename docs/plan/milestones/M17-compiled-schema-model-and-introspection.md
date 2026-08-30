# M17 — Compiled schema model and introspection

- Status: planned
- Phase: 3
- Depends on: M10, M16

## Outcome

Represent schema nodes, types, brands, values, annotations, fields, enums, and source information at runtime.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Pinned CodeGeneratorRequest fixtures load; all lookup paths work; malformed metadata fails; every conformance feature is describable.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

