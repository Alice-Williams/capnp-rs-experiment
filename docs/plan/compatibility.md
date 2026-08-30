# Compatibility and release cuts

## Authority order

1. Cap'n Proto C++ commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`:
   implementation, tests, compiler, schemas, compatibility libraries, samples.
2. Normative encoding, schema-language, and RPC documents plus the pinned
   schema files.
3. `capnproto-rust` commit `2228b71e55cee819c30450bb9bfd9c1f6a722429`
   as a secondary regression and Rust ergonomics oracle.

## Release cuts

| Release | Milestones | Result |
|---|---:|---|
| Wire preview | M00–M10 | Secure shared zero-copy readers |
| Serialization alpha | M11–M16 | Build, copy, canonicalize, pack, and I/O |
| Generated-data beta | M17–M21 | Reference-request-fed typed Rust APIs |
| Native toolchain beta | M22–M28 | Self-hosted compiler and developer tools |
| Parallel data beta | M29–M31 | Measured shared read/build/batch scaling |
| RPC alpha | M32–M36 | Thread-safe Level-1 core and E-order |
| RPC beta | M37–M39 | Flow control, lifecycle, scheduling |
| v1 | M40 | Hardened two-party Level-1 interop release |
| Capability parity | M41–M43 | Mature local capability facilities |
| Maximum RPC | M44–M46 | Level 3/4 and persistent capabilities |
| Full platform | M47–M48 | Adapters, examples, maximum parity gate |

## Performance gates

- At least 3.0x throughput from one to four physical cores for qualifying
  immutable traversal and CPU-bound same-connection handlers.
- At least 2.5x for qualifying partitioned construction workloads.
- No unexplained single-thread hot-path regression over 5% from the prior
  accepted baseline.
- Report break-even sizes and p99 latency; tiny inputs are not expected to
  benefit from parallel scheduling.

These are release gates measured on the same commit and hardware, not portable
absolute throughput promises.
