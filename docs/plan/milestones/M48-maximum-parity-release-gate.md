# M48 — Maximum-parity release gate

- Status: in progress on `dev/m47-compatibility-adapters`
- Phase: 7
- Depends on: M47

## Outcome

Publish the full-platform release and feature-by-feature C++/spec parity report.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

All inventories are implemented or explicitly Rust-inapplicable; Level 0–4/persistence matrices, multi-day fault/soak, performance, and security gates pass.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
