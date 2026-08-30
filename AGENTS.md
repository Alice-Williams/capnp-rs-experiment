# Development environment

- Use the Linux Dev Container defined by `.devcontainer/devcontainer.json` for
  development and tests on non-Linux hosts, including Windows. Do not run
  `cargo`, `rustc`, `bazel`, or `bazelisk` directly on the host there.
- The repository stays on the host filesystem and is bind-mounted at
  `/workspace`; editing files locally is expected.
- Inside the container, use `cargo build`, `cargo test`, `bazelisk`, and
  `bazel` as needed. The container runs as root to keep this personal,
  isolated environment simple and disposable.
- GitHub credentials remain host-side. This container deliberately has no SSH
  key mounts and is not the GitHub push environment; use normal host Git
  operations for commits and pushes.
- Do not commit generated output such as `target/`, Bazel output directories,
  caches, or Docker image archives.

# Engineering workflow

- The milestone contract and dependency order live in `docs/plan/README.md`.
  Read the active milestone file before changing implementation code.
- Cite the milestone ID in commits and pull requests. Do not silently absorb
  work assigned to later milestones.
- Trace product behavior first to the pinned C++ implementation and its tests.
  Wire and protocol behavior also require a pinned specification/schema source
  or an explicit compatibility ADR.
- Do not add a public `unsafe impl Send` or `unsafe impl Sync`. Public types
  must receive those auto traits from their representation; exceptions require
  a reviewed ADR and compile tests.
- Never hold a lock or `RefCell` borrow across `.await`. RPC protocol state may
  only be mutated by its owning actor/state-machine function.
- Generated code delegates protocol behavior to tested runtime primitives.
- Use checked arithmetic at every parser/decoder boundary and enforce
  configured resource limits before allocation or slicing.
- New public APIs need positive compile tests plus relevant misuse/alias
  compile-fail tests. Fuzz regressions become permanent minimal tests.
- Performance claims require checked-in benchmarks and recorded context.
- Update `compatibility/manifest.toml` as milestone support changes.
