# Streaming and flow control

M37 follows the behavior pinned at Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The primary references are
`c++/src/capnp/rpc.h` (`RpcFlowController`), `c++/src/capnp/rpc.c++`
(`WindowFlowController` and `AdaptiveFlowController`), and the adaptive-flow
corpus in `c++/src/capnp/rpc-test.c++`. The pinned `stream.capnp` schema defines
generated streaming methods; no new RPC wire-message variant is involved.

## Contract

A streaming call is sent immediately. The future returned by flow control says
when it is advisable to submit the next call; it neither owns nor cancels the
send. This split is required for E-order. Local generated clients therefore
invoke service dispatch before returning `StreamingCall`, while
`FlowController::send_now()` serializes each controller's send closure.
Independent controller instances represent independent capability streams and
do not share backpressure.

Each send has two handles:

- `FlowAck` records the peer's Return time or permanently poisons the stream
  after an acknowledgement failure.
- `FlowReady` completes immediately below the extended window, completes after
  an acknowledgement makes room, and completes on controller shutdown.
  Dropping it only abandons that wait; the recorded send remains in flight.

The fixed controller uses the pinned C++ extended-window rule, including its
one-largest-message allowance. The adaptive controller records send time,
bytes delivered at send, last delivery time, window at send, and whether the
window was full. Acknowledgements update minimum RTT and a delivery-rate BDP
estimate. Startup permits 2x growth and exits after three rounds without a
25-percent increase; steady state permits 1.25x growth. Saturated traffic may
decay to seven-eighths of the prior window, application-limited traffic cannot
shrink it, and configured minimum and maximum windows clamp every update.

## Bounds and ownership

Flow state is owned behind one controller lock, but no waker or send closure is
called while that state lock is held. Checked byte accounting rejects a message,
aggregate in-flight bytes, or blocked-waiter count before invoking the send.
The actor independently bounds active incoming calls by count and by aggregate
wire-message bytes. An answer retains its byte charge until its final removal;
all Finish, tail-call, and disconnect paths release or clear the charge exactly
once.

## Evidence and non-goals

The Rust simulator ports the pinned C++ behavioral cases: the initial extended
window, acknowledgement release, startup growth, convergence near BDP,
application-limited stability, bandwidth-loss decay, and minimum clamping.
Additional tests cover transactional quota rejection, slow-peer memory bounds,
dropped readiness futures, acknowledgement failure, shutdown wakeup, eager
generated dispatch, and independent streams.

M37 does not add cooperative cancellation or reconnect (M38), connection
sharing or vat-network policy (M39-M42), attached resources (M43), or
three-party routing (M44 and later). The flow controller is executor-neutral;
the surrounding RPC driver remains responsible for driving completion and
supplying monotonic send/ack timestamps.
