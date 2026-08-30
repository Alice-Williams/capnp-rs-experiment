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
