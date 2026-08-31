# Level-1 v1 release candidate

The workspace version `1.0.0-rc.1` freezes the first two-party Level-1
compatibility boundary. It is a release candidate, not a production-stability
claim: the crates remain unpublished and the APIs may still change before a
final v1 release.

## Compatibility authority and invariants

Compatibility is measured, in order, against Cap'n Proto C++ commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`, the normative documents and
pinned schemas, and `capnproto-rust` commit
`2228b71e55cee819c30450bb9bfd9c1f6a722429`. The exact schema hashes and every
accepted milestone are recorded in `compatibility/manifest.toml`.

The v1 boundary preserves these invariants:

- complete messages and ancillary resources are owned across async or thread
  boundaries, and every queue/table/allocation has an explicit limit;
- one connection actor exclusively mutates question, answer, import, export,
  promise, embargo, and lifecycle state;
- calls retain wire and E-order while independent application futures may
  complete out of order;
- capability references have exact duplicate-preserving accounting and all
  connection-owned state is released on disconnect;
- promise-pipeline transforms, tail routing, resolution, and disembargo are
  bounded and validated before state is committed;
- streaming calls send eagerly, acknowledgements govern readiness, and fixed
  or adaptive windows bound bytes and waiters;
- cancellation is idempotent, application opt-out is cooperative, and every
  question/redirect/embargo waiter completes exactly once;
- reconnect generations never reuse connection-scoped identity and calls are
  never replayed implicitly;
- application scheduling policy is explicit (`Concurrent`, FIFO `Serial`, or
  `Keyed`), with a dedicated-thread adapter for non-`Send` local state.

## Interoperability matrix

`tools/verify-m40-level1-interop.sh` is the executable matrix. "Transcript"
means an independently emitted complete RPC message sequence, not a shared
Rust encoder/decoder round trip.

| Surface | Rust to Rust | Rust to C++ | C++ to Rust |
|---|---|---|---|
| Standard, canonical, and packed serialization | workspace corpus | M11/M14/M15 C++ decoders and byte equality | pinned C++ fixtures consumed by native readers/codecs |
| Generated data | generated fixture tests | M19 native generated builder decoded by C++ | pinned compiler requests and C++ wire fixtures consumed by generated readers |
| Capability descriptors and lifetime | actor/capability suites | M34 native transcript verified by C++ | M34 pinned C++ transcript verified by native code |
| Promise pipeline and tail routing | actor/driver suites | M35 native return transcript verified by C++ | M35 C++ calculator transcript dispatched by native code |
| Promise resolution and E-order | actor simulator | M36 native resolution transcript verified by C++ | M36 C++ resolution/disembargo transcript verified by native code |
| Flow control | native fixed/adaptive suite | schema-compatible native messages plus pinned behavior | ten exact pinned C++ behavior tests and native behavioral port |
| Cancellation, disconnect, shutdown, reconnect | exhaustive native race suites | schema-compatible native lifecycle messages plus pinned behavior | nine exact pinned C++ lifecycle tests and six pinned reconnect cases |
| Server scheduling | native concurrency suites | application-local policy; no wire difference | application-local policy; no wire difference |

The matrix deliberately distinguishes byte/transcript interoperation from
behavioral ports. There is not yet a public socket convenience API; transports
implement the executor-neutral `DuplexTransport` boundary and are driven by
`ConnectionDriver`.

## Hardening gates

- `tools/run-m40-rpc-fuzz.sh` mutates all implemented RPC union families and
  arbitrary one-to-four-segment messages. The release gate is at least 100,000
  cases and 60 wall-clock seconds with bounded 4 Kiword messages and no panic,
  abort, hang, or limit escape.
- `tools/run-m40-level1-soak.sh` repeatedly connects two actors, pipelines a
  call, completes out of order, cancels with and without application opt-out,
  and disconnects at pending transitions. The release gate is 86,400 seconds
  and at least 100,000 sessions. Every session asserts empty connection-owned
  tables after disconnect; Linux RSS after warm-up may not grow by more than
  64 MiB.
- `tools/verify-m40-soak-result.sh` accepts only a checked-in `PASS` result
  with at least 86,400 elapsed seconds and 100,000 sessions, and binds it to
  the exact soak example and runner hashes in the result metadata.
- `tools/verify-m40-release-gates.sh` composes the interop, fuzz, soak,
  recorded parallel-performance, exact shared-budget, lifecycle, and shell
  gates. A shorter duration is a smoke test and must never be recorded as the
  release result. Set `M40_USE_RECORDED_SOAK=1` only to verify the already
  completed, hash-bound 24-hour result instead of starting another run.
- The preserved 2026-08-31 run is only an interim development soak: it was
  stopped at 12,420 seconds and 735,873,468 sessions with 3,052 KiB RSS. Its
  `INTERIM_STOPPED` record is deliberately rejected by the release verifier;
  it does not satisfy or claim the 24-hour gate.
- The workspace forbids unsafe Rust. Miri remains mandatory for the disjoint
  builder storage boundary. The complete `capnp-wire` suite also runs in
  Miri's strict-alignment abstract machine, including deliberately unaligned
  and host-endian-independent byte paths. Other release surfaces have no
  unsafe block requiring a safety exception.

## Explicit non-goals

This candidate is two-party Level 1. It does not claim the local capability
utility surface, membranes/revocation, attached resources, three-party
handoffs, Level-4 join/equality, persistent capabilities, or the full C++
convenience API. Those remain M41–M48. It also does not promise drop-in source
compatibility with `capnproto-rust`; see the migration guide.
