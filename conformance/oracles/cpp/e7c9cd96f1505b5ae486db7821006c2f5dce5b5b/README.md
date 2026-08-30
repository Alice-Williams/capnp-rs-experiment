# Pinned C++ oracle

- Repository: <https://github.com/capnproto/capnproto.git>
- Commit: `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`
- Reported version: `Cap'n Proto version 2.0-dev`
- Reference build platform: Debian Trixie, Linux x86-64
- Reference compiler: Debian Clang 19.1.7
- Build script: `tools/build-cpp-oracle.sh`
- Persistent install:
  `/opt/capnp-oracles/capnproto-e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/install`

The source checkout, compiler output, and installation live in a persistent
Docker volume and are deliberately excluded from Git and the container image.
The script verifies the remote URL, refuses to alter a dirty checkout, detaches
at the exact commit, and installs only after a successful CMake/Ninja build.

Clang is the default compiler because the Debian Trixie GCC 14.2 package is
older than the upstream source's documented GCC 14.3 minimum and encounters an
internal compiler error in the C++23 implementation. Set `CXX` explicitly to
test a different suitable compiler without changing the pinned producer source.
