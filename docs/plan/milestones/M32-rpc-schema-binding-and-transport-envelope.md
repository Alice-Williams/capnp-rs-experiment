# M32 — RPC schema binding and transport envelope

- Status: planned
- Phase: 6
- Depends on: M16, M21

## Outcome

Bind pinned RPC schemas and define owned envelopes, ancillary resources, and executor-neutral duplex transport.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Pinned hashes are explicit; optional fields tolerate revisions; in-memory peers exchange unimplemented/abort; resource quotas work portably.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

