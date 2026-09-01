# M45 — Level-4 Join and distributed equality

- Status: complete
- Phase: 7
- Depends on: M44

## Outcome

Implement network-parameterized Join, secure join keys/results, direct common-root connection, and distributed equality.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Threat model and model tests cover direct/proxy/forward/revoke/malicious paths; peers cannot falsely join unrelated objects.

Evidence: `docs/rpc/level-4-join.md`, `compatibility/manifest.toml`, and
`tools/verify-m45-level4-join.sh`. The pinned revision supplies a normative
schema and network pseudo-interface but no C++ Join implementation or test
corpus; the verifier records that limitation and covers the generic behavior
with an independent authenticated network model.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
