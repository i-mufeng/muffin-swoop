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
#     [--remote-prefix swoop-release/v0.7.0] [--latest] \
#     [--cdn-base https://update.example.com] \
#     [--include 'feed.json;*.zip;*.sha256']
#
# --remote-prefix: remote directory inside the bucket (default: repo name).
# --latest:        also publish to the sibling "latest" directory (e.g.
#                  swoop/latest/), giving stable direct-download URLs that
#                  always point at the newest release. The latest dir is
#                  EMPTIED first, then files are uploaded under version-less
#                  names: only the version segment is dropped and the name
#                  is lowercased (Swoop-0.6.0-windows-x86_64-release-setup.exe
#                  -> swoop-windows-x86_64-release-setup.exe); the rest of
#                  the name (arch, profile) is kept. feed.json and other
#                  version-less files keep their names. The version tag is
#                  taken from the last segment of --remote-prefix (v0.6.0).
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
  sed -n '2,36p' "$0"
  exit "${1:-0}"
}

DIR=""
REMOTE_PREFIX="muffin-swoop"
CDN_BASE=""
INCLUDE='feed.json;*.zip;*.dmg;*.sha256;*.hex;*-setup.exe'
SYNC_LATEST=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dir) DIR="${2:?--dir needs a value}"; shift 2 ;;
    --remote-prefix) REMOTE_PREFIX="${2:?--remote-prefix needs a value}"; shift 2 ;;
    --latest) SYNC_LATEST=1; shift ;;
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

# Sibling "latest" directory: swoop-release/v0.7.0 -> swoop-release/latest.
LATEST_PREFIX=""
LATEST_API=""
VERSION_TAG=""
if [ "$SYNC_LATEST" -eq 1 ]; then
  case "$REMOTE_PREFIX" in
    */*) LATEST_PREFIX="${REMOTE_PREFIX%/*}/latest" ;;
    *)   LATEST_PREFIX="${REMOTE_PREFIX}/latest" ;;
  esac
  LATEST_API="https://v0.api.upyun.com/${UPYUN_BUCKET}/${LATEST_PREFIX}"
  VERSION_TAG="${REMOTE_PREFIX##*/}"
  case "$VERSION_TAG" in
    v[0-9]*|V[0-9]*) ;;
    *) echo "--latest needs a version tag as the last --remote-prefix segment (e.g. swoop-release/v0.7.0); got '${VERSION_TAG}'" >&2; exit 1 ;;
  esac
fi

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
  local api="$1" name="$2" remote="${3:-$2}"
  printf 'upload  %s -> %s/%s\n' "$name" "$api" "$remote"
  curl -fsS -u "${UPYUN_OPERATOR}:${UPYUN_PASSWORD}" \
    -T "$DIR/$name" "${api}/${remote}"
}

upyun_delete() {
  local url="$1"
  printf 'delete  %s\n' "$url"
  curl -fsS -u "${UPYUN_OPERATOR}:${UPYUN_PASSWORD}" \
    -X DELETE "$url"
}

# Latest-dir rename rule: Swoop-0.6.0-windows-x86_64-release-setup.exe ->
# swoop-windows-x86_64-release-setup.exe (strip the version segment and
# lowercase; arch/profile are kept). Version-less files pass through
# unchanged (feed.json, swoop-update-public-key.hex, ...).
latest_name() {
  local name="$1"
  name="${name//-${VERSION_TAG#v}/}"  # version dir is v0.6.0, file names use 0.6.0
  echo "${name,,}"
}

for name in "${FILES[@]}"; do
  upload "$API" "$name"
done

[ -f "$DIR/feed.json" ] || { echo "feed.json not found in $DIR; refusing to publish packages without a signed feed" >&2; exit 1; }
upload "$API" "feed.json"

if [ -n "$LATEST_API" ]; then
  # 先删再发: empty the latest dir first so no stale file survives a rename
  # or a dropped platform.
  echo "clearing latest dir: ${LATEST_API}/"
  listing="$(curl -fsS -u "${UPYUN_OPERATOR}:${UPYUN_PASSWORD}" \
      -H 'X-List-Limit: 200' "${LATEST_API}/" 2>/dev/null || true)"
  while IFS=$'\t' read -r entry entry_type _rest; do
    [ "$entry_type" = "F" ] || continue
    [ -n "$entry" ] || continue
    upyun_delete "${LATEST_API}/${entry}"
  done <<< "$listing"

  for name in "${FILES[@]}"; do
    upload "$LATEST_API" "$name" "$(latest_name "$name")"
  done
  upload "$LATEST_API" "feed.json"
fi

if [ -n "$CDN_BASE" ]; then
  CDN_BASE="${CDN_BASE%/}"
  urls="${CDN_BASE}/${REMOTE_PREFIX}/feed.json"
  for name in "${FILES[@]}"; do
    urls="${urls}${IFS:+ }${CDN_BASE}/${REMOTE_PREFIX}/${name}"
  done
  if [ -n "$LATEST_PREFIX" ]; then
    urls="${urls}${IFS:+ }${CDN_BASE}/${LATEST_PREFIX}/feed.json"
    for name in "${FILES[@]}"; do
      urls="${urls}${IFS:+ }${CDN_BASE}/${LATEST_PREFIX}/$(latest_name "$name")"
    done
  fi
  echo "purge CDN: ${urls}"
  curl -fsS -u "${UPYUN_OPERATOR}:${UPYUN_PASSWORD}" \
    -X POST -d "purge=${urls}" "http://purge.upyun.com/purge/"
  echo
fi

echo "Published ${#FILES[@]} file(s) plus feed.json to Upyun bucket '${UPYUN_BUCKET}/${REMOTE_PREFIX}'."
if [ -n "$LATEST_PREFIX" ]; then
  echo "Published latest path (cleared first, version-less names): ${UPYUN_BUCKET}/${LATEST_PREFIX}"
  echo "Latest feed URL: https://<你的又拍云绑定域名>/${LATEST_PREFIX}/feed.json"
fi
echo "Feed URL: https://<你的又拍云绑定域名>/${REMOTE_PREFIX}/feed.json"
