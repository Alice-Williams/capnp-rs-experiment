# Two-party connection actor

M33 implements the Level-0 lifecycle defined by the pinned `rpc.capnp` at
Cap'n Proto commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`:
`Bootstrap`, the empty-transform `PromisedAnswer` target required for bootstrap
pipelining, `Call`, `Return`, and `Finish`.

## Ownership and ordering

One `ConnectionActor` owns both connection-scoped tables. `ConnectionHandle`,
`QuestionFuture`, `QuestionTarget`, and `CompletionToken` are thread-safe
handles, but none exposes table mutation. Every handle operation enters a
bounded FIFO mailbox. The actor removes the command and releases the mailbox
mutex before changing protocol state or producing an effect.

Incoming requests create answer entries synchronously, then leave the actor as
`Dispatch` effects. Application handlers can run as independent `Send` work.
Their completion tokens carry `(id, generation)` keys and only enqueue a
completion command, so out-of-order completion never makes handlers share the
answer table. Wakers are invoked after all mutex guards are released. No actor,
driver, or transport operation awaits while borrowing state.

Question IDs use the lowest free slot. A slot's generation advances only after
the corresponding `Return` has been received and `Finish` has been emitted.
Answer IDs are chosen by the peer and use monotonically generated internal
tokens, so a completion from a released answer cannot affect a later reuse of
the same wire ID. Active table counts, mailbox entries, and schema/message
construction are bounded before growth.

## Lifecycle

At Level 0, a `Call` can target an active bootstrap answer. M34 also permits
settled imported-cap targets and hosted/receiver-hosted payload descriptors.
M35 adds bounded non-empty promised-answer transforms, queued pipeline
delivery, `receiverAnswer`, and two-party Level-1 tail routing. Their exact
rules are documented in [capability-lifetimes.md](capability-lifetimes.md) and
[promise-pipelining.md](promise-pipelining.md).

An early `Finish` removes an answer unless unresolved pipeline calls still
depend on it. In that case the actor retains only the dependency state, routes
those calls when the handler completes, and suppresses the finished source's
wire return. A later completion for a fully removed answer becomes a harmless
stale-generation event. Shutdown closes the mailbox, rejects new
commands, completes every question exactly once with `Disconnected`, releases
all answers, discards queued work with terminal results where applicable, and
asks the driver to close the transport. A peer `abort` preserves its structured
exception as the terminal error. Invalid or duplicate protocol state emits a
pinned `abort` before close.

`ConnectionDriver<T>` connects the actor to any M32 `DuplexTransport`. It
drains ordered outgoing envelopes before reading more input, surfaces handler
dispatches without selecting an executor and propagates transport backpressure.
M43 extends send/receive effects with atomically associated resources and binds
them to the pinned capability-table `attachedFd` indices.

## Explicit non-goals

M36 layers actor-owned `senderPromise`, `Resolve`, immutable forwarding routes,
and loopback embargoes onto this design; see
[promise-resolution-and-e-order.md](promise-resolution-and-e-order.md).
Streaming flow control, cooperative cancellation of already-dispatched
handlers, reconnect, scheduling policy, attached-resource meaning, and
three-party protocol features remain M37–M45.
