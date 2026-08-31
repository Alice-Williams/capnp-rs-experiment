# Two-party Level-0 connection actor

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

At Level 0, a `Call` can target only an active bootstrap answer and its
`PromisedAnswer.transform` must be empty. Payload capability tables must be
empty. Results are owned and contain no imported capabilities, so the caller
can emit `Finish` as soon as it has retained the decoded `Return` payload.
M34 changes this boundary when imports and exports acquire independent
lifetimes.

An early `Finish` removes its answer entry. A later handler completion becomes
a harmless stale-generation event. Shutdown closes the mailbox, rejects new
commands, completes every question exactly once with `Disconnected`, releases
all answers, discards queued work with terminal results where applicable, and
asks the driver to close the transport. A peer `abort` preserves its structured
exception as the terminal error. Invalid or duplicate protocol state emits a
pinned `abort` before close.

`ConnectionDriver<T>` connects the actor to any M32 `DuplexTransport`. It
drains ordered outgoing envelopes before reading more input, surfaces handler
dispatches without selecting an executor, propagates transport backpressure,
and rejects ancillary resources because their capability association belongs
to M43.

## Explicit non-goals

M33 does not implement capability descriptors or import/export refcounts,
general promised-answer transforms, promise resolution, embargo, streaming
flow control, cooperative handler cancellation, reconnect, scheduling policy,
attached-resource meaning, or three-party protocol features. Those remain
M34–M45.
