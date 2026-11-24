#!/usr/bin/env bash
set -euo pipefail
contents="$1"
binary="$2"
version="$3"
shell="$contents/Helpers/LANLink Workbench.app"
mkdir -p "$shell/Contents/MacOS" "$shell/Contents/Resources"
install -m 755 "$binary" "$shell/Contents/MacOS/lanlink-desktop-shell"
install -m 644 "$contents/Resources/LANLink.icns" "$shell/Contents/Resources/LANLink.icns"
cat >"$shell/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.lanlink.desktop.workbench</string>
<key>CFBundleName</key><string>LANLink</string>
<key>CFBundleExecutable</key><string>lanlink-desktop-shell</string>
<key>CFBundleIconFile</key><string>LANLink.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>4</string>
</dict></plist>
PLIST
plutil -lint "$shell/Contents/Info.plist"
