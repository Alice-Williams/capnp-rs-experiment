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

evidence = manifest.get("evidence", {})
m40_state = milestones.get("M40")
m48_state = milestones.get("M48")
if m40_state is None:
    if m48_state is not None:
        raise SystemExit("M48 cannot be recorded before M40 activation")
    for number in range(41, 48):
        name = f"M{number:02d}"
        state = milestones.get(name, "")
        if not state.startswith("implementation candidate"):
            raise SystemExit(f"{name} is not recorded as an implementation candidate")
        if name not in evidence:
            raise SystemExit(f"{name} evidence is missing")
    print("M48 inventory phase: implementation candidates await M40 activation")
elif m40_state == "complete":
    for number in range(41, 48):
        name = f"M{number:02d}"
        if milestones.get(name) != "complete":
            raise SystemExit(f"{name} must activate atomically after M40")
        if name not in evidence:
            raise SystemExit(f"{name} evidence is missing")
    if m48_state not in (None, "complete"):
        raise SystemExit("M48 must be absent while in progress or recorded complete")
    if m48_state == "complete" and "M48" not in evidence:
        raise SystemExit("M48 completion evidence is missing")
    phase = "maximum-parity release complete" if m48_state == "complete" else "M40-M47 activated; M48 release pending"
    print(f"M48 inventory phase: {phase}")
else:
    raise SystemExit("M40 must be absent before its soak or recorded complete after it passes")
PY

audit=docs/release/maximum-parity-audit.md
for phrase in \
  '86,400-second' \
  'multi-day full-platform fault/soak' \
  'former nested-interface declaration compiler blocker is resolved' \
  'C++ facilities that are Rust-inapplicable' \
  'performance blocker is resolved' \
  'final security gate'
do
  grep -Fq "$phrase" "$audit"
done

if git grep -n -E 'unsafe[[:space:]]*\{' -- '*.rs'; then
  printf 'M48 inventory failed: unsafe block found\n' >&2
  exit 1
fi

printf 'M48 inventory: implemented, candidate, inapplicable, and blocked surfaces are explicit\n'
