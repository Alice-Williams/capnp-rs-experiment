# M37 — Streaming, adaptive flow control, and bounded backpressure

- Status: complete
- Phase: 6
- Depends on: M36

## Outcome

Add streaming semantics, fixed/adaptive windows, RTT/ack tracking, incoming flow limits, and quotas.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Per-stream order and cross-stream concurrency hold; slow peers bound memory; blocked senders wake; adaptive behavior matches pinned tests.

## Completion evidence

`FlowController` now separates eager, per-stream serialized send invocation
from acknowledgement-owned `FlowAck` and advisory `FlowReady` futures. Fixed
windows reproduce the extended-window and largest-message rules; adaptive
windows retain send/delivery snapshots, minimum RTT, BDP estimates, startup and
steady-state growth collars, saturation decay, application-limited stability,
and configured clamps. Every message, aggregate in-flight byte, and blocked
waiter limit is checked before the send closure runs. Failure and close wake
blocked futures, while dropping readiness cannot cancel a recorded send.

Generated local streaming calls dispatch before returning, preserving call
order. Separate controllers prove cross-stream independence. The connection
actor also accounts aggregate incoming Call wire bytes transactionally and
releases charges exactly across normal Finish, retained pipeline/tail state,
and disconnect.

`tools/verify-m37-flow-control.sh` builds the exact pinned C++ commit with Clang
and runs all ten upstream fixed/adaptive tests at `rpc-test.c++:2674-3060`, then
runs the native Rust behavioral port. Rust 1.98 and 1.85 workspace/all-target
tests, doctests, strict Clippy, shell syntax, and the complete 28-test Bazel
suite pass in the Linux development container.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
