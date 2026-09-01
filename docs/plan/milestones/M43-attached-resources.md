# M43 — Attached file descriptors and resources

- Status: complete
- Phase: 7
- Depends on: M40

## Outcome

Provide Unix SCM_RIGHTS and a generic attached-resource ownership model.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Each FD has one owner; duplicates/excess/unsupported transports close safely; quotas and fallback behavior are demonstrated.

Evidence is recorded in
[`docs/rpc/attached-resources.md`](../../rpc/attached-resources.md) and
`tools/verify-m43-attached-resources.sh`. The exact three-case pinned C++
SCM_RIGHTS corpus passes beside native wire, ownership, actor, quota, fallback,
and real Unix-domain-socket tests.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

Three-party handoff, distributed equality, persistence, Windows handle
transfer, TCP attachment transfer, and implicit local-membrane FD passthrough
remain outside this milestone.
