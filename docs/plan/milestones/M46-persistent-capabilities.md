# M46 — Persistent capabilities and SturdyRefs

- Status: implementation candidate complete on `dev/m46-persistent-capabilities`; activation awaits M40
- Phase: 7
- Depends on: M38, M40

## Outcome

Implement persistent.capnp application capabilities, typed save/restore, owner semantics, reconnect, expiry, and revocation.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Restart restoration works; invalid/expired/unauthorized/revoked/wrong-owner tokens fail closed; connection IDs are never persisted.

Evidence: `docs/rpc/persistent-capabilities.md`,
`compatibility/manifest.toml`, and
`tools/verify-m46-persistent-capabilities.sh`. The restart model restores a
stable object through a fresh connection generation and exercises all required
failure cases without placing live connection state in durable records.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
