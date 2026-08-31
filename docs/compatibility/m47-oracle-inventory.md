# M47 pinned adapter and example inventory

M47 uses the Cap'n Proto C++ tree at pinned commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The source paths and test case
names below are the behavioral oracle. Adapter schemas define the wire
contract; C++ tests define lifecycle, streaming, cancellation, and path
shortening behavior. Native Rust tests must be independent ports rather than
assertions over implementation details.

## ByteStream

Sources:

- `c++/src/capnp/compat/byte-stream.capnp`
- `c++/src/capnp/compat/byte-stream.h`
- `c++/src/capnp/compat/byte-stream.c++`
- `c++/src/capnp/compat/byte-stream-test.c++`

Wire invariants are ordered streaming `write(Data)`, explicit clean `end()`,
bounded `getSubstream(callback, limit)`, and non-terminating `startTls(host)`.
Dropping without `end()` is cancellation. While a substream owns the path,
writes to the parent are invalid. A substream ending before its limit reports
the byte count without ending the destination. Reaching the limit calls
`reachedLimit()`, resolves future writes to the returned stream, and sends any
overrun bytes to that continuation first.

The exact pinned corpus contains 12 cases: plain round-trip; one- and two-level
shortening; pipe and RPC shortening; concurrent shortening in two topologies;
two substreams with finite and unlimited limits; promise-stream forwarding;
and two explicit-end round trips/wrappers.

## JSON-RPC 2.0

Sources:

- `c++/src/capnp/compat/json-rpc.capnp`
- `c++/src/capnp/compat/json-rpc.h`
- `c++/src/capnp/compat/json-rpc.c++`
- `c++/src/capnp/compat/json-rpc-test.c++`

`RpcMessage.jsonrpc` is exactly `"2.0"`. Calls correlate arbitrary incoming
string/number IDs while locally initiated calls use numbers. Notifications
omit IDs and return immediately. Exactly one of params, result, or error is
present. Errors retain signed code, message, and optional JSON data. Multiple
in-flight calls may complete independently. The three pinned cases are basics,
error mapping, and multiple calls.

## HTTP, CONNECT, and WebSocket

Sources:

- `c++/src/capnp/compat/http-over-capnp.capnp`
- `c++/src/capnp/compat/http-over-capnp.h`
- `c++/src/capnp/compat/http-over-capnp.c++`
- `c++/src/capnp/compat/http-over-capnp-test.c++`
- `c++/src/capnp/compat/websocket-rpc.h`
- `c++/src/capnp/compat/websocket-rpc.c++`
- `c++/src/capnp/compat/websocket-rpc-test.c++`

The adapter preserves all 28 pinned HTTP methods, common/uncommon header
forms, known/unknown body size, response callbacks, pipelined request bodies,
request cancellation, fixed-zero body suppression, and service lifetime while
a call is outstanding. CONNECT covers accepted bidirectional byte streams,
rejected responses with bodies, and `startTls`. WebSocket covers ordered text,
binary, and close frames, overload propagation, path shortening, and exact
wire-byte accounting.

The pinned HTTP corpus contains method-enum parity, invalid common-value
rejection, three HTTP end-to-end/lifetime cases, three WebSocket cases, and
three CONNECT cases. The separate WebSocket message-stream corpus contains
frame behavior and byte-count cases.

## Address book and calculator

Sources:

- `c++/samples/addressbook.capnp` and `addressbook.c++`
- `c++/samples/calculator.capnp`, `calculator-client.c++`, and
  `calculator-server.c++`

The address-book example exercises nested structs/lists, text, enums, unions,
standard and packed persistence, and deterministic read-back. The calculator
exercises promised `Value` pipelines, server-defined `Function` capabilities,
client callbacks, operator capabilities, and expression recursion.

M47 extends the calculator scenario with existing native runtime facilities so
the combined examples also demonstrate streaming, cancellation, concurrent
dispatch, authenticated handoff, distributed equality, and SturdyRef restart
restoration. These additions are integration examples; they do not alter the
pinned calculator schema.

## Explicit non-goals

M47 does not implement a general web server, URL parser, TLS stack, HTTP/2 or
HTTP/3, browser WebSocket handshake, production JSON parser, database, or
network identity system. Transport integrations own those policies. The
adapter layer owns lossless mapping, bounded buffering, lifecycle, ordering,
and cancellation at the Cap'n Proto boundary. M48, not M47, owns release-gate
claims and the final maximum-parity audit.
