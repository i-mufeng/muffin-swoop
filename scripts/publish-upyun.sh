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
#     [--remote-prefix swoop/v0.7.0] [--latest] \
#     [--sign-key ~/.swoop-release/swoop-ed25519-seed.hex] \
#     [--cdn-base https://cdn.muffin.xin] \
#     [--include 'feed.json;*.zip;*.sha256']
#
# --remote-prefix: remote directory inside the bucket; last segment must be
#                  the version tag (v0.7.0) when --latest or --sign-key is
#                  used (default: repo name).
# --latest:        also publish to the sibling "latest" directory (e.g.
#                  swoop/latest/), giving stable direct-download URLs that
#                  always point at the newest release. The latest dir is
#                  EMPTIED first, then files are uploaded under version-less
#                  names: the version segment and "-release" are dropped and
#                  the name is lowercased (Swoop-0.6.0-windows-x86_64-release-setup.exe
#                  -> swoop-windows-x86_64-setup.exe); the arch is kept.
#                  feed.json and other version-less files keep their names.
# --sign-key:      if $DIR/feed.json does not exist yet, sign it offline
#                  first with the Ed25519 seed at this path (via the
#                  swoop-updater sign_feed example; requires cargo and a
#                  versioned --remote-prefix plus --cdn-base for the
#                  artifact URLs). Updatable artifacts are the release *.zip
#                  files; platform keys are derived from their names.
# --cdn-base:      public CDN origin, e.g. https://cdn.muffin.xin; used for
#                  the signed artifact URLs and to purge the CDN cache for
#                  the uploaded URLs after upload so clients fetch the fresh
#                  feed immediately (Upyun purge API, not Basic auth).
# --include:       semicolon-separated glob list; only matching file names
#                  are uploaded (default: feed.json, *.zip, *.dmg, *.sha256,
#                  *.hex, *-setup.exe).
#
# The script uploads feed.json LAST so that clients never see a feed that
# points at packages which are not fully uploaded yet.
set -euo pipefail

usage() {
  sed -n '2,46p' "$0"
  exit "${1:-0}"
}

DIR=""
REMOTE_PREFIX="muffin-swoop"
CDN_BASE=""
SIGN_KEY=""
INCLUDE='feed.json;*.zip;*.dmg;*.sha256;*.hex;*-setup.exe;*.crx;*.xpi'
SYNC_LATEST=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dir) DIR="${2:?--dir needs a value}"; shift 2 ;;
    --remote-prefix) REMOTE_PREFIX="${2:?--remote-prefix needs a value}"; shift 2 ;;
    --latest) SYNC_LATEST=1; shift ;;
    --sign-key) SIGN_KEY="${2:?--sign-key needs a value}"; shift 2 ;;
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
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Sibling "latest" directory: swoop/v0.7.0 -> swoop/latest.
LATEST_PREFIX=""
LATEST_API=""
VERSION_TAG=""
if [ "$SYNC_LATEST" -eq 1 ]; then
  case "$REMOTE_PREFIX" in
    */*) LATEST_PREFIX="${REMOTE_PREFIX%/*}/latest" ;;
    *)   LATEST_PREFIX="${REMOTE_PREFIX}/latest" ;;
  esac
  LATEST_API="https://v0.api.upyun.com/${UPYUN_BUCKET}/${LATEST_PREFIX}"
fi
if [ "$SYNC_LATEST" -eq 1 ] || [ -n "$SIGN_KEY" ]; then
  VERSION_TAG="${REMOTE_PREFIX##*/}"
  case "$VERSION_TAG" in
    v[0-9]*|V[0-9]*) ;;
    *) echo "--remote-prefix must end with a version tag (e.g. swoop/v0.7.0) when --latest or --sign-key is used; got '${VERSION_TAG}'" >&2; exit 1 ;;
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

# Sign feed.json offline when missing and a key was provided.
platform_key() {
  case "$1" in
    *-windows-x86_64-*.zip)                          echo windows-x86_64 ;;
    *-windows-arm64-*.zip|*-windows-aarch64-*.zip)   echo windows-aarch64 ;;
    *-macos-arm64-*.zip|*-macos-aarch64-*.zip)       echo macos-aarch64 ;;
    *) return 1 ;;
  esac
}

