# Maximum-parity release audit

This document defines the M48 release claim against Cap'n Proto C++ commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. "Maximum parity" means the
Rust-native product boundary in `docs/charter.md` implements every planned
wire, schema, compiler, tooling, parallel, RPC, capability, persistence, and
compatibility-adapter inventory, or identifies a C++-specific facility as
inapplicable. It does not mean C++ ABI compatibility or that this repository
reimplements KJ and unrelated C++ platform libraries.

No release may be called complete while a row is `candidate` or `blocked`.
Implementation evidence can be complete while activation remains blocked by a
long-running release gate.

## Product parity matrix

| Product surface | Milestones | State | Independent evidence | Remaining release work |
|---|---:|---|---|---|
| Wire words, framing, hostile pointer validation, exact budgets | M03–M10 | implemented | hand-authored vectors, pinned C++ frames, properties, Loom | none known |
| Exclusive/multi-segment builders, graph copy/orphans, canonicalization | M11–M14 | implemented | pinned C++ decode/canonical fixtures, compile-fail alias tests | none known |
| Packed, sync/async I/O, mmap/caller storage, feature matrix | M15–M16 | implemented | byte-exact C++ interop and arbitrary chunk boundaries | none known |
| Reflection, dynamic values, generated data/interface APIs | M17–M21 | implemented | pinned compiler requests and C++/Rust cross-read fixtures | none known |
| Native parser, semantic/layout compiler, request/codegen, CLI | M22–M26 | implemented | pinned C++ requests, self-hosted generation, multi-file CLI tests, compiled nested-interface declaration regression | none known |
| Text and JSON tools | M27–M28 | implemented | pinned C++ text corpus and byte-exact JSON policy fixtures | production application JSON policy remains caller-owned |
| Parallel read/build/batch scheduling | M29–M31 | implemented | deterministic concurrency tests and source-bound M48 four-core results | none known |
| RPC schema, transport, Level 0 tables, capability lifetime | M32–M34 | implemented | lossless schema/wire tests and pinned capability interop | activation follows M40 |
| Pipelines, promise resolution, E-order | M35–M36 | implemented | pinned calculator/resolve transcripts and actor race tests | activation follows M40 |
| Streaming, cancellation, reconnect, scheduling | M37–M39 | implemented | pinned C++ flow/lifecycle cases, fault tests, source-bound M48 scheduling results | activation follows M40 |
| Level-1 v1 release | M40 | in progress | interop, fuzz, security and performance gates pass; authoritative `c92e060` soak is running | required 86,400-second same-commit soak has not passed |
| Local capabilities, membranes, attached resources | M41–M43 | candidate | exact pinned C++ corpora plus native compile/lifetime/quota tests | activate after M40; attached OS handles are Unix-only |
| Authenticated Level 3 handoff | M44 | candidate | six pinned C++ cases and authenticated native router simulations | activate after M40 |
| Level 4 distributed equality | M45 | candidate | pinned wire surface and adversarial native network model | no pinned C++ behavioral implementation exists; activate after M40/M44 |
| Persistent capabilities | M46 | candidate | pinned interface plus restart, sealing, expiry, tamper and revocation model | realm database, cryptography, clock, owner authentication and dialer remain application policy |
| ByteStream, JSON-RPC, HTTP/CONNECT, WebSocket | M47 | candidate | exact pinned C++ case inventory plus bounded native lifecycle tests | activate after M40–M46 |
| Address-book, calculator and full-platform examples | M47 | candidate | canonical nested sample schemas, native/C++ address-book cross-read and calculator pipeline/callback/platform scenarios | activate after M40–M46 |
| Maximum-parity release | M48 | in progress | this audit, executable gates, fresh release-commit performance evidence, and the complete `c92e060` security/CI gates | multi-day full-platform fault/soak and every predecessor activation |

## C++ facilities that are Rust-inapplicable

These exclusions do not remove Cap'n Proto protocol behavior from the scope.
They identify implementation mechanisms or neighboring products that a
Rust-native implementation should not reproduce.

- C++ ABI, header layout, template names, exception types, and binary linkage.
- KJ's event loop, fibers, promises, filesystem, process, and thread APIs as
  APIs. Executor-neutral Rust futures and explicit adapters own integration.
- C++-language source generation. The native compiler emits Rust and supports
  the standard plugin request protocol for other language generators.
- `capnp::EzRpc*` convenience ownership and KJ-specific vat-network classes.
  The Rust boundary is an explicit transport plus connection driver.
- A bundled production HTTP server, URL parser, TLS implementation, browser
  WebSocket handshake, database, cryptographic identity system, or wall clock.
- Unix FD transfer on platforms without `SCM_RIGHTS`. A future Windows handle
  transport would be a distinct authenticated transport policy, not wire
  compatibility with Unix descriptors.

## Outstanding hard blockers

