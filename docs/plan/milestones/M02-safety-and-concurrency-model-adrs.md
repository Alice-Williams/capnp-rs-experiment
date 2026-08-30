# M02 — Safety and concurrency model ADRs

- Status: planned
- Phase: 0
- Depends on: M00

## Outcome

Freeze reader ownership, exact budgets, builder separation, RPC actors/order/cancellation, and unsafe-code policy in ADRs plus compile-only prototypes.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Each ADR names rejected alternatives, invariants, and enforcing tests; OwnedMessage, ObjectRef, Client, and server futures prove Send/Sync without public unsafe impls.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

