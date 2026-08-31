# Attached resources and Unix file descriptors

M43 implements Cap'n Proto's orthogonal `CapDescriptor.attachedFd` field and a
bounded Unix `SCM_RIGHTS` transport. The protocol capability remains the source
of RPC authority: an attachment is an optional local optimization/resource, so
a peer that receives no descriptor still imports and can call the capability.

## Compatibility oracle

The behavior is pinned to Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`:

- `rpc.capnp` lines 1112–1166 define `attachedFd :UInt8 = 0xff`, first-winner
  duplicate indices, absent out-of-range indices, per-message limits, and
  closing unused descriptors.
- `rpc.h` lines 190–232 define move-out ownership for received descriptors and
  require the outgoing descriptor owner to survive until the write completes.
- `rpc.c++` lines 405–438 retain the outgoing capability owners beside the
  queued write.
- `rpc-twoparty-test.c++` lines 549–795 exercise FD transfer across small and
  1 MiB calls/results/pipelines, receive limits, and an attachment whose source
  capability is dropped before the queued message is written.
- `membrane.h` keeps FD passthrough disabled by default. The Rust local
  membrane API likewise never forwards a transport attachment implicitly.

`tools/verify-m43-attached-resources.sh` runs the three exact pinned C++ cases
and then the native ownership, wire-binding, actor, and Unix transport ports.

## Ownership model

`OwnedResource` is still the move-only transport envelope value introduced in
M32. `AttachedResource` turns one such value into a clonable handle backed by
one owner. Cloning the handle does not duplicate a file descriptor. The value
is destroyed exactly once when the last capability/import/envelope handle is
dropped. Synchronous `with()` access deliberately prevents exposing a guard
that could be held across an `.await`.

Applications attach a resource to an `OutgoingCapability` with
`with_attachment()`. The connection actor transactionally assigns distinct
indices 0 through 254, encodes those indices in the capability table, and
moves matching transport-resource handles into `ActorEffect::SendWithResources`.
Index 255 is rejected because the schema reserves it for “no attachment”. A
256th attachment fails the whole capability-table description without leaving
partial export accounting.

On receive, `bind_attached_resources()` consumes the entire resource vector.
The first descriptor naming an in-range index takes that owner. A duplicate or
out-of-range descriptor receives `None`; unreferenced resources are dropped
before binding returns. For repeated descriptors of one imported capability,
the first successfully bound attachment remains associated with the import.
Releasing the last import reference releases that ownership handle.

## Unix transport

On Unix, `UnixScmRightsTransport` wraps a connected `UnixStream` and implements
the executor-neutral `DuplexTransport` contract. It:

- validates every outgoing value as an `OwnedFd` (directly or through an
  `AttachedResource`) before writing any frame byte;
- duplicates descriptors only for the `sendmsg()` syscall while retaining the
  original envelope until the complete Cap'n Proto frame is written;
- reads exactly one standard frame at a time, preventing ancillary data from
  drifting to a following message on a stream socket;
- creates received descriptors as `OwnedFd` immediately with close-on-exec;
- bounds retained descriptors by both transport and envelope limits; and
- relies on control truncation plus owned drops to close every excess value.

Unsupported outgoing resource types return `UnsupportedResource` without
consuming the envelope or partially writing the frame. A receiver configured
with a zero-descriptor limit safely discards all file descriptors while still
delivering the RPC message, demonstrating the capability fallback path.

The Unix implementation uses safe `rustix` and `async-io` APIs; the workspace
continues to forbid unsafe Rust. Non-Unix builds retain the generic ownership,
wire, actor, and transport-envelope APIs but do not export the Unix adapter.

## Explicit non-goals

M43 does not pass file descriptors through M42 local membranes, invent Windows
handle-transfer semantics, provide TCP descriptor transfer, or add three-party
handoff/equality/persistence. Those boundaries remain conservative or belong
to M44–M46. Activation of this stacked implementation candidate still awaits
the M40 release-soak gate.
