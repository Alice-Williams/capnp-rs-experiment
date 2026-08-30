# M36 — Promise resolution, release, and E-order

- Status: planned
- Phase: 6
- Depends on: M35

## Outcome

Implement Resolve, promise imports/exports, route shortening, embargoes, loopback disembargo, and the Tribble rule.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Model tests cover every ordering/resolution/release race; delivery order holds; later resolution never bypasses the mandated route.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

