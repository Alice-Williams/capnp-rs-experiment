# M19 — Codegen: structs, enums, unions, and lists

- Status: planned
- Phase: 3
- Depends on: M17, M18

## Outcome

Generate core typed Rust data APIs from a pinned reference CodeGeneratorRequest.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Generated crates avoid the C++ runtime and cover accessors/defaults/unions/groups/unknown enums/lists/imports/docs with cross-language round trips.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

