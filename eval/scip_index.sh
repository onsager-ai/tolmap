#!/usr/bin/env bash
# One SCIP index job for issue #110's P0 spike. Runs on a GitHub-hosted
# runner only (.github/workflows/scip-spike.yml): indexers are type checkers
# and the maintainer's laptop cannot take the load (CLAUDE.md).
#
#   eval/scip_index.sh <lang> <variant> <root> <runs> <clone> <out>
#
# lang     py | ts | go
# variant  default (no dependency install) | install (the lockfile's own
#          install first; throwaway runner only -- it executes code from the
#          indexed repository's dependency graph, which is exactly why the
#          hosted worker must not do it without a sandbox, issue #110 risk 1)
# root     directory inside <clone> the indexer runs in
# runs     how many times to index (2 = byte-stability check)
#
# Writes <out>/index.run<N>.scip, run<N>.time.txt (/usr/bin/time -v),
# run<N>.log, install.time.txt/install.log when installing, and meta.json.
# An indexer failure is recorded, not raised: a failed index is a data point.
set -uo pipefail

lang=$1
variant=$2
root=$3
runs=$4
clone=$(cd "$5" && pwd)
mkdir -p "$6"
out=$(cd "$6" && pwd)
work="$clone/$root"
name=$(basename "$clone")

cd "$work" || exit 1

install_rc=""
if [ "$variant" = "install" ]; then
  case "$lang" in
    ts)
      # --ignore-scripts: dependency types come from the packages
      # themselves; lifecycle scripts only add arbitrary code execution.
      /usr/bin/time -v -o "$out/install.time.txt" \
        pnpm install --frozen-lockfile --ignore-scripts > "$out/install.log" 2>&1
      install_rc=$?
      ;;
    py)
      # dify's api/ is a uv project (uv.lock). scip-python asks pip for the
      # environment's packages, and a uv venv has no pip, so seed one.
      /usr/bin/time -v -o "$out/install.time.txt" \
        bash -c 'uv sync --frozen && uv pip install --python .venv/bin/python pip' \
        > "$out/install.log" 2>&1
      install_rc=$?
      ;;
    go)
      /usr/bin/time -v -o "$out/install.time.txt" \
        go mod download > "$out/install.log" 2>&1
      install_rc=$?
      ;;
  esac
fi

index_cmd() {
  local output=$1
  case "$lang" in
    ts)
      if [ -f pnpm-workspace.yaml ]; then
        scip-typescript index --pnpm-workspaces --infer-tsconfig --no-progress-bar --output "$output"
      else
        scip-typescript index --infer-tsconfig --no-progress-bar --output "$output"
      fi
      ;;
    py)
      if [ "$variant" = "install" ] && [ -x .venv/bin/python ]; then
        # shellcheck disable=SC1091
        source .venv/bin/activate
      fi
      scip-python index --project-name "$name" --quiet --output "$output"
      ;;
    go)
      scip-go --quiet --output "$output"
      ;;
  esac
}

export lang variant name
declare -a codes=()
declare -a hashes=()
for run in $(seq 1 "$runs"); do
  target="$out/index.run${run}.scip"
  # /usr/bin/time needs an executable, not a shell function: re-declare the
  # function inside a child bash so the timing covers the indexer and
  # everything it spawns (ru_maxrss is the largest single process).
  /usr/bin/time -v -o "$out/run${run}.time.txt" \
    bash -c "$(declare -f index_cmd); index_cmd \"\$0\"" "$target" \
    > "$out/run${run}.log" 2>&1
  rc=$?
  codes+=("$rc")
  if [ -f "$target" ]; then
    hashes+=("\"$(sha256sum "$target" | cut -d' ' -f1)\"")
  else
    hashes+=("null")
  fi
  tail -n 40 "$out/run${run}.log"
done

codes_json=$(IFS=,; echo "${codes[*]}")
hashes_json=$(IFS=,; echo "${hashes[*]}")
cat > "$out/meta.json" <<EOF
{"lang": "$lang", "variant": "$variant", "root": "$root", "runs": $runs,
 "install_exit": ${install_rc:-null}, "exit_codes": [$codes_json], "sha256": [$hashes_json]}
EOF
cat "$out/meta.json"
