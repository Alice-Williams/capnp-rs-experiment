# Native v0.1 versus pinned C++

## Conclusion

The native v0.1 implementation is slower in every measured equivalent
end-to-end scenario. No measured workload is faster than pinned C++ commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.

The largest gap is not primarily a Rust-versus-C++ language effect. The current
generated Rust API is a typed wrapper over reflection: getters and setters look
up fields by string, builders clone field metadata, retained reads repeatedly
construct segment views, and text/data reads allocate owned values. The RPC
wire implementation uses the same reflection path for every Call, Return, and
Finish control message. C++ generated accessors instead compile field offsets
and wire operations into its generated code.

## End-to-end results

These are medians from nine recorded optimized runs on an Intel i7-6700K in
the Debian Trixie Docker Desktop/WSL2 development container. Both data
implementations use `no-reuse`, because the native arena has no reset operation.
Lower is better.

| Scenario | C++ | Native | Native / C++ |
| --- | ---: | ---: | ---: |
| CarSales, object | 41.68 µs | 1,147.67 µs | 27.53× |
| CarSales, standard bytes | 44.09 µs | 1,154.35 µs | 26.18× |
| CarSales, packed bytes | 75.39 µs | 1,243.37 µs | 16.49× |
| CatRank, standard bytes | 566.37 µs | 2,564.56 µs | 4.53× |
| Eval, packed bytes | 6.05 µs | 84.31 µs | 13.94× |
| Sequential Ping RPC | 5.20 µs | 13.81 µs | 2.66× |

The data runner uses identical schema layouts, deterministic C++ benchmark
RNG behavior, operation counts, and semantic response checks. Serialized byte
counts are retained as context but are not required to match because the arena
allocators choose different segment layouts.

The RPC comparison uses the same Ping interface, one bootstrap, and sequential
UInt64 request/reply calls. It favors native Rust: native `MemoryTransport`
passes owned message envelopes directly, while C++ uses a KJ in-memory byte
pipe. Native therefore does not encode or parse transport framing in this
measurement. Its 2.66× result is a lower-bound comparison for the native RPC
state machine, not a claim that the transports perform identical work.

Raw evidence:

- [data comparison](../../benchmarks/results/2026-09-02-native-cpp-g-drive-docker/comparison.tsv)
- [data samples](../../benchmarks/results/2026-09-02-native-cpp-g-drive-docker/results.tsv)
- [RPC comparison](../../benchmarks/results/2026-09-02-native-cpp-rpc-g-drive-docker/comparison.tsv)
- [RPC samples](../../benchmarks/results/2026-09-02-native-cpp-rpc-g-drive-docker/results.tsv)

## Where the time goes

### Data workloads

Native phase timing shows that request construction plus application handling
accounts for nearly all measured phase time in object and standard CarSales,
and remains dominant in the other cases:

| Scenario | Build + handle | Share of instrumented phases |
| --- | ---: | ---: |
| CarSales, object | 763.44 µs | 99.9% |
| CarSales, standard bytes | 721.52 µs | 99.5% |
| CarSales, packed bytes | 751.32 µs | 93.5% |
| CatRank, standard bytes | 1,255.29 µs | 88.4% |
| Eval, packed bytes | 46.26 µs | 89.0% |

Standard framing itself is small in CarSales: request encode/decode totals about
2.40 µs and response encode/decode about 0.78 µs. Packed CarSales spends about
50 µs encoding and decoding the request, which matters but cannot explain a
1.17 ms C++ gap. CatRank's application string and sorting work amortizes the
runtime overhead, explaining why its ratio is the least poor.

The checked-in M06 microbenchmark also measures a shared traversal-budget
charge at about 8.00 ns versus 1.27 ns for a local charge. Atomic accounting is
real overhead, but is far too small on its own to explain gaps of 4.5–27.5×.

Detailed evidence:

- [native data phases](../../benchmarks/results/2026-09-02-native-phase-g-drive-docker/phases.tsv)
- [native data wall totals](../../benchmarks/results/2026-09-02-native-phase-g-drive-docker/totals.tsv)

### RPC

