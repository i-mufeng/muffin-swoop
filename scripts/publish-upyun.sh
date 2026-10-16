#!/usr/bin/env bash
# Upload a signed Swoop release directory to Upyun (又拍云) via the REST API.
#
# Use AFTER sign_feed has produced feed.json locally (see docs/updates.md).
# Nothing here touches the private signing key; it only moves already-signed
# bytes, so it is safe to run from any machine.
#
# Required environment:
#   UPYUN_BUCKET      Upyun service (bucket) name
#   UPYUN_OPERATOR    API operator name
#   UPYUN_PASSWORD    API operator password (prefer an API-only sub-account)
#
# Usage:
#   bash scripts/publish-upyun.sh --dir /releases/0.7.0 \
#     [--remote-prefix swoop] [--cdn-base https://update.example.com] \
#     [--include 'feed.json;*.zip;*.sha256']
#
# --remote-prefix: remote directory inside the bucket (default: repo name).
# --cdn-base:      if given, purge the CDN cache for the uploaded URLs after
#                  upload so clients fetch the fresh feed immediately.
# --include:       semicolon-separated glob list; only matching file names
#                  are uploaded (default: feed.json, *.zip, *.dmg, *.sha256,
#                  *.hex, *-setup.exe).
#
# The script uploads feed.json LAST so that clients never see a feed that
# points at packages which are not fully uploaded yet.
set -euo pipefail

usage() {
  sed -n '2,26p' "$0"
  exit "${1:-0}"
}

DIR=""
REMOTE_PREFIX="muffin-swoop"
CDN_BASE=""
INCLUDE='feed.json;*.zip;*.dmg;*.sha256;*.hex;*-setup.exe'

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dir) DIR="${2:?--dir needs a value}"; shift 2 ;;
    --remote-prefix) REMOTE_PREFIX="${2:?--remote-prefix needs a value}"; shift 2 ;;
    --cdn-base) CDN_BASE="${2:?--cdn-base needs a value}"; shift 2 ;;
    --include) INCLUDE="${2:?--include needs a value}"; shift 2 ;;
    -h|--help) usage 0 ;;
    *) echo "Unknown argument: $1" >&2; usage 1 ;;
  esac
done

[ -n "$DIR" ] || { echo "Missing --dir <local release directory>" >&2; usage 1; }
[ -d "$DIR" ] || { echo "Not a directory: $DIR" >&2; exit 1; }
: "${UPYUN_BUCKET:?set UPYUN_BUCKET (service name)}"
: "${UPYUN_OPERATOR:?set UPYUN_OPERATOR}"
: "${UPYUN_PASSWORD:?set UPYUN_PASSWORD (export UPYUN_PASSWORD='...'; do not pass on the command line)}"

API="https://v0.api.upyun.com/${UPYUN_BUCKET}/${REMOTE_PREFIX}"

# Build the find expression: -name 'a' -o -name 'b' ...
FIND_EXPR=()
IFS=';' read -ra PATTERNS <<< "$INCLUDE"
for pattern in "${PATTERNS[@]}"; do
  [ -n "$pattern" ] || continue
  if [ "${#FIND_EXPR[@]}" -gt 0 ]; then
    FIND_EXPR+=( -o )
  fi
  FIND_EXPR+=( -name "$pattern" )
done

# Collect matching files, excluding feed.json (uploaded last).
mapfile -t FILES < <(
  find "$DIR" -maxdepth 1 -type f \( "${FIND_EXPR[@]}" \) ! -name 'feed.json' -printf '%f\n' | sort
)

[ "${#FILES[@]}" -gt 0 ] || { echo "No release files matched in $DIR" >&2; exit 1; }

upload() {
  local name="$1"
  printf 'upload  %s -> %s/%s\n' "$name" "$API" "$name"
  curl -fsS -u "${UPYUN_OPERATOR}:${UPYUN_PASSWORD}" \
    -T "$DIR/$name" "${API}/${name}"
}

for name in "${FILES[@]}"; do
  upload "$name"
done

[ -f "$DIR/feed.json" ] || { echo "feed.json not found in $DIR; refusing to publish packages without a signed feed" >&2; exit 1; }
upload "feed.json"

if [ -n "$CDN_BASE" ]; then
  CDN_BASE="${CDN_BASE%/}"
  urls="${CDN_BASE}/${REMOTE_PREFIX}/feed.json"
  for name in "${FILES[@]}"; do
    urls="${urls}${IFS:+ }${CDN_BASE}/${REMOTE_PREFIX}/${name}"
  done
  echo "purge CDN: ${urls}"
  curl -fsS -u "${UPYUN_OPERATOR}:${UPYUN_PASSWORD}" \
    -X POST -d "purge=${urls}" "http://purge.upyun.com/purge/"
  echo
fi

echo "Published ${#FILES[@]} file(s) plus feed.json to Upyun bucket '${UPYUN_BUCKET}/${REMOTE_PREFIX}'."
echo "Feed URL: https://<你的又拍云绑定域名>/${REMOTE_PREFIX}/feed.json"
