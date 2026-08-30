# M25 — Native CodeGeneratorRequest and self-hosted generation

- Status: planned
- Phase: 4
- Depends on: M20, M24

## Outcome

Emit standard CodeGeneratorRequest values natively and regenerate project schemas with the Rust backend.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Native/reference requests agree semantically; regeneration is deterministic; clean Cargo builds need no system capnp; bootstrap is tested.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

