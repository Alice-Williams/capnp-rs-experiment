# Native text tools

`capnp-cli` implements the schema-aware `decode`, `encode`, and `eval`
workflows without invoking the C++ executable. It uses the same native source
loader and compiler as `capnp-cli compile`.

## Decode binary messages

Standard framed messages are the default. Every message in the input stream is
printed in order:

```console
capnp-cli decode schemas/app.capnp App < messages.bin
```

Use `--short` for one line per message, `--packed` for Cap'n Proto packed
framing, or `--flat` for one unframed, single-segment message. `-I DIR` and
`--import-path=DIR` add roots for absolute schema imports.

The printer emits fields in schema `codeOrder`, omits absent pointer fields,
prints only the active union arm, resolves enum names, preserves unknown enum
ordinals, and uses reference-compatible escapes for Text and Data.

## Encode text values

Input is one or more Cap'n Proto struct literals:

```console
capnp-cli encode schemas/app.capnp App <<'EOF' > messages.bin
(name = "first", enabled = true, values = [1, 2, 3])
(name = "second", enabled = false, values = [])
EOF
```

The default output is a stream of standard framed messages. `--packed` emits a
packed stream. `--flat` requires exactly one value and a result that fits in one
segment. Text parsing has independent byte, value-count, nesting, and message
word limits. Errors report a one-based line and column plus the byte offset.

Structs, groups, lists, nested lists, enums, all scalar types, Text, and Data
are supported. Capability and unconstrained AnyPointer values can only be
absent/null; textual capability injection is intentionally unsupported.

## Evaluate constants and defaults

`eval` prints constants, field defaults, nested struct members, and list
elements:

```console
capnp-cli eval schemas/config.capnp Config.defaultTimeout
capnp-cli eval schemas/config.capnp 'Config.primes[3]'
capnp-cli eval schemas/app.capnp Shared.Config.sample.name
```

The last form follows a source-level import alias (`Shared`) before resolving
the imported and nested constant. `-b`/`-obinary`, `-p`/`-opacked`, and
`--flat`/`-oflat` emit struct-valued expressions in standard, packed, or flat
binary form. Binary output rejects scalar and list values because a Cap'n Proto
message root must be a struct for this workflow.

## Compatibility gate

`tools/verify-m27-text.sh` checks the pinned C++ 2.0-dev oracle in both
directions for standard and packed messages, compares the complete reference
text corpus, and compares direct, nested, list-item, and imported evaluation.
JSON is a separate M28 codec and is not accepted as Cap'n Proto text.
