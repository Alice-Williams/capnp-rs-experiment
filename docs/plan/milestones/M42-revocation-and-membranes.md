# M42 — Revocation and membranes

- Status: implementation candidate; activation awaits M40 and M41
- Phase: 7
- Depends on: M41

## Outcome

Implement revocable servers and recursive bidirectional membranes across clients, servers, pipelines, promises, and object copies.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Every crossing wraps once; side identity and resolution are stable; revocation cancels/rejects correctly; deterministic C++ scenarios match.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
