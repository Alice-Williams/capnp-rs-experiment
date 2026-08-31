# Native compile and ID workflows

`capnp-cli` replaces the normal schema-compilation subset of the C++ `capnp`
tool without invoking or linking the C++ implementation. Build or install it
with Cargo, then substitute the commands below in build scripts.

## Generate Rust directly

The common reference workflow:

```console
capnp compile -Ischemas --src-prefix=schemas -orust:generated schemas/app.capnp
```

becomes:

```console
capnp-cli compile -Ischemas --src-prefix=schemas -orust:generated schemas/app.capnp
```

Every explicit input is one requested file. Relative imports resolve beside
the importing file first. Rooted imports such as `/common/types.capnp` search
the source prefix and each `-I` directory in command-line order. Inputs must be
inside `--src-prefix`; this makes request filenames stable and prevents a path
from escaping its declared schema root.

Pass several input files to compile them into one shared request:

```console
capnp-cli compile --src-prefix=schemas -Ischemas -orust:generated \
  schemas/client.capnp schemas/server.capnp
```

Rust files are named from the requested schema filenames. A collision is an
error instead of silently overwriting a file.

## External generated crates

Map an imported schema file ID to an existing Rust module with a hexadecimal
`--import-map` value:

```console
capnp-cli compile --src-prefix=schemas -orust:generated \
  --import-map=8f00aa11bb22cc33=my_protocol::common \
  schemas/service.capnp
```

Repeat the option for multiple external crates. The mapping affects generated
Rust references only; the standard request still contains the imported schema
metadata required by plugins.

## Raw requests and external plugins

`-o-` writes one standard, unpacked `CodeGeneratorRequest` to stdout:

```console
capnp-cli compile --src-prefix=schemas -Ivendor -o- schemas/app.capnp > request.bin
```

Any other output name follows the Cap'n Proto plugin convention. For example,
`-ocustom:generated` starts `capnpc-custom`, sets its working directory to
`generated`, and writes the complete request to its standard input. Plugin
stdout and stderr remain attached to the invoking process, and a spawn, write,
or non-zero-exit failure is returned to the caller.

The native `rust` target is built in and therefore does not search for a
`capnpc-rust` executable.

## Generate a schema ID

```console
capnp-cli id
```

The output has schema syntax such as `@0xd8d4be8f95f000c1;`. Eight bytes are
read from the Linux system CSPRNG and the required high bit is set. Opening or
reading the entropy source is fallible and the command reports the operating
system error without printing an ID. On a non-Linux host, run this command in
the repository Dev Container, which is the supported unified tool environment.

## Current boundary

M26 covers `compile` and `id`. The M27 text workflows are documented in
[`text-tools.md`](text-tools.md). JSON is M28. Packed message conversion is a
text-tool message mode, not a schema compilation option.
