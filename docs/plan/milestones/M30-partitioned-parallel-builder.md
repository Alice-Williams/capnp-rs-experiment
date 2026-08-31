# M30 — Partitioned parallel builder

- Status: complete
- Phase: 5
- Depends on: M12, M13

## Outcome

Provide disjoint partitions, worker allocation lanes, sealed fragments, far links, and deterministic finalization.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Types prevent overlapping slots; panic/cancel leaves defaults; reference accepts output; Miri/Loom and four-core scaling gates pass.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
