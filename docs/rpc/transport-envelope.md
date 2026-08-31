# RPC schema binding and transport envelope

M32 binds the `Message` and `Exception` structures in the exact
`rpc.capnp` / `rpc-twoparty.capnp` pair from Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The source hashes remain in
`compatibility/manifest.toml`; the deterministic native compiler request and
its own hash are in `crates/capnp-rpc-core/schema`.

## Boundary

`capnp-rpc-core` currently understands only the connection-control members
needed before the actor exists:

- `abort`, including revision-stable reason, type, and trace fields;
- `unimplemented`, preserving the complete echoed message graph; and
- every other or future `Message` union tag as an unsupported raw
  discriminant.

Unknown exception enum ordinals are retained. Missing pointer-valued control
payloads take schema defaults, and later optional exception fields are ignored.
This lets revisions interoperate without treating unknown data as malformed.

Question/answer/import/export tables, bootstrap and call dispatch, capability
descriptors, cancellation, reconnect, and E-order are not part of M32.

## Transport contract

`DuplexTransport` uses `Poll` directly and has no executor dependency. A
pending send retains its owned envelope; a successful send consumes it.
Messages and move-only ancillary resources are delivered atomically and in
order. Only one task may poll each direction of an endpoint at a time.

`TransportEnvelope` validates exact message bytes, resource count, and
caller-declared resource byte charges before admission. A concrete transport
owns the meaning of an attached descriptor or handle. The portable in-memory
transport additionally bounds queued envelope count, message bytes, resource
count, and resource charges. Full queues return `Pending` and retain ownership;
receive wakes capacity waiters. Wakers are invoked only after releasing the
queue mutex.

The in-memory transport is a deterministic conformance tool, not a network
transport. M33 layers the actor and Level-0 protocol tables over this boundary.
