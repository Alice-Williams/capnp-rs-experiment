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
| Native parser, semantic/layout compiler, request/codegen, CLI | M22–M26 | implemented with recorded source-shape gap | pinned C++ requests, self-hosted generation, multi-file CLI tests | declarations nested in interfaces are not yet classified correctly by the native semantic resolver |
| Text and JSON tools | M27–M28 | implemented | pinned C++ text corpus and byte-exact JSON policy fixtures | production application JSON policy remains caller-owned |
| Parallel read/build/batch scheduling | M29–M31 | implemented | deterministic concurrency tests and checked-in four-core results | rerun performance artifacts at the release commit |
| RPC schema, transport, Level 0 tables, capability lifetime | M32–M34 | implemented | lossless schema/wire tests and pinned capability interop | activation follows M40 |
| Pipelines, promise resolution, E-order | M35–M36 | implemented | pinned calculator/resolve transcripts and actor race tests | activation follows M40 |
| Streaming, cancellation, reconnect, scheduling | M37–M39 | implemented | pinned C++ flow/lifecycle cases, fault tests, checked-in scheduling results | activation follows M40 |
| Level-1 v1 release | M40 | blocked | interop, fuzz, security and performance gates pass; 12,420-second interim soak retained | required 86,400-second same-commit soak has not passed |
| Local capabilities, membranes, attached resources | M41–M43 | candidate | exact pinned C++ corpora plus native compile/lifetime/quota tests | activate after M40; attached OS handles are Unix-only |
| Authenticated Level 3 handoff | M44 | candidate | six pinned C++ cases and authenticated native router simulations | activate after M40 |
| Level 4 distributed equality | M45 | candidate | pinned wire surface and adversarial native network model | no pinned C++ behavioral implementation exists; activate after M40/M44 |
| Persistent capabilities | M46 | candidate | pinned interface plus restart, sealing, expiry, tamper and revocation model | realm database, cryptography, clock, owner authentication and dialer remain application policy |
| ByteStream, JSON-RPC, HTTP/CONNECT, WebSocket | M47 | candidate | exact pinned C++ case inventory plus bounded native lifecycle tests | activate after M40–M46 |
| Address-book, calculator and full-platform examples | M47 | candidate | native/C++ address-book cross-read and calculator pipeline/callback/platform scenarios | nested sample declarations use explicit pinned IDs at file scope until the compiler gap is fixed |
| Maximum-parity release | M48 | in progress | this audit and executable gates | multi-day full-platform fault/soak, release-commit performance rerun, security gate and every predecessor activation |

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

## Hard blockers

1. M40 needs a checked-in `PASS` result from an uninterrupted 86,400-second
   same-commit Level-1 soak. Its preserved approximately 3-hour run is useful
   development evidence, not a release result.
2. M48 needs a multi-day full-platform fault/soak result exercising the
   capability layers and compatibility adapters, bound to exact source hashes.
3. Performance artifacts must be regenerated and accepted on the release
   commit and hardware; historical artifacts cannot excuse a regression.
4. The final security gate must pass Miri, Loom, bounded fuzz, unsafe-code,
   shell, MSRV, Cargo, rustdoc, Clippy, Bazel, and all pinned-oracle checks.
5. The nested-interface declaration compiler gap must either be implemented or
   remain an explicit release limitation with a regression test that prevents
   incorrect silent compilation.

The executable inventory check is `tools/verify-m48-inventory.sh`. It verifies
that the manifest retains the required milestone/evidence states and that this
audit continues to name every blocker; it never converts a candidate into a
release merely because the fast test suite passes.

The full-platform soak is `tools/run-m48-full-platform-soak.sh`. Its release
defaults require at least 100,000 sessions and 172,800 seconds after warmup.
Each session randomizes the order of the address-book, calculator, and platform
scenarios while checking their complete observable results. The platform case
includes normal stream completion, cancellation, authenticated Level 3
handoff, Level 4 equality, and persistence across a simulated restart. CI only
runs an explicitly bounded smoke configuration; smoke output is not release
evidence.
