#!/bin/bash
# 本地打包；不签名、不公证、不上传，也不覆盖已有产物。
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: bash scripts/package-macos.sh [--profile release|debug] [--skip-build] [--output-dir PATH]

Defaults: release profile, build the native architecture, output to target/packages.
--skip-build reuses an existing binary; it does not verify that binary matches current sources.
Output contains Swoop.app, swoop-cli, usage/validation notes, a zip, a DMG installer, and their SHA-256 checksums.
Requires cargo-packager (cargo install cargo-packager --version 0.11.8 --locked).
EOF
}

fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }

profile=release
skip_build=false
output_dir=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      [[ $# -ge 2 ]] || fail '--profile requires a value'
      profile=$2; shift 2 ;;
    --output-dir)
      [[ $# -ge 2 && -n "$2" ]] || fail '--output-dir requires a path'
      output_dir=$2; shift 2 ;;
    --skip-build) skip_build=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown argument: $1" ;;
  esac
done
[[ "$profile" == release || "$profile" == debug ]] || fail 'Profile must be release or debug'
[[ "$(uname -s)" == Darwin ]] || fail 'Run this script on macOS'

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
for tool in cargo cargo-packager rustc plutil ditto shasum lipo hdiutil cmp; do
  command -v "$tool" >/dev/null || fail "Required command is missing: $tool"
done

target_dir=$(cargo metadata --locked --no-deps --format-version 1 | plutil -extract target_directory raw -o - -)
package_id=$(cargo pkgid --locked -p swoop-app)
version=${package_id##*@}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][A-Za-z0-9.+-]+)?$ ]] || fail "Unexpected Cargo package version: $version"
bundle_version=${version%%[-+]*}
host_target=$(rustc -vV | sed -n 's/^host: //p')
case "$host_target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) fail "Unsupported native target: $host_target" ;;
esac

# 显式选择本机 target，避免继承 CARGO_BUILD_TARGET 导致把别的平台产物放进 .app。
# 默认开发构建通常位于 target/debug；skip-build 优先复用显式 target，再找默认目录。
binary="$target_dir/$host_target/$profile/swoop"
cli_binary="$target_dir/$host_target/$profile/swoop-cli"
if [[ "$skip_build" == false ]]; then
  build_args=(build --locked --package swoop-app --bins --target "$host_target")
  [[ "$profile" != release ]] || build_args+=(--release)
  cargo "${build_args[@]}"
elif [[ ! -f "$binary" ]]; then
  binary="$target_dir/$profile/swoop"
  cli_binary="$target_dir/$profile/swoop-cli"
fi
[[ -f "$binary" && -x "$binary" ]] || fail "Executable not found: $binary"
[[ -f "$cli_binary" && -x "$cli_binary" ]] || fail "CLI executable not found: $cli_binary"
lipo "$binary" -verify_arch "$arch" || fail "Binary does not contain $arch: $binary"
lipo "$cli_binary" -verify_arch "$arch" || fail "CLI binary does not contain $arch: $cli_binary"

output_dir=${output_dir:-"$target_dir/packages"}
mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd)
package_name="Swoop-$version-macos-$arch-$profile"
package_dir="$output_dir/$package_name"
archive="$output_dir/$package_name.zip"
dmg="$output_dir/$package_name.dmg"
# mkdir 是同名打包的独占锁，避免两个进程都通过存在性检查后互相覆盖。
lock_dir="$output_dir/.$package_name.lock"
mkdir "$lock_dir" 2>/dev/null || fail "Package output is locked; use another --output-dir or inspect this lock: $lock_dir"
stage_dir=
mount_dir=
cleanup() {
  [[ -z "$mount_dir" ]] || hdiutil detach "$mount_dir" -quiet 2>/dev/null || true
  [[ -z "$stage_dir" ]] || rm -rf "$stage_dir"
  rmdir "$lock_dir" 2>/dev/null || true
}
trap cleanup EXIT
for path in "$package_dir" "$archive" "$archive.sha256" "$dmg" "$dmg.sha256"; do
  [[ ! -e "$path" && ! -L "$path" ]] || fail "Output already exists; use another --output-dir: $path"
done

stage_dir=$(mktemp -d "$output_dir/.swoop-package.XXXXXX")
# Swoop.app 与 DMG 都由 cargo-packager 生成，ZIP 复用同一个 .app，保证两种包内容一致。
packager_dir="$stage_dir/packager"
mkdir -p "$packager_dir/bin" "$packager_dir/out"
cp "$binary" "$packager_dir/bin/swoop"
chmod 755 "$packager_dir/bin/swoop"
# cargo-packager 用完整 semver 和构建时间戳写版本号；macOS 只认数字点分版本，这里覆盖回去。
cat > "$packager_dir/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key><string>$bundle_version</string>
  <key>CFBundleVersion</key><string>$bundle_version</string>
</dict>
</plist>
EOF
json_str() { local s=${1//\\/\\\\}; s=${s//\"/\\\"}; printf '"%s"' "$s"; }
# 显式写 name：cargo-packager 0.11 缺 name 时会把配置文件路径当目录探测而失败。
cat > "$packager_dir/packager.json" <<EOF
{
  "name": "swoop-app",
  "productName": "Swoop",
  "version": $(json_str "$version"),
  "identifier": "com.muffin.swoop",
  "category": "Utility",
  "targetTriple": $(json_str "$host_target"),
  "binariesDir": $(json_str "$packager_dir/bin"),
  "outDir": $(json_str "$packager_dir/out"),
  "binaries": [{ "path": "swoop", "main": true }],
  "icons": [$(json_str "$repo_root/assets/brand/swoop/swoop.icns")],
  "resources": [{ "src": $(json_str "$repo_root/assets/fonts/NOTICE.md"), "target": "FONT-NOTICE.md" }],
  "macos": { "infoPlistPath": $(json_str "$packager_dir/Info.plist") }
}
EOF
cargo packager --config "$packager_dir/packager.json" --formats app,dmg
dmgs=("$packager_dir/out"/*.dmg)
[[ ${#dmgs[@]} -eq 1 && -f "${dmgs[0]}" ]] || fail "Expected exactly one DMG in $packager_dir/out"
mkdir -p "$stage_dir/$package_name"
bundle="$stage_dir/$package_name/Swoop.app"
ditto "$packager_dir/out/Swoop.app" "$bundle"
plutil -lint "$bundle/Contents/Info.plist"
cmp -s "$binary" "$bundle/Contents/MacOS/swoop" || fail 'Packaged executable differs from the built binary'
cp "$cli_binary" "$stage_dir/$package_name/swoop-cli"
chmod 755 "$stage_dir/$package_name/swoop-cli"

# 挂载只读 DMG 核对：拖进「应用程序」的 Swoop.app 必须与 ZIP 中的同一份。
hdiutil verify -quiet "${dmgs[0]}"
mount_dir="$stage_dir/mnt"
mkdir "$mount_dir"
hdiutil attach "${dmgs[0]}" -readonly -nobrowse -noautoopen -mountpoint "$mount_dir" -quiet
[[ -L "$mount_dir/Applications" ]] || fail 'DMG is missing the Applications link'
diff -r "$bundle" "$mount_dir/Swoop.app" >/dev/null || fail 'DMG Swoop.app differs from the zip bundle'
hdiutil detach "$mount_dir" -quiet
mount_dir=
mv "${dmgs[0]}" "$stage_dir/$package_name.dmg"
(cd "$stage_dir" && shasum -a 256 "$package_name.dmg" > "$package_name.dmg.sha256")

cp README.md "$stage_dir/$package_name/README.md"
mkdir -p "$stage_dir/$package_name/docs" "$stage_dir/$package_name/crates/media" "$stage_dir/$package_name/assets/fonts"
cp docs/*.md "$stage_dir/$package_name/docs/"
cp crates/media/README.md "$stage_dir/$package_name/crates/media/README.md"
cp assets/fonts/NOTICE.md "$stage_dir/$package_name/assets/fonts/NOTICE.md"
cp docs/desktop-validation.md "$stage_dir/$package_name/desktop-validation.md"
cp docs/cli.md "$stage_dir/$package_name/cli.md"
binary_sha=$(shasum -a 256 "$bundle/Contents/MacOS/swoop" | cut -d ' ' -f 1)
cli_sha=$(shasum -a 256 "$stage_dir/$package_name/swoop-cli" | cut -d ' ' -f 1)
revision=$(git rev-parse --verify HEAD 2>/dev/null || printf unknown)
cat > "$stage_dir/$package_name/build-info.txt" <<EOF
name=Swoop
application_id=com.muffin.swoop
version=$version
target=$host_target
profile=$profile
skip_build=$skip_build
source_revision=$revision
source_matches_binary=not_verified
rustc=$(rustc --version)
binary_sha256=$binary_sha
cli_sha256=$cli_sha
distribution_signature=none
notarization=none
EOF

# 先在临时目录中完成 zip，再移动完整产物；EXIT 只清理本脚本创建的临时目录。
ditto -c -k --sequesterRsrc --keepParent "$stage_dir/$package_name" "$stage_dir/$package_name.zip"
(cd "$stage_dir" && shasum -a 256 "$package_name.zip" > "$package_name.zip.sha256")
mv "$stage_dir/$package_name" "$package_dir"
mv "$stage_dir/$package_name.zip" "$archive"
mv "$stage_dir/$package_name.zip.sha256" "$archive.sha256"
mv "$stage_dir/$package_name.dmg" "$dmg"
mv "$stage_dir/$package_name.dmg.sha256" "$dmg.sha256"
printf 'Bundle: %s\nArchive: %s\nChecksum: %s\nInstaller: %s\n' "$package_dir/Swoop.app" "$archive" "$archive.sha256" "$dmg"
printf 'Local development package only; no Developer ID signature, notarization, install, launch, or upload performed.\n'
