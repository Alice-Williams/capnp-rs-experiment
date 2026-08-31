# Promise pipelining and Level-1 tail routing

M35 implements the promise-pipelining subset of the two-party protocol in the
pinned `rpc.capnp` schema and C++ implementation at Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The schema is the wire authority;
the pinned C++ trace is the independent interoperability oracle.

## Pipeline paths and ownership

`QuestionFuture::target()` creates a thread-safe reference to an unresolved
answer. `pointer_field()` appends a `getPointerField` transform and `noop()`
preserves the explicit wire operation. A target used before its source returns
is encoded as `MessageTarget.promisedAnswer`. The connection actor is the only
owner of the question, answer, import, export, and pending-call tables.

On receive, a promised-answer call is queued under its source answer without
blocking the actor or a handler. When the source completes, the actor validates
the root pointer, follows only struct pointer fields, requires the terminal
pointer to be a capability, and dispatches all now-ready calls before emitting
the source `Return`. This ordering makes chains and diamonds progress without
waiting for a network round trip. Nulls, data/list traversal, missing capability
table entries, and non-capability endpoints fail only the dependent call with a
protocol exception. Transform length is bounded by `ProtocolLimits` on both
encode and decode.

Capability-bearing results retain their owned message and exact capability
vector as a pipeline snapshot. `receiverAnswer` descriptors can refer to an
active question and carry the same bounded transform representation. Settled
hosted targets dispatch locally with their original identity. Promise
resolution, `senderPromise`, and ordering embargoes are intentionally deferred
to M36.

## Finish and tail routing

An early `Finish` removes queued calls that name the finished answer. If a
finished source still has dependent pipeline work, only the minimal source
state is retained until completion; its queued calls are dispatched, its wire
`Return` is suppressed, and `releaseResultCaps` determines whether newly
described result exports are released. A finished dependent is never
dispatched after its source resolves.

`CompletionToken::tail_call_imported()` implements Level-1 two-party tail
routing. The actor emits a call with `sendResultsTo.yourself` before returning
`takeFromOtherQuestion` for the original answer. The routing question ID stays
reserved until the redirected handler produces `resultsSentElsewhere`, so the
race where the routing return arrives first cannot reuse that ID. The original
future then receives an in-process `LocalResults`, exception, or cancellation;
local capability objects remain local rather than being spuriously exported.

## Evidence and limits

Actor tests cover pipeline chains and diamonds, invalid transforms, queued-call
cancellation, early Finish retention, capability-bearing local tail results,
and both orders of the tail-routing race. `tools/verify-m35-calculator-pipeline.sh`
checks a pinned-C++ calculator trace containing `getOperator()` and pipelined
`evaluate()` calls before either source return, then has pinned C++ validate the
native returns.

M35 does not implement `Resolve`, `senderPromise`, disembargo/E-order,
streaming flow control, cooperative cancellation of already-dispatched
application work, reconnect, attached resources, or three-party RPC. Those
belong to M36 and later milestones.
