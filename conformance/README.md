# Conformance corpus

Everything below this directory must identify an independent producer and
exact source revision. Runtime tests may consume these files, but production
code must not generate its own expected values.

## Layout

- `upstream/capnproto/<commit>/`: exact upstream schemas or test inputs copied
  from the pinned C++ product oracle.
- `oracles/<implementation>/<commit>/`: build provenance for independently
  installed fixture producers; the installations themselves are not committed.
- `schemas/`: project-owned coverage/evolution schemas.
- `fixtures/cpp/<commit>/`: bytes and text metadata emitted by the pinned C++
  tools.
- `fixtures/capnproto-rust/<commit>/`: secondary Rust-oracle outputs.
- `fixtures/hand-authored/`: tiny word-exact fixtures whose derivation is
  documented next to the file.

Generated fixture directories are committed deliberately. Each generator must
write to a temporary location, verify its producer revision, and install output
atomically so partial files cannot look authoritative. Repository text uses LF
on every host so schema and textual-input hashes are reproducible inside the
Linux development environment.

The pinned C++ oracle is built into the persistent container volume with:

```console
bash tools/build-cpp-oracle.sh
```

Its source, build, and install trees live under `/opt/capnp-oracles` and are not
part of the Git repository or Docker image.

The secondary current-Rust code-generation oracle is installed similarly:

```console
bash tools/build-rust-oracle.sh
```
