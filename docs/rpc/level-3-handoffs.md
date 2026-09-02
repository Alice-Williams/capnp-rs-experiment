# Level-3 introductions and handoffs

M44 implements the Level-3 wire surface and an executor-neutral coordinator
for authenticated three- and four-vat handoffs. The authoritative compatibility
sources are `rpc.capnp` and the C++ `rpc.h`/`rpc.c++` implementation at pinned
commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The exact upstream behavioral
oracle is `rpc-test.c++:2267-2625`.

## Boundary

`capnp-rpc-core` owns the pinned message representation. It preserves arbitrary
pointer-shaped network tokens in distinct `ThirdPartyToContact`,
`ThirdPartyToAwait`, and `ThirdPartyCompletion` types; supports `Provide`,
`Accept`, `ThirdPartyAnswer`, `sendResultsTo.thirdParty`,
`awaitFromThirdParty`, `thirdPartyHosted`, and `Disembargo.context.accept`; and
enforces the callee answer-ID range `[2^30, 2^31)` and bounded embargo IDs.

`capnp-rpc::Level3Router` owns cross-connection state. A vat supplies a
`Level3Network` implementation which:

- returns only transport-authenticated peer identities;
- mints and interprets opaque introduction tokens in the connection context
  where they are valid;
- translates a contact token only when forwarding to the requested destination
  is authorized;
- maps await/completion tokens to a private rendezvous key; and
- generates provision-scoped, unique embargo IDs.

The router does not inspect token bytes or construct capability authority. The
application resolves an incoming `Provide.target` through the connection that
received it and passes that exact capability value to `provide()`. Accepting or
forwarding only clones that value. This separation lets TLS identities, Unix
credentials, Noise keys, object-capability transports, or test networks define
authentication without imposing one transport or executor on the runtime.

## State and ordering

A provision is keyed both by its connection-scoped question ID and by the
network's private rendezvous key. It remains alive until the proxy vine is
released and `finish_provision()` is called. An `Accept` may arrive before its
matching `Provide`; it remains pending and receives no authority until the
rendezvous exists.

An accept with a non-empty embargo remains pending after rendezvous. A matching
`Disembargo.context.accept` addressed to the provision's promised answer marks
that provision-scoped ID released, after which queued accepts complete in input
order. A disembargo addressed to an imported capability produces an explicit
`ForwardVine` action so it travels the old path rather than being cleared
locally. Finishing the provision fails every still-pending accept.

`await_return()` and `third_party_answer()` may arrive in either order. Once
both authenticated tokens map to the same rendezvous, the router records the
direct connection's callee-allocated answer ID and the original connection's
question ID until the final return is consumed. Answer-ID collisions are
rejected across both pending and adopted routes.

Every table has an explicit limit. Provision, pending-accept, embargo, and
return-route limit failures are complete-or-unchanged. Invalid or contextually
forged tokens fail inside the network hook. If introductions or forwarding are
unsupported or fail, planning returns `HandoffPlan::Proxy`; the ordinary
`vineId` remains a sender-hosted import and is released with normal Level-1
accounting.

## Evidence and scope

Deterministic native simulations cover lazy three-party pickup, accept-before-
provide, introduce-to-self, embargo ordering, four-party forwarding, denied
forwarding with proxy fallback, three-leg reflected forwarding, vine teardown,
forged-token rejection, quotas, and both third-party return arrival orders.
`tools/verify-m44-level3-handoffs.sh` also executes all six exact pinned C++
handoff/embargo cases.

M44 does not define a public-internet identity scheme, make opaque tokens
portable between unrelated connections, pass file descriptors during an
introduction, implement Level-4 `Join`, or implement persistent capabilities.
The completed M40 release gate activated these Level-3 APIs; deployments still
have to supply their authenticated network identity and token policy.
