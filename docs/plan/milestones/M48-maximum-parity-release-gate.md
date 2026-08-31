# M48 — Maximum-parity release gate

- Status: in progress on `dev/m47-compatibility-adapters`
- Phase: 7
- Depends on: M47

## Outcome

Publish the full-platform release and feature-by-feature C++/spec parity report.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

All inventories are implemented or explicitly Rust-inapplicable; Level 0–4/persistence matrices, multi-day fault/soak, performance, and security gates pass.

## Authoritative release run

Commit `c92e060cbaf75d47bd53cac7b5ae63ec47d5ba9a` passed the complete local
`tools/verify-m48-security-gates.sh` composition and GitHub CI run 110. After
the inventory verifier was hardened, the complete composition passed again at
exact commit `b26a494d41d9528f07d9db96996e7501bc2389b3`; that change is outside the
soak's recorded source-input set. The immutable 48-hour run started at
`2026-08-31T19:57:25Z`; its durable checkpoints are written to
`release/results/2026-08-31-m48-c92e060-g-drive-docker` by the named isolated
container `capnp-m48-soak-c92e060`. The result remains live and uncommitted
until it records `status=PASS`.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
