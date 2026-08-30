# ADR-0005: RPC cancellation, disconnect, and shutdown

- Status: accepted
- Milestone: M02
- Date: 2026-08-30

## Context

Dropping a caller future, sending `Finish`, completing a handler, losing the
transport, and resolving a promise can race. Implicit cancellation policies can
either leak work/capabilities or cancel an operation after its effects have
become observable.

## Decision

- Cancellation is an explicit actor command and idempotent state transition.
- Dropping a result future requests cancellation only where the API contract
  says so; dropping an outbound send handle never retracts already queued bytes.
- Applications may opt out of cooperative handler cancellation for operations
  whose effects must finish.
- Every question/answer has one terminal completion path. Waiters are awakened
  exactly once and table resources are released exactly once.
- Disconnect transitions the connection atomically to terminal state, rejects
  new commands, resolves all waiters with `Disconnected`, releases imports and
  exports, and lets the writer report its final transport result.
- Clean shutdown waits for the selected output-drain policy and returns the
  transport completion/error; it is not merely task cancellation.
- Reconnect helpers create new connection-scoped capabilities. They never reuse
  question, import, export, or promise identity from a dead connection.
- Automatic retry is limited to operations explicitly declared replay-safe and
  distinguishes overload from disconnect.

## Alternatives considered

- Future drop always cancels: rejected because send/application effects may
  already be committed and some operations require completion.
- Future drop never cancels: rejected because abandoned expensive work and
  capability references would accumulate.
- Rebind old capability IDs after reconnect: rejected because IDs are meaningful
  only within one authenticated connection state machine.

## Consequences

Lifecycle APIs carry more explicit policy, but races become deterministic and
testable. Shutdown callers receive meaningful transport failures.

## Enforcement

M33 and M38 simulate dispatch/finish/return/disconnect permutations. Loom/model
tests assert exactly-once completion/release. Soak tests require zero hung
waiters and zero retained connection entries after shutdown.
