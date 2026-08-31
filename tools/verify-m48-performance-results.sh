#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_root=${1:-"$repo_root/benchmarks/results/2026-08-31-m48-g-drive-docker"}

bash "$repo_root/benchmarks/verify-m29-parallel-results.sh" "$result_root/m29"
bash "$repo_root/benchmarks/verify-m30-parallel-results.sh" "$result_root/m30"
bash "$repo_root/benchmarks/verify-m31-batch-results.sh" "$result_root/m31"
bash "$repo_root/benchmarks/verify-m39-server-scheduling-results.sh" "$result_root/m39"

printf 'M48 release-commit parallel and scheduling performance results OK\n'
