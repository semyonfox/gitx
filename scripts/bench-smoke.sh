#!/usr/bin/env bash
# Benchmark GitX against the commands it delegates to or wraps.
#
# Every mutable cache lives below BENCH_ROOT. A benchmark-created root moves to
# the desktop trash on exit when gio is available. Existing npm, Bun, pnpm,
# APT, and GitX caches are never cleared or reused.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
GITX_BIN="${GITX_BIN:-$ROOT_DIR/target/release/gitx}"
PACKAGE="${PACKAGE:-cowsay@1.6.0}"
PACKAGE_BIN="${PACKAGE_BIN:-cowsay}"
PACKAGE_RUN_ARGS="${PACKAGE_RUN_ARGS:-gitx benchmark}"
LOCAL_QUERY="${LOCAL_QUERY:-ripgrep}"
REGISTRY_QUERY="${REGISTRY_QUERY:-cowsay}"
FORGE_QUERY="${FORGE_QUERY:-gitx}"
LIMIT="${LIMIT:-5}"
REPEATS="${REPEATS:-3}"
PUBLIC_REGISTRY="https://registry.npmjs.org"

if [[ ! "$REPEATS" =~ ^[1-9][0-9]*$ ]]; then
  printf 'REPEATS must be a positive whole number.\n' >&2
  exit 2
fi

if [[ -n "${BENCH_ROOT:-}" ]]; then
  BENCH_ROOT_CREATED=0
else
  BENCH_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/gitx-bench.XXXXXX")"
  BENCH_ROOT_CREATED=1
fi

if ! mkdir -p -- "$BENCH_ROOT"; then
  printf 'Could not create benchmark root: %s\n' "$BENCH_ROOT" >&2
  exit 1
fi
BENCH_ROOT="$(cd "$BENCH_ROOT" && pwd -P)"

if [[ "$BENCH_ROOT" == "/" || "$BENCH_ROOT" == "$ROOT_DIR" ]]; then
  printf 'Refusing unsafe benchmark root: %s\n' "$BENCH_ROOT" >&2
  exit 2
fi

dispose_benchmark_root() {
  if [[ "$BENCH_ROOT_CREATED" != "1" || "${KEEP_BENCH_DIR:-0}" == "1" ]]; then
    printf 'Benchmark files retained at %s\n' "$BENCH_ROOT"
    return
  fi

  if command -v gio >/dev/null 2>&1; then
    gio trash -- "$BENCH_ROOT"
    printf 'Benchmark files moved to the desktop trash.\n'
  else
    printf 'gio is unavailable, so benchmark files remain at %s\n' "$BENCH_ROOT" >&2
  fi
}
trap dispose_benchmark_root EXIT

mkdir -p "$BENCH_ROOT/cases"
RESULTS_TSV="$BENCH_ROOT/timings.tsv"
ENVIRONMENT_TXT="$BENCH_ROOT/environment.txt"
printf 'id\tphase\titeration\tcategory\tstatus\treal_seconds\tuser_seconds\tsystem_seconds\tmax_rss_kib\tcache_bytes\tlabel\n' > "$RESULTS_TSV"

have() {
  command -v "$1" >/dev/null 2>&1
}

cache_size() {
  local cache_dir="$1"
  if [[ -d "$cache_dir" ]]; then
    du -sb -- "$cache_dir" 2>/dev/null | awk '{print $1}'
  else
    printf '0\n'
  fi
}

record_skip() {
  printf 'SKIP  %s\n' "$*" | tee -a "$ENVIRONMENT_TXT"
}

measure() {
  local id="$1"
  local phase="$2"
  local iteration="$3"
  local category="$4"
  local label="$5"
  local case_dir="$6"
  local cache_dir="$7"
  shift 7

  mkdir -p "$case_dir"
  local timing_file="$case_dir/$phase.time"
  local stdout_file="$case_dir/$phase.stdout"
  local stderr_file="$case_dir/$phase.stderr"
  local status=0
  local -a command=("$@")

  if declare -F "$1" >/dev/null 2>&1; then
    local function_name="$1"
    shift
    export -f "$function_name"
    export GITX_BIN PACKAGE PACKAGE_BIN PACKAGE_RUN_ARGS PUBLIC_REGISTRY
    command=(bash -c "$function_name \"\$@\"" "$function_name" "$@")
  fi

  /usr/bin/time -f '%e\t%U\t%S\t%M' -o "$timing_file" "${command[@]}" > "$stdout_file" 2> "$stderr_file" || status=$?

  local real_seconds='NA'
  local user_seconds='NA'
  local system_seconds='NA'
  local max_rss_kib='NA'
  local command_status="$status"
  if [[ -s "$timing_file" ]]; then
    IFS=$'\t' read -r real_seconds user_seconds system_seconds max_rss_kib < <(tail -n 1 "$timing_file") || true
  fi

  local bytes
  bytes="$(cache_size "$cache_dir")"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$id" "$phase" "$iteration" "$category" "$command_status" \
    "$real_seconds" "$user_seconds" "$system_seconds" "$max_rss_kib" "$bytes" "$label" \
    >> "$RESULTS_TSV"
  printf '%-26s %-6s sample %s: %ss (status %s, cache %s bytes)\n' \
    "$id" "$phase" "$iteration" "$real_seconds" "$command_status" "$bytes"
}

