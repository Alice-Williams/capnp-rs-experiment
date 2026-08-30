# Conformance corpus

Everything below this directory must identify an independent producer and
exact source revision. Runtime tests may consume these files, but production
code must not generate its own expected values.

## Layout

- `upstream/capnproto/<commit>/`: exact upstream schemas or test inputs copied
  from the pinned C++ product oracle.
- `schemas/`: project-owned coverage/evolution schemas.
- `fixtures/cpp/<commit>/`: bytes and text metadata emitted by the pinned C++
  tools.
- `fixtures/capnproto-rust/<commit>/`: secondary Rust-oracle outputs.
- `fixtures/hand-authored/`: tiny word-exact fixtures whose derivation is
  documented next to the file.

Generated fixture directories are committed deliberately. Each generator must
write to a temporary location, verify its producer revision, and install output
atomically so partial files cannot look authoritative.
