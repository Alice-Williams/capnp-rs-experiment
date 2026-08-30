# M12 — Multi-segment allocation and far-pointer writing

- Status: planned
- Phase: 2
- Depends on: M11

## Outcome

Grow builders across segments and emit correct single/double-far landing pads.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Forced tiny segments exercise every pad case; reference readers accept output; checked limits prevent offset overflow; layouts are deterministic.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

