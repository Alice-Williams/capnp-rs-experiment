#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

for script in "$script_dir"/*.sh; do
    bash -n "$script"
    printf 'syntax-ok  %s\n' "$(basename -- "$script")"
done
