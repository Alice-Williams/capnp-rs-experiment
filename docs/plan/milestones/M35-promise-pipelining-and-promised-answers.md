# M35 — Promise pipelining and promised answers

- Status: planned
- Phase: 6
- Depends on: M34

## Outcome

Implement pipeline paths, unresolved-answer calls, queued delivery, capability-bearing payloads, and Level-1 tail routing.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Chains/diamonds avoid round trips; transforms validate pointer-only paths; cancellation/release work; calculator pipeline interoperates.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

