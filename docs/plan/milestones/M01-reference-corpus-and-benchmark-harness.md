# M01 — Reference corpus and benchmark harness

- Status: planned
- Phase: 0
- Depends on: M00

## Outcome

Check in independently generated C++ and current-Rust schemas/fixtures, provenance metadata, and a benchmark harness that reports hardware context.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Every pointer, list, and schema category has an oracle fixture; provenance hashes verify; C++ baselines are primary and current Rust is secondary.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

