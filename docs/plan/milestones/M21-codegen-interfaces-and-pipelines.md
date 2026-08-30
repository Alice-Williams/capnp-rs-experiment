# M21 — Codegen: interfaces and pipelines

- Status: planned
- Phase: 3
- Depends on: M20

## Outcome

Generate thread-safe client/server traits, request/result types, inheritance, generics, streaming methods, and typed pipelines.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Pipeline transforms and inherited dispatch are exact; generated defaults are thread-safe; an in-memory local transport exercises calls.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

