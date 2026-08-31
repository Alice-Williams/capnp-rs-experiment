# M44 — Level-3 introductions and handoffs

- Status: implementation candidate complete on `dev/m44-level3-handoffs`; activation awaits M40
- Phase: 7
- Depends on: M40

## Outcome

Implement authenticated Provide/Accept/ThirdPartyAnswer, vines, forwarding, return routing, and third-party embargoes.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Three/four-vat, self, forwarding/reflection, redirect, and embargo simulations plus C++ interop pass without widening authority.

Evidence: `docs/rpc/level-3-handoffs.md`, `compatibility/manifest.toml`, and
`tools/verify-m44-level3-handoffs.sh`. The verifier passes all six exact pinned
C++ cases plus native authenticated routing, safe-fallback, return-adoption,
forged-token, quota, and wire simulations.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
