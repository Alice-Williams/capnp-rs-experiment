#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
result_dir=${M40_SOAK_RESULT_DIR:?set M40_SOAK_RESULT_DIR to a new result directory}

cd -- "$repo_root"
if [[ -e "$result_dir" ]]; then
  printf 'M40 result directory already exists: %s\n' "$result_dir" >&2
  exit 2
fi
if [[ -n $(git status --porcelain=v1 --untracked-files=normal) ]]; then
  printf 'M40 release soak requires a clean worktree\n' >&2
  exit 2
fi

minimum_sessions=${M40_SOAK_MIN_SESSIONS:-100000}
duration_seconds=${M40_SOAK_SECONDS:-86400}
max_rss_growth_kib=${M40_SOAK_MAX_RSS_GROWTH_KIB:-65536}
seed=${M40_SOAK_SEED:-7868967635439804779}
case "$minimum_sessions:$duration_seconds:$max_rss_growth_kib:$seed" in
  *[!0-9:]* | :* | *::* | *:) printf 'M40 release soak settings must be unsigned integers\n' >&2; exit 2 ;;
esac
if (( minimum_sessions < 100000 || duration_seconds < 86400 )); then
  printf 'M40 release evidence requires at least 100000 sessions and 86400 seconds\n' >&2
  exit 2
fi

mkdir -p -- "$result_dir"
source_commit=$(git rev-parse HEAD)
source_tree_sha256=$(git ls-tree -r "$source_commit" -- \
  Cargo.toml Cargo.lock crates .github/workflows/ci.yml \
  tools/run-m40-level1-soak.sh tools/run-m40-release-soak.sh | sha256sum | cut -d' ' -f1)
started_at=$(date --utc +%Y-%m-%dT%H:%M:%SZ)
{
  printf 'source_commit=%s\n' "$source_commit"
  printf 'source_tree_sha256=%s\n' "$source_tree_sha256"
  printf 'started_at=%s\n' "$started_at"
  printf 'minimum_sessions=%s\n' "$minimum_sessions"
  printf 'duration_seconds=%s\n' "$duration_seconds"
  printf 'max_rss_growth_kib=%s\n' "$max_rss_growth_kib"
  printf 'seed=%s\n' "$seed"
  printf 'rustc=%s\n' "$(rustc --version)"
  printf 'cargo=%s\n' "$(cargo --version)"
  printf 'kernel=%s\n' "$(uname -srmo)"
} > "$result_dir/metadata.txt"

M40_SOAK_MIN_SESSIONS=$minimum_sessions \
M40_SOAK_SECONDS=$duration_seconds \
M40_SOAK_SEED=$seed \
M40_SOAK_RESULT_FILE="$result_dir/soak.txt" \
  bash tools/run-m40-level1-soak.sh 2>&1 | tee "$result_dir/console.txt"

printf 'completed_at=%s\n' "$(date --utc +%Y-%m-%dT%H:%M:%SZ)" >> "$result_dir/metadata.txt"
bash tools/verify-m40-soak-result.sh "$result_dir"
printf 'M40 release soak evidence recorded in %s\n' "$result_dir"
