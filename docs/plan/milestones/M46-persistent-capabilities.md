# M46 — Persistent capabilities and SturdyRefs

- Status: planned
- Phase: 7
- Depends on: M38, M40

## Outcome

Implement persistent.capnp application capabilities, typed save/restore, owner semantics, reconnect, expiry, and revocation.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Restart restoration works; invalid/expired/unauthorized/revoked/wrong-owner tokens fail closed; connection IDs are never persisted.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

