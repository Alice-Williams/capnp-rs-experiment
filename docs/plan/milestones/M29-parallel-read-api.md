# M29 — Parallel read API and subtree planner

- Status: planned
- Phase: 5
- Depends on: M06, M10

## Outcome

Share typed owned messages and partition large lists/subtrees for Rayon or a scoped executor without copying.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Loom/stress preserve budgets; four-core qualifying workloads reach the target; thresholds avoid small-input regressions; map/reduce examples exist.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