run_repeats() {
  local id="$1"
  local category="$2"
  local label="$3"
  shift 3

  local iteration
  for ((iteration = 1; iteration <= REPEATS; iteration += 1)); do
    local case_dir="$BENCH_ROOT/cases/$id/$iteration"
    measure "$id" repeat "$iteration" "$category" "$label" "$case_dir" "$case_dir/cache" "$@"
  done
}

run_package_pairs() {
  local id="$1"
  local label="$2"
  local runner="$3"
  local iteration

  for ((iteration = 1; iteration <= REPEATS; iteration += 1)); do
    local case_dir="$BENCH_ROOT/cases/$id/$iteration"
    local cache_dir="$case_dir/cache"
    mkdir -p "$cache_dir"
    measure "$id" cold "$iteration" package "$label" "$case_dir" "$cache_dir" "$runner" "$cache_dir"
    measure "$id" warm "$iteration" package "$label" "$case_dir" "$cache_dir" "$runner" "$cache_dir"
  done
}

raw_dpkg_query() {
  local query="$1"
  dpkg-query -W -f='${binary:Package}\t${Version}\t${Status}\t${Description}\n' \
    | awk -F '\t' -v query="$query" 'BEGIN { q = tolower(query) } index(tolower($0), q) { found = 1 } END { exit !found }'
}

