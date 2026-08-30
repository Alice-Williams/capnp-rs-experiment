# M34 — Capability import/export and lifetime management

- Status: planned
- Phase: 6
- Depends on: M33

## Outcome

Implement payload descriptors, import/export tables/refcounts, hosted/receiver capabilities, and explicit/implicit release.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

C++ callback/capability interop passes; refcounts and duplicate descriptors are exact; disconnect/quotas release without leaks.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