sign_feed() {
  local version="${VERSION_TAG#v}" name key cargo_bin
  local args=() notes=()
  for name in "${FILES[@]}"; do
    key="$(platform_key "$name" || true)"
    [ -n "$key" ] || continue
    args+=( --artifact "$key" "$DIR/$name" "${CDN_BASE}/${REMOTE_PREFIX}/${name}" )
  done
  [ "${#args[@]}" -gt 0 ] || { echo "--sign-key: no updatable release *.zip found in $DIR" >&2; exit 1; }
  [ -f "$DIR/notes.txt" ] && notes=( --notes-file "$DIR/notes.txt" )
  cargo_bin="$(command -v cargo || true)"
  [ -n "$cargo_bin" ] || cargo_bin="$HOME/.cargo/bin/cargo"
  [ -x "$cargo_bin" ] || { echo "cargo not found; install Rust or provide a pre-signed feed.json" >&2; exit 1; }
  printf 'sign    feed.json (version %s, key ...%s)\n' "$version" "${SIGN_KEY: -8}"
  ( cd "$PROJECT_ROOT" && SWOOP_RELEASE_SIGN=1 "$cargo_bin" run -q -p swoop-updater --example sign_feed -- \
      --private-key "$SIGN_KEY" --version "$version" --channel stable \
      "${args[@]}" "${notes[@]}" \
      --output "$DIR/feed.json" \
      --public-key-out "$DIR/swoop-update-public-key.hex" )
}

if [ ! -f "$DIR/feed.json" ]; then
  if [ -n "$SIGN_KEY" ]; then
    [ -n "$CDN_BASE" ] || { echo "--sign-key needs --cdn-base to build artifact URLs" >&2; exit 1; }
    sign_feed
    # The freshly written public key must go up with the packages.
    if [ -f "$DIR/swoop-update-public-key.hex" ]; then
      FILES+=("swoop-update-public-key.hex")
    fi
  else
    echo "feed.json not found in $DIR; refusing to publish packages without a signed feed (sign offline or pass --sign-key <seed>)" >&2; exit 1
  fi
fi

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
# swoop-windows-x86_64-setup.exe (strip the version segment and "-release",
# lowercase; the arch is kept). Version-less files pass through unchanged
# (feed.json, swoop-update-public-key.hex, ...).
latest_name() {
  local name="$1"
  name="${name//-${VERSION_TAG#v}/}"  # version dir is v0.6.0, file names use 0.6.0
  name="${name//-release/}"
  echo "${name,,}"
}

for name in "${FILES[@]}"; do
  upload "$API" "$name"
done

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
  purge_urls=(
    "${CDN_BASE}/${REMOTE_PREFIX}/feed.json"
  )
  for name in "${FILES[@]}"; do
    purge_urls+=( "${CDN_BASE}/${REMOTE_PREFIX}/${name}" )
  done
  if [ -n "$LATEST_PREFIX" ]; then
    purge_urls+=( "${CDN_BASE}/${LATEST_PREFIX}/feed.json" )
    for name in "${FILES[@]}"; do
      purge_urls+=( "${CDN_BASE}/${LATEST_PREFIX}/$(latest_name "$name")" )
    done
  fi

  # Upyun purge API (docs.upyun.com/api/purge):
  #   Authorization: UpYun <bucket>:<operator>:<md5(joined&bucket&date&md5(pw))>
  # joined = URLs separated by \n with a trailing \n, identical to the body.
  purge_date="$(date -u '+%a, %d %b %Y %H:%M:%S GMT')"
  purge_pw_md5="$(printf '%s' "$UPYUN_PASSWORD" | md5sum | cut -d' ' -f1)"
  purge_tmp="$(mktemp)"
  printf '%s\n' "${purge_urls[@]}" > "$purge_tmp"
  purge_joined="$(cat "$purge_tmp")"  # $() strips the trailing newline; re-add below
  purge_sign="$(printf '%s\n&%s&%s&%s' "$purge_joined" "$UPYUN_BUCKET" "$purge_date" "$purge_pw_md5" | md5sum | cut -d' ' -f1)"
  purge_file="$purge_tmp"
  if command -v cygpath >/dev/null 2>&1; then
    purge_file="$(cygpath -w "$purge_tmp")"  # native curl on Git Bash needs a Windows path
  fi
  echo "purge CDN: ${purge_urls[*]}"
  curl -fsS -X POST "http://purge.upyun.com/purge/" \
    -H "Date: ${purge_date}" \
    -H "Authorization: UpYun ${UPYUN_BUCKET}:${UPYUN_OPERATOR}:${purge_sign}" \
    -F "purge=@${purge_file};type=text/plain"
  echo
  rm -f "$purge_tmp"
fi

echo "Published ${#FILES[@]} file(s) plus feed.json to Upyun bucket '${UPYUN_BUCKET}/${REMOTE_PREFIX}'."
if [ -n "$LATEST_PREFIX" ]; then
  echo "Published latest path (cleared first, version-less names): ${UPYUN_BUCKET}/${LATEST_PREFIX}"
  echo "Latest feed URL: https://<你的又拍云绑定域名>/${LATEST_PREFIX}/feed.json"
fi
echo "Feed URL: https://<你的又拍云绑定域名>/${REMOTE_PREFIX}/feed.json"
