#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_dir"

go run ./cmd/practice-server &
practice_pid=$!

cleanup() {
  kill "$practice_pid" 2>/dev/null || true
  wait "$practice_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

for _ in {1..100}; do
  if curl --fail --silent http://127.0.0.1:8787/healthz >/dev/null; then
    exec_status=0
    break
  fi
  if ! kill -0 "$practice_pid" 2>/dev/null; then
    wait "$practice_pid"
  fi
  exec_status=1
  sleep 0.1
done

if [[ "${exec_status:-1}" -ne 0 ]]; then
  echo "Go practice engine did not become ready." >&2
  exit 1
fi

corepack pnpm --filter @trl/web dev --hostname 127.0.0.1 --port "${PORT:-3000}"
