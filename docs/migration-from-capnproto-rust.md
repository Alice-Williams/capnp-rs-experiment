# Migrating from `capnproto-rust`

This v1 candidate preserves Cap'n Proto wire, generated-data, and two-party
Level-1 semantics; it is not a drop-in replacement for `capnp`, `capnpc`, or
`capnp-rpc`. Migrate one schema and boundary at a time and keep a
cross-language fixture in CI until the application is fully moved.

The crates are experimental and currently `publish = false`. Consumers must
use a pinned Git revision or workspace path rather than crates.io.

## Build and generated code

Replace a `capnpc::CompilerCommand` build script with the native CLI or the
`capnp-compiler` library. The CLI accepts multiple imports, `--src-prefix`,
crate mappings, raw `CodeGeneratorRequest` streams, external plugins, and
built-in Rust output:

```console
cargo run -p capnp-cli -- compile \
  --src-prefix schemas \
  --output rust:src/generated \
  schemas/addressbook.capnp
```

Generated modules use builders/readers backed by `capnp-schema` and
`capnp-message`; they do not reproduce every historical `capnp::traits` type.
Compile the old and new generated modules side by side while migrating call
sites. Preserve field ordinals and schema IDs—the wire does not change.

## Reading and writing messages

`capnproto-rust` commonly returns a reader borrowing an input buffer. Here,
`OwnedMessage` shares immutable segment storage and an exact traversal budget,
so typed roots and retained object references can safely cross threads:

```rust
use capnp_message::{OwnedMessage, ReaderLimits};

let root_word = vec![0_u8; 8];
let message = OwnedMessage::new(vec![root_word], ReaderLimits {
    traversal_words: 1024,
    nesting_levels: 32,
})?;
let root = message.root_struct()?;
let same_message_on_a_worker = root.clone();
# Ok::<(), Box<dyn std::error::Error>>(())
```

For standard stream framing, replace `serialize::read_message` /
`write_message` with bounded `capnp_io::read_frame` / `write_frame`, then
construct an `OwnedMessage` from the returned frame's segments. Packed and
async adapters are separate, explicit layers. Reader limits are shared
security state, not advisory options.

Builders use `ExclusiveArena`; mutation is intentionally single-owner.
Parallel construction requires the opt-in partition APIs, which expose only
provably disjoint primitive slices or sealed worker fragments.

## RPC clients and servers

Instead of binding the protocol core to one executor, construct a
`ConnectionDriver<T: DuplexTransport>` and drive each returned
`DriverDispatch` on the application executor. Complete it through its
`DriverCompletion`; only the connection actor may mutate protocol tables.

Choose server ordering explicitly:

- `Concurrent<S>` for independent thread-safe calls;
- `Serial<S>` for FIFO stateful service access;
- `Keyed<S, K, F>` for serialization by application key;
- `LocalServer<S>` when state must be constructed and kept on one dedicated
  thread and cannot implement `Send`.

Generated streaming methods dispatch before returning. Await the returned
readiness/acknowledgement future only to govern the next send. Cancellation is
cooperative: inspect `DriverCompletion::cancellation()` or call
`disallow_cancellation()` before cancellation wins. `DriverShutdown` must be
awaited to observe transport-close errors.

Reconnect through `CapabilityReconnector`; retry only disconnected failures.
Overload means back off, and application calls are never replayed implicitly.

## Intentional differences to audit

| Existing assumption | v1 candidate behavior |
|---|---|
| Runtime-specific RPC system owns spawning | Core is executor-neutral; executor and scheduling wrapper are explicit |
| Server objects are usually serialized by the runtime | Default can overlap; choose `Serial` or `Keyed` when ordering is required |
| Borrowed readers remain on one task | Owned messages and stable coordinate references are naturally `Send + Sync` |
| Traversal limits are local reader options | All clones consume one exact shared budget |
| Builder mutation can be shared through application synchronization | Ordinary builders remain exclusive; only typed partitions cross workers |
| Reconnect wrappers may retry transparently | Generations recreate authority, but callers own replay safety and backoff |
| Local capability helpers and membranes are broadly available | M41–M43 provide local clients, pipelines, revocation, membranes, and attached-resource boundaries; transport-specific authentication and OS-handle policy remain explicit |

Before switching production traffic, run
`tools/verify-m40-level1-interop.sh` at the pinned revision and add an
application-specific Rust/C++ fixture for every schema and RPC method family
the application actually uses.
