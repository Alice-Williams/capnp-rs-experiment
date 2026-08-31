# RPC server scheduling

M39 keeps the protocol/application boundary established by ADR-0004: the
connection actor alone mutates RPC tables, while owned application futures may
run independently and return through completion tokens. The pinned C++
reference at commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b` provides the
behavioral boundary through `Capability::Server::dispatchCall`, local promise
clients, and `CapabilityServerSet` in `capability.h` / `capability.c++`.
Cap'n Proto does not prescribe a cross-language thread-pool policy, so
Concurrent/Serial/Keyed scheduling is an explicit Rust-side choice and changes
no wire behavior.

## Policies

- `Concurrent<S>` delegates immediately. Independent calls may overlap and
  complete in any order.
- `Serial<S>` acquires a FIFO gate synchronously at dispatch and holds its
  permit until the complete application future resolves. It guarantees no
  overlap and dispatch-order admission.
- `Keyed<S, K, F>` assigns each dispatch a key. Equal keys share the same FIFO
  gate; different keys have independent gates and may overlap. Dead key entries
  are weak references and are removed opportunistically.

Gate mutexes protect only queues and state transitions. Application futures are
never polled while a gate lock is held, and wakers are invoked after unlocking.
Dropping a waiting future marks its waiter canceled; permit release skips it and
wakes the next live waiter. For dispatch-order policy, place Serial or Keyed
outside an executor adapter, as the checked-in benchmark does.

## Executors and bounds

`ExecutorService` submits an owned `Send` service future to a `TaskExecutor` and
returns a thread-safe response future. `GenericExecutor` accepts a fallible
spawn callback. `TokioExecutor` is dependency-free: a Tokio application passes
`|task| { tokio::spawn(task); }`, avoiding a mandatory runtime dependency in the
protocol crate. `ThreadPoolExecutor` supplies a small built-in CPU executor with
a configured positive worker count and bounded FIFO queue; full queues return
`SchedulerError::Overloaded` before retaining the task.

Every executor response has a panic boundary. A panicking service future
completes its caller with `SchedulerError::Panicked`, and built-in workers keep
running. Spawn errors likewise complete the response instead of leaving a
waiter pending. The pool's workers exit after the last sender is dropped.

## Non-Send local state

`LocalServer::spawn` moves a `Send` state factory to a named dedicated thread,
then constructs the state there. The resulting state can contain `Rc`, `Cell`,
or other non-`Send` values because it never crosses the thread boundary. A
bounded channel carries owned messages to a synchronous state callback, while
the exposed `LocalServer` and every returned response future remain `Send +
Sync`. Calls are serial by construction. Queue overload and worker disconnect
are explicit errors; callback panic completes the active response and the
worker continues.

The adapter intentionally accepts a synchronous callback. Non-`Send` futures
borrowing local state, migration between local executors, and a general
single-thread async reactor are not part of M39.

## Evidence

Unit and stress tests prove concurrent overlap, serial exclusion, per-key
exclusion with cross-key overlap, FIFO admission, canceled-waiter skipping,
bounded overload, generic/Tokio-style executor adaptation, panic completion,
worker survival, and isolation of an `Rc<Cell<_>>` state behind a `Send + Sync`
server handle.

The recorded i7-6700K Docker/WSL2 benchmark submits 64 CPU-bound calls through
one service. Seven-sample medians reach 3.853x throughput from one to four
workers (201.616 to 776.802 jobs/s). Four-worker concurrent median p99 is 82.289
ms versus 317.277 ms with one worker. Serial remains near one-worker throughput,
Keyed reaches 763.947 jobs/s, and its four evenly loaded keys record a median
maximum same-key completion run of one. Raw samples and environment metadata
are in `benchmarks/results/2026-08-31-m39-g-drive-docker`.

M39 does not change actor ordering, capability lifetime, cancellation, flow
control, retry, or transport semantics. Two-party Level-1 interoperability and
hardening remain M40; attached resources and three-party routing remain later
milestones.
