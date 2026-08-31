# JSON-RPC 2.0 compatibility boundary

M47 provides an executor-neutral JSON-RPC message and correlation layer in
`capnp-compat`. It uses the bounded native `capnp-json` syntax tree and parser;
transport actors decide how and when bytes are read or written.

The compatibility contract follows the pinned `json-rpc.capnp` schema and its
three C++ tests:

- `jsonrpc` is exactly `"2.0"` and exactly one of `params`, `result`, or
  `error` is present.
- Incoming request IDs may be JSON strings or numbers and are reflected without
  changing their spelling. Locally initiated calls use increasing numeric IDs.
- Notifications omit `id` and never consume a pending-call slot.
- Error responses preserve the signed 32-bit code, message, and optional JSON
  data value.
- Multiple calls can be in flight and complete in any order. Unknown, duplicate,
  and replayed response IDs fail closed.
- Pending-call count, method bytes, JSON frame bytes, header bytes, syntax-tree
  values, and nesting are independently bounded. Limit failures do not add a
  pending call.
- `ContentLengthCodec` incrementally implements the VS Code-style
  `Content-Length` header transport, including partial bodies and multiple
  complete frames, while rejecting duplicate, invalid, and oversized lengths
  before exposing a body.

This layer intentionally does not choose an async executor, socket type, or
Cap'n Proto interface dispatcher. Applications map validated `method` and
`params` values to their generated or dynamic server APIs, then return a result
or `JsonRpcFailure` through the same session.
