# M20 — Codegen: generics, brands, constants, and annotations

- Status: planned
- Phase: 3
- Depends on: M19

## Outcome

Complete non-RPC generation for generic scopes, brands, constants, annotations, and cross-crate imports.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Generic/unbound cases and pointer constants compile/run; annotation metadata is typed; large-schema compile growth is benchmarked.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

