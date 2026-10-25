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
python3 "$repo/scripts/npm-source-metadata.py" "$artifact_dir" "$sha"
out=$(mktemp -d)
nonce=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
capture_diagnostics() {
  if [[ -f "$out/consumer-diagnostics.json" ]]; then
    cp "$out/consumer-diagnostics.json" "$artifact_dir/consumer-diagnostics-$node_version.json"
    echo "Consumer failed; bounded details saved in consumer-diagnostics-$node_version.json" >&2
  fi
  if [[ -f "$out/reader-extraction-cost.json" ]]; then
    cp "$out/reader-extraction-cost.json" "$artifact_dir/reader-extraction-cost-$node_version.json"
  fi
  if [[ -f "$out/reader-index-timeout.json" ]]; then
    cp "$out/reader-index-timeout.json" "$artifact_dir/reader-index-timeout-$node_version.json"
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
echo "PASS host verified consumer-result-$node_version.json nonce, SHA, tarball, source metadata and all named checks"
