# M36 — Promise resolution, release, and E-order

- Status: complete
- Phase: 6
- Depends on: M35

## Outcome

Implement Resolve, promise imports/exports, route shortening, embargoes, loopback disembargo, and the Tribble rule.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Model tests cover every ordering/resolution/release race; delivery order holds; later resolution never bypasses the mandated route.

## Completion evidence

The connection actor now owns unresolved, remotely shortened, promised-answer,
loopback-embargoed, local, and broken import states plus one immutable route per
exported promise. Tests cover calls before and after resolution, broken and
duplicate resolution, release before resolution, late resolution of a released
import, chained promise resolution, the Tribble frozen-route regression, and
sender/receiver loopback ordering with local dispatch held behind the embargo.
Exact reference tests also prove duplicate `senderPromise` accounting and
released export-ID reservation.

`tools/verify-m36-promise-resolution.sh` reproduces pinned C++ fixture SHA-256
`a159859b0b6f51ce296b58e186619636ba45d749f063e9973d92fce54fcce7a7`, has
native Rust validate all M36 wire variants, then has pinned C++ validate
independently emitted native messages. Rust 1.98 and 1.85 workspace/all-target
tests, doctests, strict Clippy, shell syntax, M34–M36 pinned C++ interop, and
the complete Bazel suite pass in the Linux development container.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
