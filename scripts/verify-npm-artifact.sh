#!/usr/bin/env bash
set -euo pipefail
artifact_dir=$(realpath "${1:?artifact directory}")
repo=$(realpath "${2:?repository checkout}")
sha=${3:?source SHA}
node_version=${4:?Node version}
[[ "$sha" =~ ^[0-9a-f]{40}$ ]]
[[ "$(git -C "$repo" rev-parse HEAD)" == "$sha" ]]
out=$(mktemp -d)
trap 'rm -rf -- "$out"' EXIT
nonce=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
docker run --rm --init \
  --volume "$artifact_dir:/artifacts:ro" --volume "$repo:/repo:ro" --volume "$out:/out" \
  --env EXPECTED_SHA="$sha" --env CONSUMER_NONCE="$nonce" --env NODE_VERSION="$node_version" \
  ubuntu:22.04 bash /repo/scripts/npm-consumer-check.sh
# Exit status alone cannot prove that the mounted check script ran.
python3 "$repo/scripts/npm-consumer-result.py" verify "$out/consumer-result.json" "$nonce" "$sha" "$artifact_dir" "$node_version"
cp "$out/consumer-result.json" "$artifact_dir/consumer-result-$node_version.json"
echo "PASS host verified consumer-result-$node_version.json nonce, SHA, tarball and all named checks"
