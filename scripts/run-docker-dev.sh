#!/usr/bin/env bash

set -euo pipefail

cache_dir="${DTM_CACHE_DIR:-${HOME}/.cache/dtm-download}"
port="${DTM_PORT:-5173}"
cid_file="$(mktemp)"
container_id=""

rm -f "$cid_file"

cleanup() {
  if [[ -s "$cid_file" ]]; then
    IFS= read -r container_id < "$cid_file"
    if [[ "$container_id" =~ ^[0-9a-f]{12,64}$ ]]; then
      docker stop --time 10 "$container_id" >/dev/null 2>&1 || true
    fi
  fi
  rm -f "$cid_file"
}

trap cleanup EXIT HUP INT TERM

mkdir -p "$cache_dir"

docker run \
  --rm \
  --init \
  --cidfile "$cid_file" \
  --publish "${port}:3000" \
  --mount "type=bind,source=${cache_dir},target=/var/cache/ontario-dtm-download" \
  ontario-dtm-download:local
