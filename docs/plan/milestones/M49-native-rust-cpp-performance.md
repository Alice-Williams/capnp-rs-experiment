# M49 — Native Rust versus C++ performance characterization

- Status: in-progress
- Phase: 8
- Depends on: M48

## Outcome

Measure the v0.1 native Rust implementation against the pinned C++ product
oracle in equivalent data and RPC scenarios, explain material differences from
profiles or isolated measurements, and identify any workloads where native
Rust is faster.

## Fair-comparison rules

- Compare the native workspace at an exact commit with C++ commit
  `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.
- Use semantically identical schemas, fixtures, operation counts, and checksum
  validation. Do not compare M40 and M48 soak-session rates.
- Build optimized binaries, warm them before sampling, alternate
  implementation order, and retain raw samples plus host/toolchain metadata.
- Separate build, read, standard framing, packed framing, and RPC costs where
  possible. Do not present application-defined persistence or C++-unsupported
  Level 4 Join behavior as cross-language comparisons.
- Treat the existing M01 numbers as C++ versus upstream `capnproto-rust`
  context only; they are not measurements of this native implementation.

## Implementation checklist

- [x] Extend the checked-in runner with native Rust executables for the common
  C++ carsales, catrank, and expression-evaluation workloads.
- [x] Add a native Rust two-party RPC benchmark matching the existing Ping
  schema, sequential request/reply pattern, and in-memory transport.
- [x] Record fresh data and RPC comparison runs with raw samples and
  environment metadata.
- [ ] Attribute material gaps using profiles or targeted component timings.
- [ ] Publish a concise report covering slower, faster, and inapplicable cases.
- [ ] Add result-integrity and runner checks to Bazel/CI.
- [ ] Run the full Cargo and Bazel validation gates.

## Required exit evidence

Checked-in source and scripts reproduce every stated comparison, raw samples
support the summary, checksums prove equivalent results, and each causal claim
is backed by a profile or an isolated benchmark.

## Scope boundary

This milestone characterizes and diagnoses v0.1. Optimization changes are
separate follow-up work unless they are needed to make a benchmark equivalent
or correct.
