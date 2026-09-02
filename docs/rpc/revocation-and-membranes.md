# Revocation and membranes

M42 implements the local capability membrane semantics defined by pinned
Cap'n Proto C++ `membrane.h`, `membrane.c++`, `membrane-test.c++`, and the
`RevocableServer` case in `capability-test.c++`. A membrane is identified by
one shared `Membrane` state/policy object. Direction is part of each wrapper:
normal wrappers expose an inside target to outside callers; reverse wrappers
expose an outside target to inside callers.

## Crossing invariants

- A target/direction pair has at most one live wrapper identity. Wrapping the
  same target in the same direction reuses it. Crossing the same membrane in
  the opposite direction removes the existing wrapper and returns the original
  capability.
- Request capabilities cross opposite the wrapper direction; result and
  provisional-pipeline capabilities cross with the wrapper direction. The
  immutable message bytes and every capability-table index remain unchanged.
- `LocalRequest` and `LocalResponse` carry bounded process-local capability
  tables. Copying a struct, list, or AnyPointer graph across a local membrane
  therefore clones the immutable message and transforms its complete external
  capability table without rewriting wire pointers.
- Promise wrappers remain promise-like. Resolution is wrapped or unwrapped
  through the same registry, and concurrent resolution observers receive the
  same live resolved identity.
- The registry contains weak client/schema references. It cannot keep an
  otherwise dropped target, wrapper, or server alive.

## Policy and revocation

`MembranePolicy` receives inbound and outbound calls with the interface ID,
method ordinal, and underlying target. It may forward, redirect to a capability
already on the caller's side, or reject. Forwarding recursively transforms
request, response, and provisional-pipeline capability tables. Redirection is
deliberately not auto-transformed.

When `should_resolve_before_redirecting()` is enabled for a promise target, the
runtime waits for target resolution and reapplies the same membrane identity.
If the promise reflects a capability back to its original side, the opposite
wrapper is removed and policy redirection is bypassed, matching the pinned C++
reflection cases.

`Membrane::revoke()` atomically records a stable failure and wakes every
outstanding forwarded response. Existing pipeline-derived clients and all later
calls observe that failure. `RevocableServer` adds `is_in_use()` accounting:
live wrapper clients and outstanding calls count; weak registry entries and the
controller do not.

`MembraneLimits` bounds live wrapper registrations and outstanding forwarded
calls. Limits are enforced before wrapper creation or target dispatch.

## Evidence and scope

`tools/verify-m42-membranes.sh` runs the pinned C++ `RevocableServer` case and
all 17 membrane cases at `membrane-test.c++` lines 189–398, then the native
identity, request/result/pipeline/copy, policy, promise, quota, wakeup,
revocation, and lifetime ports.

M42 does not allow attached descriptors or file descriptors through a membrane;
those remain M43. Three-party handoff, distributed equality, persistence, and
compatibility adapters remain separate M44–M47 boundaries. The completed M40
release soak activated the stacked M41/M42 implementation.
