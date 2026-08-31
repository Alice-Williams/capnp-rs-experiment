# Mature local capability utilities

M41 extends the executor-neutral local RPC boundary without changing the M40
wire protocol. Compatibility follows pinned Cap'n Proto C++
`capability.h`/`capability.c++` and `capability-test.c++`, especially the basic,
inheritance, pipeline lifetime, provisional pipeline, tail-call, dynamic,
capability-list, server-set, `thisCap`, transfer, nested `RemotePromise`, and
capability-preserving clone cases. Pinned schemas remain authoritative for
interface IDs, method ordinals, parameter/result types, and inheritance.

## Invariants

- A local client is an immutable, cloneable `Send + Sync` handle. Cloning a
  client or capability table preserves process-local capability identity.
- A promise client queues no hidden application work. Calls await the one-shot
  resolution and then dispatch exactly once in submission order; rejection and
  disabled/broken clients fail every waiter.
- A response and every pipeline derived from it share one underlying call.
  Dropping or awaiting the response cannot invalidate retained pipelines.
- Provisional pipelines may expose only capabilities explicitly published by
  the callee. Final response capabilities are resolved through the checked
  pointer transform and exact local capability table.
- Tail-call helpers transfer the raw response and capability table rather than
  decode/re-encode or proxy them through an intermediate result.
- Dynamic calls validate the interface inheritance graph, method name/ordinal,
  and parameter/result struct IDs against the compiled schema before dispatch.
- A `CapabilityServerSet` unwraps only clients registered by that exact set.
  Synchronous unwrap never follows an unresolved promise. Asynchronous unwrap
  waits for resolution, so it cannot bypass an actor-owned E-order embargo.
- Capability lists, response copies, and transfers retain the original
  `Arc`-derived identity; they never duplicate server authority.

## Runtime boundary

`capnp-rpc` owns the executor-neutral primitives:

- `LocalClient::promise()`, `broken()`, and `disabled()` define local client
  settlement and stable failure behavior. The resolver is a consuming one-shot
  authority and detects direct or transitive promise cycles.
- `LocalCall`, `LocalResponse`, and `CapabilityList` keep the response message
  and its checked local capability table together. `PendingCall::response()`,
  `send_ignoring_result()`, and `send_for_pipeline()` are ownership choices over
  the same shared dispatch.
- `PipelineBuilder` is the local equivalent of C++ `CallContext::setPipeline()`.
  A pipeline may be installed once, has a configured entry limit, and rejects
  duplicate paths.
- `tail_call()` and `direct_tail_call()` return the original response/capability
  table. `flatten_pending()` reduces a future pending call without
  decode/re-encode.
- `CapabilityServerSet` keeps a private set identity. `try_get_local_server()`
  is non-blocking; `get_local_server()` may wait for promise settlement.
- `DynamicCapability` and `DynamicServer` validate interface inheritance,
  declaring method ordinals, parameter/result struct IDs, and named pipeline
  fields against `CompiledSchema`.

Generated clients implement `FromLocalClient`; generated result pipelines carry
an opaque `PipelineSource` through nested struct, generic, implicit-parameter,
interface, and `AnyCapability` fields. Generated code contains no dispatch or
settlement state machine.

## Compatibility evidence

`tools/verify-m41-local-capabilities.sh` runs pinned upstream
`capability-test.c++` lines 44–1210 (28 cases) and the native behavioral ports.
That range ends at capability-aware `clone()`. The next applicable facility,
`RevocableServer` at line 1420, is intentionally the first M42 case.

## Explicit non-goals

M41 is local capability ergonomics. It does not add revocation or membranes
(M42), attached descriptors/resources (M43), three-party handoff (M44),
distributed equality/join (M45), persistence (M46), or a runtime-specific task
system. Generated source compatibility with every C++ or historical Rust API
name is not claimed; the behavioral boundary is.
