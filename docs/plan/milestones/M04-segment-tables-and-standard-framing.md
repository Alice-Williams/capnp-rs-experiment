# M04 — Segment tables and standard framing

- Status: in-progress
- Phase: 1
- Depends on: M03

## Outcome

Parse and write standard unpacked message framing into immutable segment descriptors under configured limits.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Clean EOF differs from truncation; limits/overflow/padding/short bodies fail safely; reference one-, two-, and many-segment frames cross-read.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
