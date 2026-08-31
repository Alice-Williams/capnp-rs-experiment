# Native JSON codec

`capnp-json` converts reflected values without generated code, and `capnp-cli
convert` exposes the common binary/JSON workflows without invoking C++:

```console
capnp-cli convert --short binary:json schemas/app.capnp App < app.bin
capnp-cli convert json:binary schemas/app.capnp App < app.json > app.bin
capnp-cli convert packed:json schemas/app.capnp App < app.packed
capnp-cli convert json:flat schemas/app.capnp App < app.json > app.flat
```

Standard framing is named `binary`; `packed`, `flat`, and `json` are also
accepted. `--short` selects compact JSON and the default is pretty JSON. The
native `convert` command currently requires one side to be JSON; binary-only
format conversion remains available by composing the lower-level framing APIs.

## Compatibility policy

The oracle is `capnp/compat/json.{h,c++}` and `json.capnp` from pinned C++
commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`:

| Cap'n Proto value | JSON representation |
| --- | --- |
| Void | `null` |
| Bool, 8/16/32-bit integers | boolean or number |
| signed/unsigned 64-bit integer | decimal string, preventing precision loss |
| finite float | number, including `-0` |
| positive infinity, negative infinity, NaN | `"Infinity"`, `"-Infinity"`, `"NaN"` |
| Text | string |
| Data | array of integers in `[0, 255]` |
| known enum | schema name string |
| unknown enum ordinal | number on encode; default decoding rejects it |
| List | array |
| Struct/group | object |
| Capability or AnyPointer | explicit handler required |

Primitive fields are emitted even when default-valued. Null pointer fields are
omitted. `HasMode::NonDefault` also omits scalars equal to their schema default.
Unknown input object fields are ignored for schema evolution by default;
`set_reject_unknown_fields(true)` makes them errors. Duplicate fields are
always rejected.

Input has independent byte, value-count, nesting (64 by default), and output
message-word limits. Syntax errors carry byte, one-based line, and one-based
column positions. JSON numbers retain their spelling until conversion, so
integer range checks happen against the actual Cap'n Proto field type.

## Annotations and extensions

The CLI enables the upstream `/capnp/compat/json.capnp` annotations
automatically. The library enables them with `handle_by_annotation(true)`:

- `$Json.name` on fields and enumerants;
- `$Json.base64` and `$Json.hex` on Data fields;
- `$Json.flatten`, including prefixes;
- `$Json.discriminator`, including a shared `valueName`.

`JsonHandler` provides deterministic encode/decode overrides registered by
reflected type or by `(struct ID, field name)`. Decode handlers return the
canonical JSON shape consumed by the built-in decoder, avoiding access to an
arena or unsafe lifetimes. Handlers are `Send + Sync` and stored behind `Arc`.

## Conformance gate

`tools/verify-m28-json.sh` checks both directions against pinned C++ 2.0-dev
for the complete wire corpus and an annotation-heavy corpus. It proves that
C++ decodes native messages to the same JSON and native code decodes C++
messages to the same JSON. Fixture and vendored-schema hashes are separate
Bazel gates.
