# Engineering plan

This plan turns the 2026-08-29 parallel-native Cap'n Proto engineering dossier
into reviewable, dependency-ordered work. Each milestone has a separate task
file under `milestones/`; its exit evidence becomes a permanent compatibility
gate for later work.

## How to use this plan

1. Work on the earliest unblocked milestone only, except for explicitly
   independent test/oracle preparation.
2. Set its status to `in-progress` before implementation.
3. Keep scope, invariants, exclusions, and required evidence visible in the
   milestone file.
4. Cite the milestone ID in commits.
5. Mark it complete only after every exit criterion is evidenced and the
   compatibility manifest is updated.

Valid statuses are `planned`, `in-progress`, `blocked`, and `complete`.

## Architecture and sequencing

- [Dossier analysis](analysis.md)
- [Compatibility and release cuts](compatibility.md)
- [Decision record template](../adr/0000-template.md)

The critical path is:

`pins/oracles -> safe wire reader -> builder/codecs -> reflection/codegen -> native compiler`

RPC branches after generated interfaces and the owned-message model exist.
Parallel construction branches after safe allocation, far pointers, and exact
budgeting exist. This deliberately prevents concurrency work from weakening
wire safety or protocol ordering.

## Milestones

### Phase 0 — Target and independent oracles

- [M00 — Project charter and compatibility manifest](milestones/M00-project-charter-and-compatibility-manifest.md) — complete
- [M01 — Reference corpus and benchmark harness](milestones/M01-reference-corpus-and-benchmark-harness.md) — complete
- [M02 — Safety and concurrency model ADRs](milestones/M02-safety-and-concurrency-model-adrs.md) — complete

### Phase 1 — Wire primitives and safe messages

- [M03 — Words, endian, and checked wire integers](milestones/M03-words-endian-and-checked-wire-integers.md) — complete
- [M04 — Segment tables and standard framing](milestones/M04-segment-tables-and-standard-framing.md) — complete
- [M05 — Pointer validation](milestones/M05-pointer-validation.md) — complete
- [M06 — Exact traversal and nesting limits](milestones/M06-exact-traversal-and-nesting-limits.md) — complete
- [M07 — Primitive, enum, text, and data readers](milestones/M07-primitive-enum-text-and-data-readers.md) — complete
- [M08 — Struct readers and evolution semantics](milestones/M08-struct-readers-and-evolution-semantics.md) — complete
- [M09 — List readers and upgrade semantics](milestones/M09-list-readers-and-upgrade-semantics.md) — complete
- [M10 — Owned shared messages and stable object references](milestones/M10-owned-shared-messages-and-stable-object-references.md) — complete

### Phase 2 — Construction, copying, canonicalization, and I/O

- [M11 — Exclusive builder arena](milestones/M11-exclusive-builder-arena.md) — complete
- [M12 — Multi-segment allocation and far-pointer writing](milestones/M12-multi-segment-allocation-and-far-pointer-writing.md) — complete
- [M13 — Deep copy, clear, orphan/disown/adopt](milestones/M13-deep-copy-clear-orphan-disown-adopt.md) — complete
- [M14 — Canonicalization and canonical checker](milestones/M14-canonicalization-and-canonical-checker.md) — complete
- [M15 — Packed codec](milestones/M15-packed-codec.md) — complete
- [M16 — Sync, async, mmap, and no-allocation adapters](milestones/M16-io-and-storage-adapters.md) — complete

### Phase 3 — Reflection and generated Rust APIs

- [M17 — Compiled schema model and introspection](milestones/M17-compiled-schema-model-and-introspection.md)
- [M18 — Dynamic value, struct, and list API](milestones/M18-dynamic-value-struct-and-list-api.md)
- [M19 — Codegen: structs, enums, unions, lists](milestones/M19-codegen-core-data-apis.md)
- [M20 — Codegen: generics, brands, constants, annotations](milestones/M20-codegen-generics-brands-constants-annotations.md)
- [M21 — Codegen: interfaces and pipelines](milestones/M21-codegen-interfaces-and-pipelines.md)

### Phase 4 — Native compiler and developer tools

- [M22 — Lexer and lossless syntax tree](milestones/M22-lexer-and-lossless-syntax-tree.md)
- [M23 — Names, imports, IDs, constants, type resolution](milestones/M23-semantic-resolution.md)
- [M24 — Struct layout, unions/groups, evolution](milestones/M24-layout-and-evolution-compiler.md)
- [M25 — Native request and self-hosted generation](milestones/M25-native-request-and-self-hosted-generation.md)
- [M26 — compile and id CLI](milestones/M26-compile-and-id-cli.md)
- [M27 — Text decode, encode, and eval](milestones/M27-text-tools.md)
- [M28 — C++-parity JSON codec](milestones/M28-json-codec.md)

### Phase 5 — Parallel data processing

- [M29 — Parallel read API and subtree planner](milestones/M29-parallel-read-api.md) — complete
- [M30 — Partitioned parallel builder](milestones/M30-partitioned-parallel-builder.md) — complete
- [M31 — Batch codec and pipeline scheduling](milestones/M31-batch-codec-and-pipeline-scheduling.md) — complete

### Phase 6 — Thread-safe two-party RPC Level 1

- [M32 — RPC schema binding and transport envelope](milestones/M32-rpc-schema-binding-and-transport-envelope.md) — complete
- [M33 — Connection actor and Level-0 tables](milestones/M33-connection-actor-and-level-0-tables.md) — complete
- [M34 — Capability import/export and lifetime](milestones/M34-capability-import-export-and-lifetime.md) — complete
- [M35 — Promise pipelining and promised answers](milestones/M35-promise-pipelining-and-promised-answers.md) — complete
- [M36 — Promise resolution and E-order](milestones/M36-promise-resolution-and-e-order.md) — complete
- [M37 — Streaming and adaptive flow control](milestones/M37-streaming-and-flow-control.md) — complete
- [M38 — Cancellation, disconnect, and reconnect](milestones/M38-cancellation-disconnect-and-reconnect.md) — complete
- [M39 — Thread-safe server scheduling](milestones/M39-thread-safe-server-scheduling.md) — complete
- [M40 — Level-1 release gate](milestones/M40-level-1-release-gate.md)

### Phase 7 — Maximum RPC and C++ product parity

- [M41 — Mature local capability utilities](milestones/M41-local-capability-utilities.md) — implementation candidate; activation awaits M40
- [M42 — Revocation and membranes](milestones/M42-revocation-and-membranes.md)
- [M43 — Attached descriptors/resources](milestones/M43-attached-resources.md)
- [M44 — Level-3 introductions and handoffs](milestones/M44-level-3-handoffs.md)
- [M45 — Level-4 Join and distributed equality](milestones/M45-level-4-join.md)
- [M46 — Persistent capabilities and SturdyRefs](milestones/M46-persistent-capabilities.md)
- [M47 — C++ compatibility adapters and examples](milestones/M47-compatibility-adapters-and-examples.md)
- [M48 — Maximum-parity release gate](milestones/M48-maximum-parity-release-gate.md)
