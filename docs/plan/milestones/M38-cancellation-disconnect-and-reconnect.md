# M38 — Cancellation, clean disconnect, and reconnect

- Status: complete
- Phase: 6
- Depends on: M37

## Outcome

Define cancellation/opt-out, transport-complete shutdown, disconnect propagation, and capability-recreation reconnect helpers.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Every lifecycle race is simulated; errors surface; no waiter hangs; reconnect never reuses connection IDs and distinguishes overload/disconnect.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
