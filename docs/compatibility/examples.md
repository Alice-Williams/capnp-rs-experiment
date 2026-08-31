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

## Native compiler source-shape limitation

The pinned C++ schemas nest `PhoneNumber`/`PhoneType` inside `Person` and
`Expression`/`Value`/`Function`/`Operator` inside `Calculator`. The current
native semantic resolver does not yet classify declarations nested in an
interface correctly. M47 therefore keeps those declarations at file scope and
spells every pinned type ID explicitly. This preserves their wire identities
and generated APIs while recording the remaining source-language parity gap;
it is not a claim that nested-interface declaration resolution is complete.

Run all scenarios with:

```sh
cargo run -p capnp-examples --bin m47_examples
```
