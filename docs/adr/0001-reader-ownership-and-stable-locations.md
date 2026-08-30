# ADR-0001: Reader ownership and stable wire locations

- Status: accepted
- Milestone: M02
- Date: 2026-08-30

## Context

Borrowed generated readers are efficient but awkward to retain across threads.
Self-referential owned readers are difficult to express safely, while cached
native pointers become invalid when backing storage moves and obscure bounds
validation.

## Decision

Readers store validated coordinates, never long-lived native pointers:

- `WireLocation` is a segment identifier plus a word offset.
- `ReaderContext` owns immutable segment descriptors and a traversal budget.
- `BorrowedMessage<'a>` refers to caller-owned byte slices or mapped storage.
- `OwnedMessage` owns immutable segment backing through `Arc`.
- `ObjectRef<T>` owns an `Arc<OwnedMessage>`, a validated `WireLocation`, and a
  type marker. Calling `read()` creates a short-lived reader.

Constructing an object reference validates the pointer's wire kind. Reading it
again still checks the size/version required by the requested generated view.
No reader may cache an unchecked pointer into a segment.

## Alternatives considered

- Self-referential owned readers: rejected because moving/borrowing them safely
  would require pervasive pinning or unsafe code.
- Copying each retained subobject: rejected because it defeats zero-copy shared
  traversal and changes capability/graph identity semantics.
- Stable raw pointers into pinned buffers: rejected because pointer provenance,
  segment bounds, and cross-segment locations become implicit.

## Consequences

Generated borrowed readers stay small. Owned subobjects can be stored and sent
between threads without extending a borrow. Dereferencing performs inexpensive
coordinate lookup and necessary validation instead of relying on pointer
identity.

## Enforcement

M02 compile prototypes assert that `OwnedMessage` and `ObjectRef<T>` are
`Send + Sync` through representation. M03–M10 add checked-coordinate unit and
compile-fail tests; M10 adds concurrent traversal and retained-subobject tests.
