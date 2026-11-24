#!/usr/bin/env bash
# Run only on a disposable macOS release runner.
set -euo pipefail
umask 077
: "${RUNNER_TEMP:?}" "${GITHUB_ACTIONS:?}" "${MAC_CERTIFICATES_P12_BASE64:?}" "${MAC_CERTIFICATES_PASSWORD:?}"
: "${MAC_NOTARY_KEY_BASE64:?}" "${MAC_NOTARY_KEY_ID:?}" "${MAC_NOTARY_ISSUER_ID:?}"
private_dir=$(mktemp -d "$RUNNER_TEMP/lanlink-signing.XXXXXXXX")
p12="$private_dir/identities.p12"
api_key="$private_dir/notary.p8"
trap 'rm -f "$p12" "$api_key"' EXIT
printf %s "$MAC_CERTIFICATES_P12_BASE64" | /usr/bin/base64 -D > "$p12"
printf %s "$MAC_NOTARY_KEY_BASE64" | /usr/bin/base64 -D > "$api_key"

keychain="$private_dir/release.keychain-db"
password=$(openssl rand -hex 32)
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$p12" -k "$keychain" -P "$MAC_CERTIFICATES_PASSWORD" \
  -T /usr/bin/codesign -T /usr/bin/productsign -T /usr/bin/pkgbuild
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" >/dev/null
login_keychain=$(security default-keychain -d user | sed -E 's/^[[:space:]]*"([^"]+)"$/\1/')
security list-keychains -d user -s "$keychain" "$login_keychain"
xcrun notarytool store-credentials LANLink-release --key "$api_key" \
  --key-id "$MAC_NOTARY_KEY_ID" --issuer "$MAC_NOTARY_ISSUER_ID"
