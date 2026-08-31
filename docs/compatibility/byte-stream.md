# ByteStream compatibility boundary

M47 models the lifecycle and path-shortening behavior of the pinned C++
`capnp/compat/byte-stream` implementation without choosing an async executor or
transport. `capnp-compat::ByteStream` is a synchronous state machine which an
owning transport actor can drive from its own event loop.

The compatibility contract is:

- `end()` is explicit and successful only once. Dropping an open stream or
  calling `cancel()` invokes the sink's cancellation hook instead.
- Empty writes are no-ops. Successful TLS upgrade keeps the same stream open;
  it does not imply end-of-stream.
- A sink failure is terminal. Later operations return the recorded failed
  state rather than attempting more I/O.
- A bounded substream owns the original destination temporarily. Ending before
  its limit reports the exact byte count and permits the caller to reclaim the
  still-open destination.
- Reaching the limit invokes the callback exactly once. Bytes beyond the limit,
  including an overrun in the write which reaches it, go to the returned
  continuation stream. Ending then ends that continuation, while the original
  destination remains reclaimable.
- Limit zero shortens the path immediately. Abandoning an unfinished substream
  cancels its live paths and reports cancellation when no terminal callback has
  already completed.
- Byte counts use checked `u64` arithmetic before slicing or forwarding.

The adapter deliberately does not implement sockets, TLS, executor scheduling,
or Cap'n Proto RPC dispatch. Those are supplied by the transport which
implements `ByteSink`. The schema and native tests are compared with the exact
pinned C++ lifecycle cases listed in `m47-oracle-inventory.md`.
