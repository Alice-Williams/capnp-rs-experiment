#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
m48_soak_results=${M48_SOAK_RESULT_DIR:?set M48_SOAK_RESULT_DIR to the recorded M48 result directory}
cd -- "$repo_root"

bash tools/verify-m48-security-gates.sh
bash tools/verify-m40-soak-result.sh
bash tools/verify-m48-soak-result.sh "$m48_soak_results"

python3 - <<'PY'
from pathlib import Path
import tomllib

milestones = tomllib.loads(Path("compatibility/manifest.toml").read_text())["milestones"]
for number in range(49):
    name = f"M{number:02d}"
    if milestones.get(name) != "complete":
        raise SystemExit(f"release activation is incomplete: {name}")
PY

printf 'M48 maximum-parity release gates OK\n'
