#!/usr/bin/env bash
set -euo pipefail

# 先做最便宜的版本号一致性检查(以 package.json 为准),不一致就别浪费时间 build 镜像。
# 发版流水线里 preflight job 另外会用 tag 版本再查一次。
bash "$(dirname "${BASH_SOURCE[0]}")/check-version-consistency.sh"

image_name="${AGENTGATE_PREFLIGHT_IMAGE:-agentgate-preflight}"
container_name="${AGENTGATE_PREFLIGHT_CONTAINER:-agentgate-preflight}"
host_port="${AGENTGATE_PREFLIGHT_PORT:-19090}"

cleanup() {
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}

trap cleanup EXIT

docker build -t "$image_name" .

cleanup
docker run -d \
  --name "$container_name" \
  -p "127.0.0.1:${host_port}:9090" \
  "$image_name" >/dev/null

for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${host_port}/health" >/dev/null; then
    echo "Docker preflight passed: agentgate-serve is running."
    exit 0
  fi
  sleep 1
done

docker logs "$container_name" >&2 || true
echo "Docker preflight failed: health endpoint did not become ready." >&2
exit 1