1. M40 needs a checked-in `PASS` result from an uninterrupted 86,400-second
   same-commit Level-1 soak. Its preserved approximately 3-hour run is useful
   development evidence, not a release result.
2. M48 needs a multi-day full-platform fault/soak result exercising the
   capability layers and compatibility adapters, bound to exact source hashes.

The final security gate is resolved for source commit
`c92e060cbaf75d47bd53cac7b5ae63ec47d5ba9a`. An exact local invocation of
`tools/verify-m48-security-gates.sh` passed Miri, Loom, 60-second bounded fuzz,
unsafe-code and shell checks, Rust 1.85 MSRV, workspace Cargo tests and docs,
rustdoc, strict Clippy, all 30 Bazel tests, every pinned oracle, and the fresh
performance results. GitHub CI run 110 independently passed its
conformance/Bazel, Miri, and full pinned-oracle jobs.

Commit `b26a494d41d9528f07d9db96996e7501bc2389b3` changed the inventory gate to
accept only its three legal dependency phases. The complete security-gate
composition was therefore rerun at that exact commit and passed again,
including the phase-aware inventory, every pinned oracle, Cargo/MSRV/Bazel,
the bounded fuzz target, Loom, Miri, and the release performance checks. The
changes after `c92e060` remain outside both soak wrappers' recorded source-input
sets, so this verifier hardening and its evidence do not alter either live run.

The former nested-interface declaration compiler blocker is resolved. Named
declarations now take precedence over contextual methods, a compiled-schema
regression covers the distinction, and the native examples use the canonical
pinned C++ nesting rather than flattened substitutes.

The executable inventory check is `tools/verify-m48-inventory.sh`. It verifies
that the manifest retains the required milestone/evidence states and that this
audit continues to name every blocker. It accepts exactly three dependency-
consistent phases: M41–M47 candidates while M40 is pending, atomic M40–M47
activation while M48 remains absent, or M00–M48 complete with M48 evidence.
Partial activation is rejected, and the verifier never converts a candidate
into a release merely because the fast test suite passes.

The authoritative M40 run must use `tools/run-m40-release-soak.sh` with a new
`M40_SOAK_RESULT_DIR`. The wrapper refuses dirty worktrees, reused result
directories, and settings below 100,000 sessions or 86,400 seconds. It records
the exact source commit, source-tree digest, settings, toolchain, and durable
checkpoint output before invoking `tools/verify-m40-soak-result.sh`. That
verifier accepts an explicit result directory and rejects incomplete,
below-threshold, internally inconsistent, excessive-memory, or source-diverged
evidence. The older 12,420-second interim directory remains the default only
to make accidental release-gate use fail closed; it is never upgraded in
place.

The full-platform soak is `tools/run-m48-full-platform-soak.sh`. Its release
defaults require at least 100,000 sessions and 172,800 seconds after warmup.
Each session randomizes the order of the address-book, calculator, and platform
scenarios while checking their complete observable results. The platform case
includes normal stream completion, cancellation, authenticated Level 3
handoff, Level 4 equality, and persistence across a simulated restart. CI only
runs an explicitly bounded smoke configuration; smoke output is not release
evidence.

A release run must use `tools/run-m48-release-soak.sh` with a new
`M48_SOAK_RESULT_DIR`. That wrapper refuses dirty worktrees and existing result
directories, records the exact Git index inputs and toolchain, runs the
48-hour gate, then invokes `tools/verify-m48-soak-result.sh`. The verifier
rejects below-threshold results, inconsistent settings, excessive
resident-memory growth, missing provenance, and any later source-input change.

Release-commit performance evidence is checked by
`tools/verify-m48-performance-results.sh`. The four underlying verifiers accept
an explicit result directory, bind each artifact to the current benchmark and
implementation source hashes, and require the producing commit to remain an
ancestor. M48 uses fresh M29 parallel-read, M30 partitioned-build, M31 ordered
batch, and M39 scheduling runs from one named result root.

The performance blocker is resolved by
`benchmarks/results/2026-08-31-m48-g-drive-docker`. The aggregate verifier
passes all four qualification gates. Rejected noisy/pre-fix attempts are kept
beside the accepted results and explained in that result root's README rather
than being discarded or mistaken for release evidence.

The executable fast release boundary is
`tools/verify-m48-security-gates.sh`: every pinned oracle, unsafe-code and shell
check, Cargo tests/docs/lints, MSRV, Bazel, bounded fuzz, Loom, Miri, and the
fresh performance artifacts. `tools/verify-m48-release-gates.sh` adds the
recorded M40 and M48 soak results and requires every M00–M48 manifest state to
be `complete`; it is intentionally impossible to pass while an activation or
long-run result is missing. GitHub CI runs the complete pinned-oracle suite as
its own job in addition to the existing Cargo/Bazel and Miri jobs.