raw_apt_cache() {
  local query="$1"
  local -a names=()
  mapfile -t names < <(apt-cache --no-generate search --names-only "$query" | awk 'NR <= 5 { print $1 }')
  if (( ${#names[@]} == 0 )); then
    return 1
  fi
  apt-cache --no-generate show --no-all-versions "${names[@]}" >/dev/null
}

raw_snap_list() {
  local query="$1"
  snap list | awk -v query="$query" 'BEGIN { q = tolower(query) } NR == 1 { next } index(tolower($0), q) { found = 1 } END { exit !found }'
}

raw_cargo_list() {
  local query="$1"
  cargo install --list | awk -v query="$query" 'BEGIN { q = tolower(query) } index(tolower($0), q) { found = 1 } END { exit 0 }'
}

raw_python_list() {
  local query="$1"
  python3 -m pip --disable-pip-version-check list --format=json \
    | jq -e --arg query "$query" '[.[] | select((.name | ascii_downcase | contains($query | ascii_downcase)))] | length > 0' >/dev/null
}

raw_npm_global_list() {
  local query="$1"
  npm ls --global --depth=0 --json --silent \
    | jq -e --arg query "$query" '.dependencies | keys | any(. | ascii_downcase | contains($query | ascii_downcase))' >/dev/null
}

gitx_npm() {
  local cache_dir="$1"
  local -a args=()
  read -r -a args <<< "$PACKAGE_RUN_ARGS"
  env GITX_CACHE_DIR="$cache_dir" npm_config_registry="$PUBLIC_REGISTRY" \
    "$GITX_BIN" exec "$PACKAGE" --backend npm --yes -- "${args[@]}"
}

raw_npm() {
  local cache_dir="$1"
  local -a args=()
  read -r -a args <<< "$PACKAGE_RUN_ARGS"
  env npm_config_cache="$cache_dir/npm-cache" npm_config_registry="$PUBLIC_REGISTRY" \
    npm exec --yes --ignore-scripts --package="$PACKAGE" -- "$PACKAGE_BIN" "${args[@]}"
}

gitx_bun() {
  local cache_dir="$1"
  local -a args=()
  read -r -a args <<< "$PACKAGE_RUN_ARGS"
  env GITX_CACHE_DIR="$cache_dir" BUN_CONFIG_REGISTRY="$PUBLIC_REGISTRY" \
    "$GITX_BIN" exec "$PACKAGE" --backend bun --yes --allow-scripts -- "${args[@]}"
}

raw_bun_shared() {
  local -a args=()
  read -r -a args <<< "$PACKAGE_RUN_ARGS"
  bunx "$PACKAGE" "${args[@]}"
}

gitx_pnpm() {
  local cache_dir="$1"
  local -a args=()
  read -r -a args <<< "$PACKAGE_RUN_ARGS"
  env GITX_CACHE_DIR="$cache_dir" npm_config_registry="$PUBLIC_REGISTRY" \
    "$GITX_BIN" exec "$PACKAGE" --backend pnpm --yes -- "${args[@]}"
}

raw_pnpm() {
  local cache_dir="$1"
  local -a args=()
  read -r -a args <<< "$PACKAGE_RUN_ARGS"
  env pnpm_config_cache_dir="$cache_dir/pnpm-cache" pnpm_config_store_dir="$cache_dir/pnpm-store" npm_config_registry="$PUBLIC_REGISTRY" \
    pnpm dlx "$PACKAGE" "${args[@]}"
}

write_environment() {
  {
    printf 'GitX comparison benchmark\n'
    printf 'timestamp=%s\n' "$(date -Is)"
    printf 'package=%s\n' "$PACKAGE"
    printf 'package_bin=%s\n' "$PACKAGE_BIN"
    printf 'package_args=%s\n' "$PACKAGE_RUN_ARGS"
    printf 'local_query=%s\n' "$LOCAL_QUERY"
    printf 'registry_query=%s\n' "$REGISTRY_QUERY"
    printf 'forge_query=%s\n' "$FORGE_QUERY"
    printf 'repeats=%s\n' "$REPEATS"
    uname -a
    if [[ -r /etc/os-release ]]; then
      . /etc/os-release
      printf 'os=%s %s\n' "$ID" "$VERSION_ID"
    fi
    for tool in gitx npm bun pnpm git curl apt-cache dpkg-query snap cargo python3 jq; do
      if have "$tool"; then
        printf '%s=%s\n' "$tool" "$("$tool" --version 2>/dev/null | head -n 1)"
      else
        printf '%s=missing\n' "$tool"
      fi
    done
  } > "$ENVIRONMENT_TXT"
}

if [[ ! -x "$GITX_BIN" ]]; then
  printf 'Building release GitX binary.\n'
  (cd "$ROOT_DIR" && cargo build --release --locked) || exit 1
fi

write_environment
printf 'Benchmark root: %s\n' "$BENCH_ROOT"
printf 'Raw timing data: %s\n' "$RESULTS_TSV"

printf '\nLocal catalog searches\n'
if have dpkg-query; then
  run_repeats gitx-system local 'GitX installed-system catalog' \
    "$GITX_BIN" search "$LOCAL_QUERY" --source system --limit "$LIMIT" --format json
  run_repeats raw-dpkg local 'raw dpkg-query with equivalent filtering' raw_dpkg_query "$LOCAL_QUERY"
else
  record_skip 'dpkg-query is unavailable'
fi

if have apt-cache; then
  run_repeats gitx-apt local 'GitX APT cache catalog' \
    "$GITX_BIN" search "$LOCAL_QUERY" --source apt --limit "$LIMIT" --format json
  run_repeats raw-apt local 'raw apt-cache search plus package details' raw_apt_cache "$LOCAL_QUERY"
else
  record_skip 'apt-cache is unavailable'
fi

if have snap; then
  run_repeats gitx-snap local 'GitX Snap catalog' \
    "$GITX_BIN" search core --source snap --limit "$LIMIT" --format json
  run_repeats raw-snap local 'raw snap list with filtering' raw_snap_list core
else
  record_skip 'snap is unavailable'
fi

if have cargo; then
  run_repeats gitx-cargo local 'GitX Cargo install catalog' \
    "$GITX_BIN" search cargo --source cargo --limit "$LIMIT" --format json
  run_repeats raw-cargo local 'raw cargo install list with filtering' raw_cargo_list cargo
else
  record_skip 'cargo is unavailable'
fi

if have python3 && have jq; then
  run_repeats gitx-python local 'GitX Python package catalog' \
    "$GITX_BIN" search pip --source python --limit "$LIMIT" --format json
  run_repeats raw-python local 'raw pip JSON list with filtering' raw_python_list pip
else
  record_skip 'python3 or jq is unavailable'
fi

if have npm && have jq; then
  run_repeats gitx-npm-global local 'GitX global npm catalog' \
    "$GITX_BIN" search codex --source npm-global --limit "$LIMIT" --format json
  run_repeats raw-npm-global local 'raw npm global JSON list with filtering' raw_npm_global_list codex
else
  record_skip 'npm or jq is unavailable'
fi

printf '\nRemote discovery searches\n'
run_repeats gitx-npm-search remote 'GitX npm registry search' \
  env NPM_CONFIG_REGISTRY="$PUBLIC_REGISTRY" "$GITX_BIN" search "$REGISTRY_QUERY" --source npm --limit "$LIMIT" --format json
run_repeats npm-search remote 'npm CLI registry search' \
  env npm_config_registry="$PUBLIC_REGISTRY" npm search "$REGISTRY_QUERY" --searchlimit="$LIMIT" --json
run_repeats raw-npm-search remote 'raw npm registry search endpoint' \
  curl --fail --silent --show-error --connect-timeout 10 --max-time 15 \
    --user-agent 'gitx-benchmark/0.2.0' \
    "$PUBLIC_REGISTRY/-/v1/search?text=$REGISTRY_QUERY&size=$LIMIT"

run_repeats gitx-bun-search remote 'GitX Bun-selected registry search' \
  env BUN_CONFIG_REGISTRY="$PUBLIC_REGISTRY" NPM_CONFIG_REGISTRY="$PUBLIC_REGISTRY" \
    "$GITX_BIN" search "$REGISTRY_QUERY" --source bun --limit "$LIMIT" --format json
run_repeats raw-bun-registry remote 'raw registry endpoint used by the Bun source' \
  curl --fail --silent --show-error --connect-timeout 10 --max-time 15 \
    --user-agent 'gitx-benchmark/0.2.0' \
    "$PUBLIC_REGISTRY/-/v1/search?text=$REGISTRY_QUERY&size=$LIMIT"

run_repeats gitx-github-search remote 'GitX GitHub repository search' \
  env -u GITHUB_TOKEN "$GITX_BIN" search "$FORGE_QUERY" --source github --limit "$LIMIT" --format json
run_repeats raw-github-search remote 'raw GitHub repository search endpoint' \
  curl --fail --silent --show-error --connect-timeout 10 --max-time 15 \
    --user-agent 'gitx-benchmark/0.2.0' -H 'Accept: application/vnd.github+json' \
    "https://api.github.com/search/repositories?q=$FORGE_QUERY&per_page=$LIMIT"

run_repeats gitx-gitlab-search remote 'GitX GitLab project search' \
  env -u GITLAB_TOKEN "$GITX_BIN" search "$FORGE_QUERY" --source gitlab --limit "$LIMIT" --format json
run_repeats raw-gitlab-search remote 'raw GitLab project search endpoint' \
  curl --fail --silent --show-error --connect-timeout 10 --max-time 15 \
    --user-agent 'gitx-benchmark/0.2.0' \
    "https://gitlab.com/api/v4/projects?search=$FORGE_QUERY&simple=true&per_page=$LIMIT&order_by=star_count&sort=desc"

run_repeats gitx-codeberg-search remote 'GitX Codeberg repository search' \
  env -u CODEBERG_TOKEN "$GITX_BIN" search "$FORGE_QUERY" --source codeberg --limit "$LIMIT" --format json
run_repeats raw-codeberg-search remote 'raw Codeberg repository search endpoint' \
  curl --fail --silent --show-error --connect-timeout 10 --max-time 15 \
    --user-agent 'gitx-benchmark/0.2.0' \
    "https://codeberg.org/api/v1/repos/search?q=$FORGE_QUERY&limit=$LIMIT"

printf '\nIsolated package execution\n'
printf 'npm and pnpm use a fresh cache for every cold sample. Bun may satisfy an exact package from its shared cache before writing the requested cache directory, so its native row is a warm-path measurement.\n'
if have npm; then
  run_package_pairs gitx-npm 'GitX preflight plus npm exec' gitx_npm
  run_package_pairs raw-npm 'npm exec with scripts ignored' raw_npm
else
  record_skip 'npm is unavailable'
fi

if have bunx; then
  run_package_pairs gitx-bun 'GitX preflight plus bunx, GitX cache cold or warm' gitx_bun
  run_repeats raw-bun-shared package 'bunx using its normal shared cache, warm path only' raw_bun_shared
else
  record_skip 'bunx is unavailable'
fi

if have pnpm; then
  run_package_pairs gitx-pnpm 'GitX preflight plus pnpm dlx with an isolated cache' gitx_pnpm
  run_package_pairs raw-pnpm 'pnpm dlx with an isolated cache' raw_pnpm
else
  record_skip 'pnpm is unavailable'
fi

printf '\nSummary, arithmetic mean by case and phase\n'
awk -F '\t' '
  NR > 1 && $5 == 0 {
    key = $1 FS $2
    count[key] += 1
    sum[key] += $6
    last_cache[key] = $10
    label[key] = $11
  }
  END {
    printf "id\tphase\tsamples\tmean_real_seconds\tlast_cache_bytes\tlabel\n"
    for (key in count) {
      split(key, part, FS)
      printf "%s\t%s\t%d\t%.3f\t%s\t%s\n", part[1], part[2], count[key], sum[key] / count[key], last_cache[key], label[key]
    }
  }
' "$RESULTS_TSV" | sort

printf '\nFull environment: %s\n' "$ENVIRONMENT_TXT"
printf 'Full timing data: %s\n' "$RESULTS_TSV"
