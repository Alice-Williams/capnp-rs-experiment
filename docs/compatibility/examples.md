# M47 native examples

`capnp-examples` is an end-to-end consumer of the native compiler, code
generator, message, I/O, local RPC, compatibility, Level-3, and persistence
crates. It deliberately builds its schemas at compile time with the native
toolchain; it does not use `capnpc-rust` or import implementation code from the
archived experiment.

The address-book schema retains every explicit node ID from the pinned C++
sample. The example constructs nested person and phone-number lists, enum and
employment-union values, persists the message in standard and packed form,
and compares deterministic generated-reader summaries. The M47 verifier also
feeds the native standard frame to the pinned C++ `capnp decode` tool.

The calculator uses the pinned schema IDs and generated typed clients. Its
service intentionally handles capability-valued results through `LocalResponse`
so each wire capability index has a matching process-local capability table.
It covers a server-created operator, a client callback, recursive expressions,
a client-defined function, a promised `Value` called before the parent result,
and concurrent dispatch.

The platform scenario composes the existing runtime facilities without adding
wire behavior: ordered `ByteStream` writes, explicit cancellation, authenticated
direct-handoff planning, distributed equality's identical-local-object
shortcut, and a sealed SturdyRef restored after replacing the connection
resolver. A restored reference preserves stable object identity but receives
fresh connection state.

## Canonical nested source shape

The pinned C++ schemas nest `PhoneNumber`/`Type` inside `Person` and
`Expression`/`Value`/`Function`/`Operator` inside `Calculator`. The current
native semantic resolver classifies named declarations before contextual
interface methods, so these schemas retain the pinned sample's original nested
shape. A compiled-schema regression covers nested structs, enums, interfaces,
and methods inside an interface. The examples also spell the pinned derived
type IDs explicitly so accidental source movement cannot silently change their
wire identities.

Run all scenarios with:

```sh
cargo run -p capnp-examples --bin m47_examples
```
