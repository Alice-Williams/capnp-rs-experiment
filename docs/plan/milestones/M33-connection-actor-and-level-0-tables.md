# M33 — Two-party connection actor and Level-0 tables

- Status: planned
- Phase: 6
- Depends on: M02, M32

## Outcome

Implement the per-connection actor, bounded mailboxes, generation-safe tables, bootstrap/call/return/finish, errors, and shutdown.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Handles/futures/actor are Send; ordered state and concurrent handlers coexist; deterministic lifecycle/ID reuse tests pass with no lock across await.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

