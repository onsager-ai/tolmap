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
      # Every tracked tsconfig.json is a project, deepest first. The ingest
      # keeps the first document it sees for a path, so a file covered by
      # both a package's tsconfig and a root one is read under its nearest
      # config, which is the one its own compiler invocation uses.
      #
      # Not `--pnpm-workspaces --infer-tsconfig`: the first spike run
      # (35960017915) used that, and inferring a tsconfig for every
      # workspace package without one replaced vue's root tsconfig (the one
      # carrying the `@vue/*` paths) for all of its sources: 6 of 259
      # cross-package edges survived. A project whose tsconfig cannot load
      # (an `extends` into an uninstalled package, TS6053) is dropped by
      # scip-typescript; the ingest reports those files as not indexed.
      mapfile -t projects < <(git ls-files -- 'tsconfig.json' '*/tsconfig.json' \
        | grep -v node_modules \
        | awk -F/ '{ d = NF > 1 ? substr($0, 1, length($0) - length($NF) - 1) : "."; print NF "\t" d }' \
        | sort -t$'\t' -k1,1nr -k2,2 | cut -f2)
      printf 'projects:\n'; printf '  %s\n' "${projects[@]}"
      scip-typescript index --no-progress-bar --output "$output" "${projects[@]}"
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
