# M40 — Level-1 interoperability, fuzz, and release gate

- Status: complete
- Phase: 6
- Depends on: M37, M38, M39

## Outcome

Produce the v1 candidate and complete Level-1 compatibility report.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Rust/C++ matrices pass; 24-hour randomized soak is leak/hang free; fuzz/performance/security gates pass; migration docs exist.

## Interim development evidence

The 2026-08-31 Docker/WSL2 run was cleanly stopped after the durable
12,420-second checkpoint with 735,873,468 sessions and 3,052 KiB RSS. It is
preserved under `release/results/2026-08-31-m40-g-drive-docker` as useful
approximately-3-hour development evidence. Its status is `INTERIM_STOPPED`,
not `PASS`; the required 86,400-second soak remains incomplete and M40 stays
in progress.

## Authoritative release evidence

The immutable replacement run started at `2026-08-31T19:57:00Z` and completed
at `2026-09-01T19:57:39Z` from source commit
`c92e060cbaf75d47bd53cac7b5ae63ec47d5ba9a`. The named isolated container
`capnp-m40-soak-c92e060` exited zero after recording `status=PASS`, exactly
86,400 wall-clock seconds, 3,794,488,678 disconnected sessions, and no RSS
growth from its 3,048 KiB baseline. The provenance-bound result is preserved
under `release/results/2026-08-31-m40-c92e060-g-drive-docker`.

`tools/verify-m40-soak-result.sh` accepts the recorded evidence, and the full
recorded-soak `tools/verify-m40-release-gates.sh` suite passes the pinned C++
interop matrix, decoder fuzzing, performance artifacts, unsafe-code scan,
Loom and actor tests, isolated-target Miri tests, and shell validation.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
