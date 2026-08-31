#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixture_dir="$repo_root/conformance/fixtures/text"

cd -- "$fixture_dir"
sha256sum --check SHA256SUMS
