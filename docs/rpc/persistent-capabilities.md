# Persistent capabilities and SturdyRefs

M46 implements the application-level persistence model from the pinned
`persistent.capnp` schema at commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. The schema is authoritative for
the generic `Persistent<SturdyRef, Owner>.save()` contract, owner sealing, realm
separation, and the `$persistent` annotation. It deliberately does not define
a SturdyRef representation or a universal restore interface; those are realm
and application policy.

## Runtime boundary

`capnp-rpc::PersistenceManager` separates three trusted application hooks:

- `PersistenceRealm` mints and validates opaque, realm-branded SturdyRefs. A
  production realm is responsible for cryptographic authenticity, unguessable
  object authority, host discovery/authentication data, and key rotation.
- `PersistentStore` durably commits canonical grant records and grant- or
  object-wide revocation before returning success.
- `PersistentResolver` maps a live capability to a stable object ID when saved
  and maps that ID back to a newly usable capability when restored.

The durable `GrantRecord` contains only grant ID, stable object ID, optional
owner, expiry, and revocation. Its type has no live capability, transport, or
connection field. Reconnect is therefore restoration through the resolver,
not serialization of an import/export/question/connection ID.

`SturdyRef<Realm>` is an opaque byte sequence because applications must store
and reload it, but realm branding catches accidental cross-realm use at
compile time. Bytes loaded from disk remain untrusted. Restore checks the byte
limit, realm authentication, durable lookup, exact claim/store agreement,
revocation, expiry, and authenticated owner before asking the resolver for any
capability. A realm transition must explicitly authenticate and transform the
token into the destination realm; byte rebranding cannot pass destination
validation unless that realm deliberately accepts it.

## Owner and lifetime semantics

`SaveOptions.seal_for` corresponds to `SaveParams.sealFor`. A sealed token can
only be restored with an `AuthenticatedOwner` equal to the durable owner. The
application or transport constructs that wrapper only after its realm-defined
proof succeeds. An unsealed token is bearer authority if the realm permits its
issuance; a stricter realm rejects such issuance in `PersistenceRealm::issue`.

Expiry is an absolute application-defined `u64`. Callers supply `now`, avoiding
a hidden wall clock and making restore deterministic. `now >= expires_at`
fails. Saving an already expired grant is rejected before realm or store state
changes.

A SturdyRef is not an owning pointer. Cloning or dropping its bytes has no
effect on the underlying object, consistent with the pinned warning against
using SturdyRefs as owned references. `revoke()` durably revokes one grant;
`revoke_object()` revokes all current grants for a stable object. Both remain
effective after manager/process restart.

## Threat model and evidence

The realm, durable store, resolver, and owner-authentication boundary are
trusted. Serialized tokens, clocks supplied by remote peers, claimed owners,
network addresses, and all live connection IDs are not trusted. Applications
must use a trusted local time source and must not expose `AuthenticatedOwner`
construction to untrusted code.

The deterministic model tests cover process restart with a fresh connection,
sealed and unsealed restore, missing/wrong owners, expiry boundaries, token
tampering, oversized input, valid-but-unknown grants, canonical-store claim
mismatch, individual revocation, object-wide revocation, non-owning token drop,
and durable revocation after restart. The model token authenticator is only a
test oracle, not production cryptography.

The pinned C++ tree provides the schema but no universal persistence runtime or
behavioral corpus, which follows from the schema's application-defined realm
design. M46 does not choose a database, key-management system, public identity,
wall clock, network dialer, concrete SturdyRef encoding, or automatic realm
gateway. It remains an implementation candidate until the M40 release gate is
complete.