The 100,000-call native diagnostic run records 12.45 µs wall time per call.
Five driver/protocol phases consume 11.38 µs: client request, server dispatch,
server return, client return, and server Finish processing. That is 94.0% of
instrumented phase time and 91.4% of child-process wall time. Request building,
call submission, application handling, and result checking together consume
only about 0.72 µs per call.

- [native RPC phases](../../benchmarks/results/2026-09-02-native-rpc-phase-g-drive-docker/phases.tsv)
- [native RPC wall total](../../benchmarks/results/2026-09-02-native-rpc-phase-g-drive-docker/totals.tsv)

## Traced causes

1. **Generated accessors are reflection wrappers.** The code generator emits
   getters using `inner.get("field")` and setters using `inner.set("field",
   ...)` in [capnp-codegen](../../crates/capnp-codegen/src/lib.rs). Dynamic
   lookup calls `StructSchema::field`, which linearly scans the field vector in
   [model.rs](../../crates/capnp-schema/src/model.rs). A builder lookup also
   clones the selected `Field` in [dynamic.rs](../../crates/capnp-schema/src/dynamic.rs).

2. **Retained scalar reads reconstruct a segment view.** `OwnedMessage`'s
   `borrowed_segments()` collects a fresh `Vec<&[u8]>`; `ObjectRef::with_reader`
   invokes it for each opened reader in
   [owned.rs](../../crates/capnp-message/src/owned.rs). Generated and dynamic
   field operations repeatedly cross this path, including validation and
   traversal accounting.

3. **Text and data access copies.** Dynamic text reads produce a `String` and
   data reads produce a `Vec<u8>` in
   [dynamic.rs](../../crates/capnp-schema/src/dynamic.rs). The CatRank port reads
   snippets for scoring and again for its result, then copies URLs. C++ readers
   expose borrowed wire views.

4. **Messages cannot reuse arena storage.** The fair benchmark disables C++
   scratch reuse because `ExclusiveArena` has no reset/reuse operation. Native
   still creates arenas, owned segment collections, and control messages for
   each iteration.

5. **RPC control traffic uses reflection too.** Call, Return, and Finish are
   constructed with `DynamicStructBuilder` and string field names, then decoded
   through `DynamicStruct::get` in
   [level0.rs](../../crates/capnp-rpc-core/src/level0.rs) and
   [protocol.rs](../../crates/capnp-rpc-core/src/protocol.rs). Even the direct
   in-memory transport pays these wire-message costs.

6. **The executor-neutral actor boundary adds synchronization.** Each handle
   command enters a mutex-protected shared mailbox, and each sequential call is
   advanced through separate driver polls for request, dispatch, return,
   delivery, and Finish in [actor.rs](../../crates/capnp-rpc-core/src/actor.rs)
   and [driver.rs](../../crates/capnp-rpc/src/driver.rs). This is valuable for
   safe cross-thread use but expensive for the single-thread Ping case.

As a control, the older checked-in oracle run compared the same C++ version
with mature upstream `capnproto-rust`, not this native implementation. Upstream
Rust was only 1.06–1.39× slower in the five data cases and 2.11× slower in Ping.
That strongly indicates that most of the current data gap is architectural and
recoverable rather than inherent to Rust.

## Optimization order

1. Generate direct constant-offset readers/builders for typed application
   schemas, with borrowed text/data results. Keep reflection as an explicit
   dynamic API rather than the generated fast path.
2. Generate or hand-specialize direct RPC control-message codecs so Call,
   Return, and Finish do not perform string lookup and cloned metadata work.
3. Remove the per-reader segment-vector allocation by introducing a reusable
   validated read context or a segment representation that can be borrowed
   directly.
4. Add safe arena reset/scratch reuse, then rerun both `no-reuse` and `reuse`
   comparisons separately.
5. Add a same-thread driver fast path or command batching that preserves the
   executor-neutral and cross-thread APIs, and measure each RPC protocol pass
   after the wire fast path lands.

These should be separate optimization milestones. Each needs the same raw
cross-language rerun and semantic checks; isolated native speedups are not a
substitute for the end-to-end C++ comparison.

## Not compared

Application-defined persistence and Level 4 Join/equality have no equivalent
C++ scenario in this suite. The native parallel read/build/batch benchmarks
demonstrate speedups over native serial controls, but there is no matched C++
parallel implementation here, so they are not evidence that native Rust is
faster than C++.
