# M26 — compile and id CLI

- Status: planned
- Phase: 4
- Depends on: M25

## Outcome

Provide plugin-compatible compile workflows, path handling, raw request output, mappings, and secure ID generation.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Multi-file/import workflows work; replacement instructions are documented; IDs set the high bit and surface entropy failures.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

