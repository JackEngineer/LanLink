#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
arch="${1:-aarch64-apple-darwin}"
case "$arch" in aarch64-apple-darwin|x86_64-apple-darwin) ;; *) exit 2 ;; esac
npm run build
cargo build --release --locked --target-dir target --target "$arch" -p lanlink-agent
out="dist/macos-$arch"
rm -rf -- "$out"
app="$out/LANLink.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/web"
cp "target/$arch/release/lanlink-agent" "$app/Contents/MacOS/lanlink-agent"
cp -R apps/web/dist/. "$app/Contents/Resources/web/"
cp LICENSE "$app/Contents/Resources/LICENSE"
cp scripts/macos/launch.sh "$app/Contents/MacOS/LANLink"
cp scripts/macos/{login-start,stop,uninstall}.sh "$app/Contents/Resources/"
chmod +x "$app/Contents/MacOS/LANLink"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.lanlink.desktop</string>
<key>CFBundleName</key><string>LANLink</string>
<key>CFBundleExecutable</key><string>LANLink</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.2.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app/Contents/MacOS/lanlink-agent"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
cp scripts/macos/login-start.sh "$out/"
cp scripts/macos/uninstall.sh "$out/"
cp scripts/macos/stop.sh "$out/"
cp LICENSE "$out/LICENSE"
pkgbuild --component "$app" --install-location /Applications --identifier app.lanlink.desktop --version 0.2.0 "$out/LANLink-0.2.0-$arch.pkg"
ditto -c -k --keepParent "$out" "dist/LANLink-0.2.0-$arch.zip"
shasum -a 256 "$out/LANLink-0.2.0-$arch.pkg" "dist/LANLink-0.2.0-$arch.zip"
