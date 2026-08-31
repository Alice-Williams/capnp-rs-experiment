# M40 — Level-1 interoperability, fuzz, and release gate

- Status: in-progress
- Phase: 6
- Depends on: M37, M38, M39

## Outcome

Produce the v1 candidate and complete Level-1 compatibility report.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Rust/C++ matrices pass; 24-hour randomized soak is leak/hang free; fuzz/performance/security gates pass; migration docs exist.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
