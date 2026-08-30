# M01 — Reference corpus and benchmark harness

- Status: in-progress
- Phase: 0
- Depends on: M00

## Outcome

Check in independently generated C++ and current-Rust schemas/fixtures, provenance metadata, and a benchmark harness that reports hardware context.

## Implementation checklist

- [x] Check in the four standard schemas from the pinned C++ commit with exact
  provenance and SHA-256 values.
- [x] Add reproducible remote acquisition and local corpus-integrity scripts.
- [x] Make checked-in schema integrity a Bazel and CI test.
- [ ] Add project-owned schemas covering every pointer, list, evolution, schema,
  and interface category.
- [ ] Build the pinned C++ oracle and check in generated wire/compiler fixtures.
- [ ] Build the pinned current-Rust oracle and add its secondary fixtures.
- [ ] Add fixture metadata that records command line, producer commit, schema,
  and output hash.
- [ ] Add benchmark harnesses and hardware metadata output.
- [ ] Record primary C++ and secondary current-Rust baseline results.
- [ ] Update compatibility evidence and run full Cargo/Bazel validation.

## Required exit evidence

Every pointer, list, and schema category has an oracle fixture; provenance hashes verify; C++ baselines are primary and current Rust is secondary.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
