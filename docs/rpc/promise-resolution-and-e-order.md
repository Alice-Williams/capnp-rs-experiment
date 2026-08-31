# Promise resolution and E-order

M36 implements the two-party promise-resolution subset from the exact
`rpc.capnp` pinned at Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The behavioral oracle is the
pinned C++ implementation's `handleResolve()`, `handleDisembargo()`, and
upstream Tribble regression test. This document records the rules enforced by
the native actor rather than treating the schema as sufficient behavioral
documentation.

## Ownership and states

The connection actor remains the sole mutable owner of promise imports,
promise exports, and embargoes. `ConnectionHandle::new_promise()` returns a
stable `PromiseCapability` plus a one-shot `PromiseResolver`; the promise must
be exported in a payload before that resolver is consumed. Handles enqueue
commands and never mutate routing tables directly.

An imported promise moves exactly once from unresolved to one of:

- a remote export ID, for same-connection route shortening;
- a promised-answer route;
- a local hosted capability behind a loopback embargo;
- a local hosted capability after that embargo; or
- a broken exception.

An exported promise queues calls while unresolved, then stores one immutable
`FrozenPromiseRoute`. Calls arriving before resolution are routed before the
actor emits `Resolve`. Every later call to that export uses the frozen route,
even if the imported capability at the far end later resolves again. This is
the upstream Tribble rule: resolving `P` to `R` permanently means “forward to
R”, not “look through whatever R becomes later”.

## References and release races

Every `senderPromise` occurrence contributes one checked reference, including
duplicates in one payload. A promise export whose last reference is released
becomes an uncounted ID tombstone until its resolver fires. This both suppresses
the unwanted late `Resolve` and prevents the old export ID from being reused in
the intervening race. A `Resolve` received for an already-released import is
accepted; if its resolution introduces a sender capability, the actor emits the
matching `Release` instead of retaining it. Duplicate resolutions, descriptor
kind collisions, unknown active targets, and self-resolution are fatal protocol
errors.

## Loopback embargo

When a promise imported from the peer resolves to one of this endpoint's own
exports, immediate local dispatch could overtake calls that were already sent
through the peer. The actor therefore allocates a bounded lowest-free embargo
ID, emits `senderLoopback` against the original promise route, and queues new
local calls under the configured aggregate embargo-call bound. The peer
verifies that the frozen export route points back to the
sender and echoes `receiverLoopback` only after earlier calls have passed
through its FIFO actor. The originating actor then releases queued calls in
order and uses `DispatchLocal`/`LocalCompletionToken` for subsequent
capability-free calls. Calls carrying capabilities retain the wire route so
their reference-accounting semantics are not weakened by the optimization.

## Scope

M36 does not add streaming flow control, cooperative cancellation, reconnect,
third-party handoff, attached resources, or a scheduler policy. Those remain
M37–M44 work. The actor exposes dispatch effects and completion authorities but
does not choose an application executor.
