#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${M48_SOAK_RESULT_DIR:?set M48_SOAK_RESULT_DIR to a new result directory}

cd -- "$repo_root"
if [[ -e "$result_dir" ]]; then
  printf 'M48 result directory already exists: %s\n' "$result_dir" >&2
  exit 2
fi
if [[ -n $(git status --porcelain=v1 --untracked-files=normal) ]]; then
  printf 'M48 release soak requires a clean worktree\n' >&2
  exit 2
fi

minimum_sessions=${M48_SOAK_MIN_SESSIONS:-100000}
duration_seconds=${M48_SOAK_SECONDS:-172800}
warmup_sessions=${M48_SOAK_WARMUP_SESSIONS:-100}
max_rss_growth_kib=${M48_SOAK_MAX_RSS_GROWTH_KIB:-65536}
seed=${M48_SOAK_SEED:-7868976431532826987}
if (( minimum_sessions < 100000 || duration_seconds < 172800 )); then
  printf 'M48 release evidence requires at least 100000 sessions and 172800 seconds\n' >&2
  exit 2
fi

mkdir -p -- "$result_dir"
source_commit=$(git rev-parse HEAD)
source_tree_sha256=$(git ls-tree -r "$source_commit" -- \
  Cargo.toml Cargo.lock crates .github/workflows/ci.yml \
  tools/run-m48-full-platform-soak.sh tools/run-m48-release-soak.sh | sha256sum | cut -d' ' -f1)
started_at=$(date --utc +%Y-%m-%dT%H:%M:%SZ)
{
  printf 'source_commit=%s\n' "$source_commit"
  printf 'source_tree_sha256=%s\n' "$source_tree_sha256"
  printf 'started_at=%s\n' "$started_at"
  printf 'minimum_sessions=%s\n' "$minimum_sessions"
  printf 'duration_seconds=%s\n' "$duration_seconds"
  printf 'warmup_sessions=%s\n' "$warmup_sessions"
  printf 'max_rss_growth_kib=%s\n' "$max_rss_growth_kib"
  printf 'seed=%s\n' "$seed"
  printf 'rustc=%s\n' "$(rustc --version)"
  printf 'cargo=%s\n' "$(cargo --version)"
  printf 'kernel=%s\n' "$(uname -srmo)"
} > "$result_dir/metadata.txt"

M48_SOAK_MIN_SESSIONS=$minimum_sessions \
M48_SOAK_SECONDS=$duration_seconds \
M48_SOAK_WARMUP_SESSIONS=$warmup_sessions \
M48_SOAK_MAX_RSS_GROWTH_KIB=$max_rss_growth_kib \
M48_SOAK_SEED=$seed \
M48_SOAK_RESULT_FILE="$result_dir/soak.txt" \
  bash tools/run-m48-full-platform-soak.sh 2>&1 | tee "$result_dir/console.txt"

printf 'completed_at=%s\n' "$(date --utc +%Y-%m-%dT%H:%M:%SZ)" >> "$result_dir/metadata.txt"
bash tools/verify-m48-soak-result.sh "$result_dir"
printf 'M48 release soak evidence recorded in %s\n' "$result_dir"
