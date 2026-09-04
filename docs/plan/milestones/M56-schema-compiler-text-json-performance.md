# M56 — Schema, compiler, text, and JSON performance

- Status: in progress
- Phase: 8
- Depends on: M50, M52, M53, M54, M55

## Outcome

Make the native dynamic-data and tooling stack preserve the performance already
earned by the wire, message, packing, and generated-data layers. Reflection is
intentionally more general than generated access, but its own schema lookup and
dynamic dispatch must be efficient. Schema compilation, text conversion, and
JSON conversion must then add no unexplained cost relative to the pinned C++
implementation.

## Ordered performance layers

M56 proceeds upward through five independently reported layers:

1. reflected schema lookup and dynamic scalar, blob, list, struct, union, and
   builder access with an already-loaded schema and validated message;
2. schema parsing, import resolution, semantic analysis, request construction,
   and Rust code generation, both phase-isolated and end to end;
3. schema-aware Cap'n Proto text formatting and parsing;
4. schema-aware JSON formatting and parsing, with annotations disabled and
   enabled as separate workloads;
5. native CLI decode, encode, eval, convert, and compile workflows, including
   process and file-I/O cost only when the matched C++ workload includes it.

The next layer does not begin until the current layer has a checked-in matched
comparison, attributable profile or phase evidence for material gaps, and a
documented disposition for every gate.

## Inherited performance contract

Each workload has a paired lower-layer control which performs the same wire,
message-read/build, packing, or dynamic-data operation and observes the same
semantic result. The paired lower-layer native/C++ ratio, plus the program's 3%
measurement tolerance, is the cumulative ceiling for the higher-layer
workload. After subtracting the paired lower-layer medians, the incremental
native/C++ ratio must be no greater than 1.03.

Reflection lookup, schema loading, parsing, semantic analysis, request
construction, code generation, text syntax, JSON syntax, dynamic conversion,
framing, and process startup are timed separately wherever subtraction would
combine unlike work or amplify timer noise. A faster phase may not hide a
slower phase. Throughput workloads also report bytes and schema declarations or
message values processed so corpus size cannot distort the comparison.

## Comparison contract

- Pin C++ to `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`
  and use byte-identical schemas, messages, text, JSON, import graphs, and
  compiler options wherever both implementations support the operation.
- Start with the existing conformance schemas and wire fixtures. Add small,
  medium, import-heavy, annotation-heavy, and evolution corpora only with
  checked-in provenance and exact content hashes.
- Separate cold schema load or index construction from repeated hot field
  lookup and access. Report name lookup and cached field-descriptor access as
  different reflection workloads.
- Require identical decoded values and semantic checksums. Require
  byte-identical compiler requests and binary output where compatibility
  already promises it; compare normalized generated source or independently
  compile and exercise it when formatting legitimately differs.
- Match validation state, traversal and nesting limits, output limits,
  allocation policy, scratch reuse, formatting style, annotation handling,
  and file or stream boundaries. Do not exclude setup from only one language.
- Record two warmups, nine alternating optimized samples, binary hashes,
  producer commits, compiler versions, host/container metadata, medians,
  ranges, cumulative ratios, incremental ratios, and peak resident memory for
  compiler and whole-document codec workloads.
- Preserve exact defaults, unknown enum/union behavior, schema evolution,
  import resolution, diagnostic locations, resource limits, and deterministic
  output. Optimization may not introduce `unsafe` code.

## Required benchmark shapes

- Reflection: field lookup by name, cached descriptor access, scalar/default
  and enum reads/writes, text/data views, primitive and pointer lists, nested
  structs/groups, active and unknown union discriminants, and schema-evolution
  reads.
- Compiler: one small file; the complete language fixture; an import graph;
  parse-only, resolve-only, request emission, Rust generation, and full compile.
- Text: compact and pretty formatting plus parsing for scalar-heavy,
  pointer-heavy, nested/list, union/default, and evolution values.
- JSON: compact and pretty formatting plus parsing for the same value shapes;
  64-bit integers and non-finite floats; annotations, flattening, base64/hex,
  and strict/evolution-tolerant field handling as separate cases.
- CLI: warm process and cold process results are not conflated. Streaming cases
  include enough messages to amortize startup and report per-message cost.

## Implementation checklist

- [ ] Trace native and pinned C++ reflection representations and hot access
  paths; add matched descriptor-lookup and dynamic-access benchmarks.
- [ ] Record and verify the unmodified reflection baseline, then close every
  reflection gate before proceeding upward.
- [ ] Add phase-isolated and end-to-end schema/compiler benchmarks with exact
  request and generated-output correctness checks.
- [ ] Attribute and close schema parsing, resolution, request construction, and
  code-generation gaps independently.
- [ ] Add matched text-format and text-parse benchmarks and close their
  cumulative and incremental gates.
- [ ] Add matched JSON-format and JSON-parse benchmarks, including annotation
  variants, and close their cumulative and incremental gates.
- [ ] Add streaming and process-level CLI comparisons without hiding startup or
  I/O in only one implementation.
- [ ] Record final performance and memory evidence for every required shape.
- [ ] Add Bazel evidence gates and pass full Cargo/MSRV/Bazel/Miri validation.

## Required exit evidence

Every required workload preserves its paired lower-layer cumulative ceiling and
has incremental native/C++ cost no greater than 1.03, subject only to the
documented 3% measurement tolerance. If an API intentionally provides stronger
ownership, safety, diagnostics, determinism, or resource limiting than C++, it
must also have a like-for-like isolated benchmark; unlike semantics are never
used to waive a performance gate. Compiler and codec peak memory must have no
unexplained regression greater than 5% against C++ on the same corpus.

## Scope boundary

M56 may change reflection/schema representations, schema loading and compiler
internals, native code generation, text/JSON codecs, and narrowly required CLI
plumbing. It does not optimize RPC control messages, actor scheduling,
transports, capability lifecycle, or end-to-end application workloads; those
belong to M57 and M58.
