# M44 — Level-3 introductions and handoffs

- Status: in progress on `dev/m44-level3-handoffs`
- Phase: 7
- Depends on: M40

## Outcome

Implement authenticated Provide/Accept/ThirdPartyAnswer, vines, forwarding, return routing, and third-party embargoes.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Three/four-vat, self, forwarding/reflection, redirect, and embargo simulations plus C++ interop pass without widening authority.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
