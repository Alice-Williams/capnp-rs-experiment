# Cap'n Proto in native Rust

This repository is a fresh experiment in implementing Cap'n Proto directly in
Rust. It intentionally starts from a minimal crate instead of importing the
previous prototype's implementation.

## Approach

- Treat the official Cap'n Proto specification and reference tooling as the
  compatibility oracle.
- Add behavior only with conformance fixtures, including cross-implementation
  tests where practical.
- Design readers for untrusted input from the beginning, with checked offsets,
  traversal limits, nesting limits, and allocation limits.
- Keep encoding, schema handling, code generation, and RPC as distinct layers.
- Prefer a small, correct subset over broad APIs that only round-trip against
  themselves.

## Plan and current scope

The original engineering dossier has been decomposed into 49 dependency-ordered
milestones under [`docs/plan`](docs/plan/README.md). The Phase 0 foundation,
oracle corpus, benchmark harnesses, and safety/concurrency ADRs are complete.
Implementation has completed the conformance-first wire, construction,
reflection, code-generation, native compiler, text-tooling, JSON-tooling, and
parallel-processing phases. The RPC implementation now covers the complete
two-party Level-1 boundary: bounded actor-owned tables, capability lifetimes,
promise pipelines and E-order, flow control, cancellation/disconnect/reconnect,
and explicit thread-safe scheduling policies. M40's frozen-source 24-hour
Level-1 soak and release suite passed, activating the M41-M47 maximum-RPC and
compatibility work. M48's frozen-source 48-hour full-platform soak and complete
security, conformance, build, performance, and release suite passed, completing
the maximum-parity engineering plan.

The `v0.1.0` tag is the first experimental release snapshot. Crates remain
unpublished and their APIs may change before 1.0.

The repository is a fifteen-crate workspace matching the intended architecture.
`capnp-wire` implements M03's no_std words, little-endian scalar access, checked
ranges, and raw pointer bitfields. `capnp-io` implements M04's bounded standard
unpacked framing over immutable segment descriptors. `capnp-message` implements
M05's coordinate-only, bounds-checked pointer validation and M06's exact local
and concurrent traversal limits, copied nesting limits, amplification defense,
and iterative hostile-depth traversal. M07 adds default-aware primitive and
enum reads plus charged, zero-copy Text and Data views. M08 adds coordinate-
based struct/group readers, union-tag preservation, short-section evolution,
and limited pointer defaults. M09 adds typed primitive, enum, pointer, nested,
and struct-list readers with the reference implementation's legal list-upgrade
semantics. M10 adds borrowed and `Arc`-owned message contexts, typed roots, and
stable struct/list references with exact shared traversal accounting. The
Phase 2 begins with M11's exclusive, typed-offset, zero-initializing
builder arena and checked base-shape emitters. M12 extends it across bounded,
deterministically sized segments with direct, single-far, and double-far
pointer emission. M13 adds bounded schema-independent graph copy/clear and
typed same-arena struct/list orphan movement with zeroing on abandonment. The
remaining crates are explicit ownership
boundaries rather than claimed protocol features.

## Development

On non-Linux hosts, run commands in the repository's Dev Container:

```console
cargo test --workspace --all-targets
bazel test //...
```

The project intentionally supports both build systems: Cargo is the native
Rust package/MSRV interface, while Bazel is the pinned Linux orchestration and
conformance environment.

Native schema compilation no longer requires the C++ `capnp` executable. See
the [compile and ID replacement guide](docs/tooling/compile-and-id.md) for
multi-file imports, Rust generation, external plugins, crate mappings, raw
requests, and secure schema IDs.

Native [text decode, encode, and evaluation](docs/tooling/text-tools.md) cover
standard, packed, and flat messages plus imported and nested constants.

Native [reflection-driven JSON conversion](docs/tooling/json-codec.md) covers
the C++ value policy, standard/packed/flat input and output, schema annotations,
bounded parsing, strict-mode decoding, and type/field extension handlers.

Native [parallel immutable reads](docs/tooling/parallel-read.md) share owned
messages without copying, preserve exact traversal accounting across workers,
and expose executor-neutral list partitions and deterministic subtree batches.

Opt-in [partitioned parallel construction](docs/tooling/parallel-build.md)
provides disjoint primitive-list slices and sealed worker fragments with
deterministic far-pointer finalization, while keeping ordinary builders
exclusive and capability registration out of the concurrent boundary.

Bounded [ordered batch pipelines](docs/tooling/batch-pipeline.md) run independent
message transform/build/pack work concurrently, keep slow-writer memory within
explicit item/byte reservations, and emit each stream in input order without a
persistent pool for single-message paths.

The [RPC transport envelope](docs/rpc/transport-envelope.md) owns complete
messages and move-only ancillary resources, applies exact portable queue
quotas, and exchanges pinned `abort` / `unimplemented` control messages without
depending on an async executor.

The [Level-0 connection actor](docs/rpc/connection-actor.md) adds bounded
thread-safe command handles, generation-safe question/answer tables,
bootstrap-pipelined calls, concurrent handler dispatch, ordered returns and
finishes, and an executor-neutral duplex transport driver.

[Streaming flow control](docs/rpc/streaming-and-flow-control.md) adds eager
ordered streaming dispatch, fixed and adaptive bandwidth-delay-product windows,
acknowledgement/RTT tracking, bounded blocked senders and in-flight bytes, and
aggregate incoming-call quotas. Its behavioral corpus runs directly against the
pinned C++ fixed/adaptive tests.

[RPC lifecycle handling](docs/rpc/cancellation-disconnect-and-reconnect.md)
adds last-lease and explicit cancellation, application opt-out, legacy Finish
compatibility, transport-complete shutdown with surfaced errors, and
generation-safe capability recreation that distinguishes disconnect from
overload.

[RPC server scheduling](docs/rpc/server-scheduling.md) adds explicit concurrent,
FIFO serial, and per-key execution policies, bounded generic/Tokio-style and CPU
executor adapters, plus a dedicated-thread local server that isolates non-Send
state behind thread-safe clients.

[Attached resources](docs/rpc/attached-resources.md) add one-owner generic
resource handles, the pinned `attachedFd` capability-table semantics, and a
bounded Unix `SCM_RIGHTS` transport with safe excess-descriptor closure and an
RPC-capability fallback when descriptors are unavailable.

The [Level-1 release candidate report](docs/rpc/level-1-release-candidate.md)
defines the executable Rust/C++ matrix and fuzz, soak, performance, and security
gates. Applications coming from the established Rust implementation should use
the [migration guide](docs/migration-from-capnproto-rust.md); source-level API
compatibility is intentionally not claimed.

This is experimental software and is not yet suitable for production use.

## License

Licensed under the [MIT License](LICENSE).
