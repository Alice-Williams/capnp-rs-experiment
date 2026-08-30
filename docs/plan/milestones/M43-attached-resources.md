# M43 — Attached file descriptors and resources

- Status: planned
- Phase: 7
- Depends on: M40

## Outcome

Provide Unix SCM_RIGHTS and a generic attached-resource ownership model.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Each FD has one owner; duplicates/excess/unsupported transports close safely; quotas and fallback behavior are demonstrated.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

