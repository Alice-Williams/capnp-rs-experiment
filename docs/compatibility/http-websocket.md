# HTTP, CONNECT, and WebSocket compatibility boundary

M47 models the pinned `http-over-capnp.capnp` surface as executor-neutral
metadata and lifecycle state machines. Socket I/O, HTTP parsing, TLS, and RPC
dispatch stay in the owning transport actor; the adapter owns lossless mapping,
ordering, bounded body accounting, cancellation, and callback state.

The HTTP contract is:

- all 28 pinned method ordinals map exactly to their HTTP spellings;
- all 47 common header names and the one common header value reject invalid or
  out-of-range wire values, while uncommon names and values remain lossless;
- header count/name/value, URL, host, declared body, and accumulated body bytes
  have independent bounds checked before state mutation;
- fixed-zero request and response bodies have no stream, fixed bodies must end
  at exactly the declared length, and unknown bodies remain bounded globally;
- HEAD and status 204, 205, and 304 suppress response body streams while making
  response metadata available immediately;
- request bodies can be driven after response processing begins, preserving the
  promise-pipelined request shape;
- response and WebSocket callbacks are one-shot, cancellation reaches live body
  and WebSocket state, and an outstanding exchange retains its service owner;
- CONNECT acceptance requires a 2xx response, rejection preserves its bounded
  response body, and `startTls` records the expected server hostname without
  ending the tunnel.

The WebSocket contract preserves ordered text, binary, and close frames. Abort
disconnects immediately. Backend overload sends close code 1013 when idle, but
first delivers a frame or close already queued before surfacing disconnection.
Frame count, queued bytes, frame bytes, sent payload bytes, and delivered
payload bytes are checked independently. A binary frame can carry one complete
standard-framed Cap'n Proto message for the `WebSocketMessageStream` mapping.

Path shortening itself is delegated to the M47 `ByteStream` adapter, so the
same HTTP and CONNECT lifecycle is used whether transport actors take the
ordinary or shortened path.
