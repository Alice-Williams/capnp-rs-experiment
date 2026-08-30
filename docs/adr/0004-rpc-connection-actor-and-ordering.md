# ADR-0004: RPC connection actor and ordering

- Status: accepted
- Milestone: M02
- Date: 2026-08-30

## Context

RPC question/answer IDs, capability imports/exports, promise resolution, and
embargoes form one ordered protocol state machine. Scattered locks risk
deadlocks and lock-across-await bugs, while a wholly local executor prevents
clients and handlers from using ordinary multi-threaded Rust runtimes.

## Decision

One actor owns each connection's protocol tables and state. Public `Client`
handles contain `Arc`-backed immutable/shared command state and submit commands
to a bounded mailbox. A read task preserves incoming wire order; a bounded
writer task preserves outgoing frame order.

The actor performs immediate protocol transitions synchronously in mailbox
order, then dispatches application handlers as independent `Send` futures.
Handlers may finish out of order because question IDs correlate results.
Promise routing, disembargo, ID reuse, and capability lifetime remain actor
transitions. Network I/O and handlers are never awaited while actor state is
mutably borrowed or locked.

Generated server traits are `Send + Sync + 'static` by default. Explicit
`Concurrent`, `Serial`, and `Keyed` scheduling wrappers control application
execution. A `LocalServer` adapter may isolate non-`Send` application state
without making remote client handles non-`Send`.

## Alternatives considered

- Replace `Rc<RefCell<_>>` with `Arc<Mutex<_>>`: rejected because ownership and
  await boundaries remain unclear and contention spreads across the graph.
- One local-thread RPC runtime: rejected as the default because it prevents
  thread-safe clients and natural handler parallelism.
- One task per protocol table: rejected because cross-table transitions and
  E-order become distributed transactions.

## Consequences

Protocol mutation is intentionally serial per connection, while application
work and independent connections scale. Bounded mailboxes make overload an
explicit error rather than unbounded memory growth.

## Enforcement

M02 prototypes prove client and server-future thread traits. M33 adds a
deterministic actor simulator and no-lock-across-await review tests. M35–M36
model pipelining and E-order races; M39 benchmarks scheduling policies.
