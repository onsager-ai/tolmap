#!/usr/bin/env bash
set -euo pipefail
artifact_dir=$(realpath "${1:?artifact directory}")
repo=$(realpath "${2:?repository checkout}")
sha=${3:?source SHA}
node_version=${4:?Node version}
consumer_uid=$(id -u)
[[ "$consumer_uid" -ne 0 ]]
[[ "$sha" =~ ^[0-9a-f]{40}$ ]]
[[ "$(git -C "$repo" rev-parse HEAD)" == "$sha" ]]
out=$(mktemp -d)
nonce=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
capture_diagnostics() {
  if [[ -f "$out/consumer-diagnostics.json" ]]; then
    cp "$out/consumer-diagnostics.json" "$artifact_dir/consumer-diagnostics-$node_version.json"
    cat "$out/consumer-diagnostics.json" >&2
  fi
  rm -rf -- "$out"
}
trap capture_diagnostics EXIT
docker run --rm --init \
  --volume "$artifact_dir:/artifacts:ro" --volume "$repo:/repo:ro" --volume "$out:/out" \
  --env EXPECTED_SHA="$sha" --env CONSUMER_NONCE="$nonce" --env NODE_VERSION="$node_version" --env CONSUMER_UID="$consumer_uid" \
  ubuntu:22.04 bash /repo/scripts/npm-consumer-check.sh
# Exit status alone cannot prove that the mounted check script ran.
python3 "$repo/scripts/npm-consumer-result.py" verify "$out/consumer-result.json" "$nonce" "$sha" "$artifact_dir" "$node_version" "$consumer_uid"
cp "$out/consumer-result.json" "$artifact_dir/consumer-result-$node_version.json"
echo "PASS host verified consumer-result-$node_version.json nonce, SHA, tarball and all named checks"
