# Settled capability imports, exports, and lifetime

M34 implements the settled-capability subset of Level 1 from the pinned
`rpc.capnp` at Cap'n Proto commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The wire binding accepts
`senderHosted`, `receiverHosted`, and tombstone `none` descriptors. Promise,
promised-answer, third-party, and attached-resource descriptors remain
deliberately unsupported until their owning milestones.

## Identity and accounting

`HostedCapability` gives an application object stable process-local identity.
Each connection actor owns a `CapabilityTables` instance. Exporting the same
identity twice reuses the lowest available `ExportId`, but adds two references.
Receiving duplicate `senderHosted` descriptors similarly records two import
references to one peer ID. `receiverHosted` resolves an existing export back to
the identical hosted object without incrementing its export reference count.

`Release.referenceCount` must be non-zero and cannot exceed the held count.
Batch application validates every count before mutation. Payload description
is transactional: a quota or overflow error restores the whole table rather
than retaining a successful prefix. Import count, export count, and aggregate
references are included in `ConnectionStats`.

The actor applies `Return.releaseParamCaps` to the exact duplicate-preserving
list recorded with the original call. Result payloads with capabilities cause
the caller to send `Finish.releaseResultCaps = false`; those imports remain
live until explicitly released. Capability-free results keep the Level-0 fast
path and send `true`. Disconnect clears imports and exports without sending
traffic, matching the four-table connection-lifetime rule.

Calls to imported settled capabilities use `ConnectionHandle::call_imported`.
Calls may carry hosted callbacks; the receiving actor resolves the target to
the original `HostedCapability` and exposes it as `IncomingCallTarget::Hosted`.
Handler results can attach capabilities through
`CompletionToken::complete_with_capabilities`.

## Compatibility evidence

`tools/verify-m34-capabilities.sh` compiles a small program against the pinned
C++ implementation. C++ emits a call containing duplicate `senderHosted`,
`receiverHosted`, and `none` descriptors; native Rust decodes it and emits the
same semantic payload; pinned C++ validates the native frame. Actor tests then
exercise the hosted callback, implicit parameter release, explicit batched
release, ID reuse, exact quotas, and disconnect cleanup.

## Explicit non-goals

M34 does not implement `senderPromise`, `receiverAnswer`, non-empty promised
answer transforms, `Resolve`, embargo, tail routing, attached descriptors,
third-party handoff, cancellation, reconnect, or higher-level generated client
ergonomics. These remain M35 and later.
