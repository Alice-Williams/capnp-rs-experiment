# M35 — Promise pipelining and promised answers

- Status: complete
- Phase: 6
- Depends on: M34

## Outcome

Implement pipeline paths, unresolved-answer calls, queued delivery, capability-bearing payloads, and Level-1 tail routing.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Chains/diamonds avoid round trips; transforms validate pointer-only paths; cancellation/release work; calculator pipeline interoperates.

## Completion evidence

`QuestionTarget` emits bounded `noop`/`getPointerField` transforms and the
single-owner actor queues unresolved calls. Chain and diamond tests prove the
dependent dispatches precede source returns; malformed endpoints fail only
their dependent calls, and early `Finish` removes canceled queued work while
retaining only live dependency state. Two-party tail tests cover the routing
return race, local capability-bearing results, reserved question IDs, and the
schema-required ordering from `resultsSentElsewhere` through the original and
forwarded `Finish` messages.

`tools/verify-m35-calculator-pipeline.sh` reproduces fixture SHA-256
`c9523a32a16e79283eab9b4483b4c52573346cd814f3a848003c34b59e3d4d18` and
passes pinned C++ -> native -> pinned C++ calculator interoperability. Rust
1.98 and 1.85 workspace/all-target tests, doctests, strict Clippy, shell
syntax, M34 regression interop, and all 28 Bazel tests pass in the Linux
development container.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
