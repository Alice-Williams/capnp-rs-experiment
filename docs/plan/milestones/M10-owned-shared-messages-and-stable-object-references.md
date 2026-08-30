# M10 — Owned shared messages and stable object references

- Status: complete
- Phase: 1
- Depends on: M02, M08, M09

## Outcome

Add OwnedMessage, TypedMessage, and ObjectRef around stable validated wire locations.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Types are Send/Sync by representation; concurrent traversal and stored subobject refs work without copying; compile tests reject dangling/invalid refs.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
