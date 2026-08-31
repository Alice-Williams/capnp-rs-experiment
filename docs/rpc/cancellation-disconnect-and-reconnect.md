# Cancellation, disconnect, and reconnect

M38 follows Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The wire contract comes from
`c++/src/capnp/rpc.capnp` (`Return.noFinishNeeded`, `Finish`, and
`requireEarlyCancellationWorkaround`) and the application opt-out contract from
the `allowCancellation` annotation in `c++/src/capnp/c++.capnp`. The operational
references are `rpc.c++`, the cancellation and clean-shutdown cases in
`rpc-test.c++`, and `reconnect.h` / `reconnect-test.c++`.

## Outgoing question lifetime

`QuestionFuture` and every `QuestionTarget` derived from it share one question
lease. Dropping the response future does not cancel work while a pipeline target
still exists. Dropping the last lease, or explicitly calling
`QuestionFuture::cancel`, queues one idempotent actor cancellation. A question
already sent to the peer emits `Finish(releaseResultCaps = true)`; a local
shortened question is removed without wire traffic.

Cancellation completes the local result with `ConnectionError::Canceled`, but
keeps a sent question ID reserved until its late `Return` arrives or the
connection disconnects. The late result is ignored, its parameter releases are
still applied, and the ID can only then be reused with a new local generation.
`Return.noFinishNeeded` suppresses the usual `Finish`; it is rejected if a
result capability table would still require answer lifetime tracking.

## Incoming cancellation and application opt-out

Every remote dispatch carries a clonable `CancellationSignal`. A modern
`Finish` atomically changes an allowed signal to canceled and removes the
answer. A completion that loses this race is a counted stale completion and
cannot emit a `Return`.

Applications whose effects must run to completion call
`CompletionToken::disallow_cancellation` (or the matching driver method) before
cancellation wins. An early `Finish` then records that the caller has gone away,
retains the answer until the handler completes, and suppresses its `Return`.
Legacy peers may set `requireEarlyCancellationWorkaround`; the actor yields one
scheduling turn before applying that `Finish`, matching the compatibility
window in the pinned implementation. Disconnect is stronger than cooperative
cancellation and force-cancels every remaining signal, including opted-out
work.

## Shutdown and reconnect

`ConnectionDriver::shutdown()` returns a future that owns the shutdown drive:
it transitions the actor, completes all question and embargo waiters, cancels
all dispatch signals, and remains pending until `DuplexTransport::poll_close`
finishes. A transport close failure is returned as `DriverError::Transport`.
Normal mailbox pressure cannot prevent cancellation or shutdown commands from
being queued; their number is bounded by live lifecycle objects.

`CapabilityReconnector` lazily creates a capability and assigns each creation a
checked, monotonically increasing generation. An error observed through the
current generation invalidates it only when classified as disconnected. A
stale lease cannot invalidate a newer capability. Overload is classified as
backoff and does not reconnect; protocol, application, cancellation, and other
errors stop. Existing in-flight leases keep their old capability and still
fail normally—the helper never replays a call or reuses connection-scoped
question/import/export identities.

The synchronous factory runs under the reconnector's mutex so concurrent first
use creates exactly one capability. It must therefore be short and must not
re-enter the same reconnector. Scheduling, delay/backoff, replay-safety policy,
and retry count remain the caller's responsibility.

## Evidence and non-goals

Actor tests simulate drop/target lifetime, explicit cancellation, Finish before
completion, completion after Finish, application opt-out, legacy deferral,
late Return, `noFinishNeeded`, queued embargo disconnect, and force-cancel on
shutdown. Driver tests prove pending transport close is awaited and its error
surfaces. Reconnect tests cover concurrent lazy creation, disconnect-only
invalidation, overload classification, stale generations, reset, and monotonic
non-reuse. `tools/verify-m38-lifecycle.sh` runs the exact pinned C++ lifecycle
cases and the native behavioral suites.

M38 does not add server thread-pool policy (M39), automatic operation replay,
network reconnect/dialing, attached resources (M43), or three-party routing
(M44 and later).
