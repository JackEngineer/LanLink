#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
version="$(node -p "require('./package.json').version")"
agent_version="${LANLINK_RELEASE_VERSION:-$version}"
[[ "$agent_version" =~ ^[0-9]+[.][0-9]+[.][0-9]+(-preview[.][0-9]+)?$ ]] || { echo "Unsupported LANLINK_RELEASE_VERSION" >&2; exit 2; }
arch="${1:-aarch64-apple-darwin}"
case "$arch" in aarch64-apple-darwin|x86_64-apple-darwin) ;; *) exit 2 ;; esac
# A partial signing configuration must never silently produce an ad-hoc release.
app_identity="${LANLINK_MAC_APP_IDENTITY:-}"
installer_identity="${LANLINK_MAC_INSTALLER_IDENTITY:-}"
notary_profile="${LANLINK_MAC_NOTARY_PROFILE:-}"
require_notarization="${LANLINK_MAC_REQUIRE_NOTARIZATION:-0}"
case "$require_notarization" in 0|1) ;; *) echo "LANLINK_MAC_REQUIRE_NOTARIZATION must be 0 or 1" >&2; exit 2 ;; esac
signed=0
if [[ -n "$app_identity$installer_identity$notary_profile" || "$require_notarization" == 1 ]]; then
  if [[ "$app_identity" != "Developer ID Application: "* || "$installer_identity" != "Developer ID Installer: "* || -z "$notary_profile" ]]; then
    echo "Release signing requires Developer ID Application, Developer ID Installer and a notarytool keychain profile" >&2
    exit 2
  fi
  signed=1
fi
notarize() {
  local result
  result="$(xcrun notarytool submit "$1" --keychain-profile "$notary_profile" --wait --output-format json)"
  printf '%s' "$result" | python3 -c 'import json, sys; result = json.load(sys.stdin); sys.exit(0 if result.get("status") == "Accepted" else "Apple notarization was not Accepted")'
}
npm run build
cargo build --release --locked --target-dir target --target "$arch" -p lanlink-agent
cargo build --release --locked --target-dir target/desktop-shell --target "$arch" --manifest-path crates/desktop-shell/Cargo.toml
out="dist/macos-$arch"
rm -rf -- "$out"
app="$out/LANLink.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/web"
cp "target/$arch/release/lanlink-agent" "$app/Contents/MacOS/lanlink-agent"
cp -R apps/web/dist/. "$app/Contents/Resources/web/"
cp LICENSE "$app/Contents/Resources/LICENSE"
cp assets/branding/lanlink.icns "$app/Contents/Resources/LANLink.icns"
bash scripts/macos/stage-desktop-ui.sh "$app/Contents" "target/desktop-shell/$arch/release/lanlink-desktop-shell" "$version"
workbench="$app/Contents/Helpers/LANLink Workbench.app"
case "$arch" in
  aarch64-apple-darwin) launcher_arch=arm64 ;;
  x86_64-apple-darwin) launcher_arch=x86_64 ;;
esac
xcrun clang -arch "$launcher_arch" -mmacosx-version-min=11.0 -Wall -Wextra -Werror \
  scripts/macos/launcher.c -o "$app/Contents/MacOS/LANLink"
cp scripts/macos/launch.sh "$app/Contents/Resources/launch.sh"
cp scripts/macos/{login-start,stop,uninstall}.sh "$app/Contents/Resources/"
chmod +x "$app/Contents/MacOS/LANLink"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.lanlink.desktop</string>
<key>CFBundleName</key><string>LANLink</string>
<key>CFBundleExecutable</key><string>LANLink</string>
<key>CFBundleIconFile</key><string>LANLink.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>4</string>
<key>LANLinkAgentVersion</key><string>$agent_version</string>
<key>LSUIElement</key><true/>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"
if [[ "$signed" == 1 ]]; then
  codesign --force --options runtime --timestamp --sign "$app_identity" "$app/Contents/MacOS/lanlink-agent"
  codesign --force --options runtime --timestamp --sign "$app_identity" "$workbench"
  codesign --force --options runtime --timestamp --sign "$app_identity" "$app"
else
  codesign --force --sign - "$app/Contents/MacOS/lanlink-agent"
  codesign --force --sign - "$workbench"
  codesign --force --sign - "$app"
fi
codesign --verify --deep --strict "$app"
cp scripts/macos/login-start.sh "$out/"
cp scripts/macos/uninstall.sh "$out/"
cp scripts/macos/stop.sh "$out/"
cp LICENSE "$out/LICENSE"
# Notarize the app separately, then staple before the installer captures its bytes.
if [[ "$signed" == 1 ]]; then
  notary_zip="$out/notary-app.zip"
  ditto -c -k --keepParent "$app" "$notary_zip"
  notarize "$notary_zip"
  rm "$notary_zip"
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  spctl --assess --type execute --verbose=2 "$app"
fi
pkg_sign_args=()
if [[ "$signed" == 1 ]]; then
  pkg_sign_args=(--sign "$installer_identity" --timestamp)
fi
pkgbuild ${pkg_sign_args[@]+"${pkg_sign_args[@]}"} --component "$app" --install-location /Applications --identifier app.lanlink.desktop --version "$version" "$out/LANLink-$version-$arch.pkg"
if [[ "$signed" == 1 ]]; then
  pkg="$out/LANLink-$version-$arch.pkg"
  pkgutil --check-signature "$pkg"
  notarize "$pkg"
  xcrun stapler staple "$pkg"
  xcrun stapler validate "$pkg"
  spctl --assess --type install --verbose=2 "$pkg"
fi
ditto -c -k --keepParent "$out" "dist/LANLink-$version-$arch.zip"
shasum -a 256 "$out/LANLink-$version-$arch.pkg" "dist/LANLink-$version-$arch.zip"
