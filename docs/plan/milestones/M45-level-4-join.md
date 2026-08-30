# M45 — Level-4 Join and distributed equality

- Status: planned
- Phase: 7
- Depends on: M44

## Outcome

Implement network-parameterized Join, secure join keys/results, direct common-root connection, and distributed equality.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Threat model and model tests cover direct/proxy/forward/revoke/malicious paths; peers cannot falsely join unrelated objects.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

