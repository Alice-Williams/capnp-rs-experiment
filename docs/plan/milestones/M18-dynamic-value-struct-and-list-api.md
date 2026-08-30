# M18 — Dynamic value, struct, and list API

- Status: planned
- Phase: 3
- Depends on: M11, M17

## Outcome

Provide reflection-driven dynamic readers/builders, typed downcast, and generic stringification.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Dynamic/generated views agree; field/union/list/enum operations work; schemas need no leaked lifetime or global mutable registry.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

