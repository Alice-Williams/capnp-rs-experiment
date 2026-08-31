#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"

python3 - <<'PY'
from pathlib import Path
import tomllib

manifest = tomllib.loads(Path("compatibility/manifest.toml").read_text())
milestones = manifest["milestones"]
for number in range(0, 40):
    name = f"M{number:02d}"
    if milestones.get(name) != "complete":
        raise SystemExit(f"{name} is not recorded complete")
if "M40" in milestones:
    raise SystemExit("M40 must not be recorded complete before its 24-hour soak")
for number in range(41, 48):
    name = f"M{number:02d}"
    state = milestones.get(name, "")
    if not state.startswith("implementation candidate"):
        raise SystemExit(f"{name} is not recorded as an implementation candidate")
    if name not in manifest.get("evidence", {}):
        raise SystemExit(f"{name} evidence is missing")
PY

audit=docs/release/maximum-parity-audit.md
for phrase in \
  '86,400-second' \
  'multi-day full-platform fault/soak' \
  'nested-interface declaration compiler gap' \
  'C++ facilities that are Rust-inapplicable' \
  'Performance artifacts' \
  'final security gate'
do
  grep -Fq "$phrase" "$audit"
done

if git grep -n -E 'unsafe[[:space:]]*\{' -- '*.rs'; then
  printf 'M48 inventory failed: unsafe block found\n' >&2
  exit 1
fi

printf 'M48 inventory: implemented, candidate, inapplicable, and blocked surfaces are explicit\n'
