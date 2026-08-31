# Level-4 Join and distributed equality

M45 implements the pinned Level-4 `Join` wire message and an executor-neutral,
network-parameterized coordinator for discovering whether two or more
capability paths reach the same hosted object. The authoritative compatibility
source is `rpc.capnp` at pinned Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`, especially `Join`,
`JoinKeyPart`, `JoinResult`, and the `VatNetwork.newJoiner()` pseudo-interface.
`rpc-twoparty.capnp` is the pinned example of a concrete network encoding.

The pinned C++ runtime has no Join implementation or behavioral test corpus at
this revision. M45 therefore treats the schema's protocol narrative and
network-interface sketch as normative, verifies the wire representation
directly, and uses an independent authenticated model network for behavioral
evidence. This is an explicit oracle limitation, not a claim of C++ behavioral
parity.

## Boundary and lifecycle

`capnp-rpc-core` owns the wire representation. `Join` is message-union
discriminant 12 and carries a question ID, target, and lossless opaque
`JoinKeyPart`. Its answer is an ordinary `Return.results` containing a lossless
opaque `JoinResult`. The two opaque values have distinct Rust types.

`capnp-rpc::DistributedJoin` owns bounded session and path state. A vat supplies
a `JoinNetwork` implementation which authenticates connections, creates one
key part per path, translates key parts and results across transparent proxy
hops, validates each result in its exact session/path/connection context,
detects a common object and host, and establishes the authenticated direct
connection. The coordinator never reads, hashes, concatenates, or manufactures
opaque token bytes.

For a multi-path Join, the coordinator:

1. authenticates every input connection and rejects broken, revoked, mixed
   local/remote, duplicate-question, and over-limit inputs before creating a
   network session;
2. sends exactly one distinct key part on each requested path;
3. forwards only through a caller-resolved fully transparent proxy, with an
   explicit hop limit and network translation at every connection boundary;
4. accepts each result only on its original connection and question path;
5. asks the network to connect only after every result has been validated; and
6. exposes the corresponding `Finish` messages only with the successful direct
   connection, or queues all of them for cleanup after a failed proof.

Cancellation calls the network's session cleanup and returns a `Finish` for
every outstanding path. Join answers remain owned by the active session until
successful connection or cancellation, matching the pinned requirement that
the host be able to observe an early `Finish` as cancellation.

When all candidates are the same local object, Join returns that object
directly. When all candidates are the same import ID on the same authenticated
connection, Join returns that import directly. This is the schema's exact
two-party equality shortcut and creates no key material or extra messages.

## Threat model

The trusted computing base is the local RPC state machine, its transport's
authenticated connection identity, and the `JoinNetwork` implementation. A
network implementation must provide unforgeable, context-bound key parts and
results and must refuse to connect unless every expected part proves the same
root object at the same host. Transport addresses, peer-supplied vat IDs,
targets, and opaque bytes are not independently trusted.

The coordinator and model tests cover these failures:

- A forged or replayed result is rejected by the network before it contributes
  to a session. A valid result on the wrong connection, question, session, or
  path cannot satisfy another slot.
- Results for different objects or hosts cannot produce a direct connection;
  the whole session is canceled and every path becomes finishable.
- Broken and revoked targets fail before key creation. A non-transparent proxy
  is never bypassed. Only the trusted target resolver may label a proxy fully
  transparent.
- Every forward and return relay re-authenticates both endpoints and asks the
  network to translate the opaque value. Raw bearer values are never copied
  between unrelated connection contexts.
- Path/session/hop limits bound retained state and forwarding. Limit failures
  are complete-or-unchanged.

These properties mean an individual intermediary cannot cause unrelated
objects to join, assuming the network's authentication and token-validation
contract. As the pinned schema notes, intermediaries controlling every input
path can reproduce a combined secret; Join establishes agreement among the
supplied paths, not honesty of all colluding holders. It also does not turn an
intentional membrane or opaque proxy into a transparent one.

## Integration and scope

The RPC driver resolves an incoming `Join.target` in the connection context
where it arrived and passes either its local root or an explicitly transparent
forwarding resolution to `route_join()`. It sends the returned action and
relays results using normal question/answer ownership. After a completed Join,
the driver sends the Level-3 `Accept` using the returned
`ThirdPartyCompletion`; M44 owns that handoff and embargo behavior.

The coordinator does not prescribe TLS, Noise, public-key identities, token
formats, or dialing. It does not implement the concrete two-party batching
optimization sketched in `rpc-twoparty.capnp`, integrate a particular async
executor, bypass opaque policy proxies, persist capabilities, or claim a C++
Join oracle that does not exist. M45 remains an implementation candidate until
the M40 release gate and its M44 dependency are activated.
