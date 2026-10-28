#!/usr/bin/env bash
# Builds RadioMaster.app as a universal app (Apple Silicon and Intel Macs, macOS 11 or newer) and zips it.
# Usage: tools/package_macos.sh <version> [zip name]      (run on a Mac, from the repository root)
#
# Signing, all optional (without them the app is ad-hoc signed and macOS asks the user to allow it once):
#   APPLE_CERTIFICATE_P12        base64 of a "Developer ID Application" certificate exported as .p12
#   APPLE_CERTIFICATE_PASSWORD   the .p12 password
#   APPLE_SIGNING_IDENTITY       e.g. "Developer ID Application: DovaFM Digital Entertainment Group (TEAMID)"
# Notarization (needs the signing above), so macOS opens the app with no warning:
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD (an app-specific password from appleid.apple.com)
set -euo pipefail
ver="${1:?usage: tools/package_macos.sh <version> [zip name]}"
zip="${2:-RadioMaster-v${ver}-macos.zip}"
min_macos=11.0

# Intel Macs can't run an Apple Silicon-only program (macOS says it is "not supported on this Mac"),
# so build both and join them with lipo.
export MACOSX_DEPLOYMENT_TARGET="$min_macos"
rustup target add aarch64-apple-darwin x86_64-apple-darwin
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked --bin RadioMaster --target "$t"
done

app=RadioMaster.app
rm -rf "$app" icon.iconset "$zip"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/RadioMaster" \
  target/aarch64-apple-darwin/release/RadioMaster target/x86_64-apple-darwin/release/RadioMaster
lipo -info "$app/Contents/MacOS/RadioMaster"
# only programs go in Contents/MacOS (code signing rejects anything else there); i18n.rs looks in Resources too
cp -r lang "$app/Contents/Resources/lang"

mkdir icon.iconset
for s in 16 32 64 128 256; do
  sips -z $s $s assets/icon.png --out "icon.iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d assets/icon.png --out "icon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns icon.iconset -o "$app/Contents/Resources/RadioMaster.icns"
rm -rf icon.iconset

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>RadioMaster</string>
  <key>CFBundleDisplayName</key><string>RadioMaster</string>
  <key>CFBundleIdentifier</key><string>com.dovafm.radiomaster</string>
  <key>CFBundleVersion</key><string>$ver</string>
  <key>CFBundleShortVersionString</key><string>$ver</string>
  <key>CFBundleExecutable</key><string>RadioMaster</string>
  <key>CFBundleIconFile</key><string>RadioMaster</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>$min_macos</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>NSHumanReadableCopyright</key><string>© 2026 DovaFM Digital Entertainment Group and RadioMaster contributors. MIT licence.</string>
  <key>NSMicrophoneUsageDescription</key><string>RadioMaster uses the microphone for live talk-overs and voice tracks.</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

if [ -n "${APPLE_CERTIFICATE_P12:-}" ] && [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "Signing with $APPLE_SIGNING_IDENTITY"
  kc="${RUNNER_TEMP:-$(mktemp -d)}/radiomaster-signing.keychain-db"
  kcpass="$(uuidgen)"
  security create-keychain -p "$kcpass" "$kc"
  security set-keychain-settings -lut 3600 "$kc"
  security unlock-keychain -p "$kcpass" "$kc"
  p12="$(mktemp)"
  trap 'rm -f "$p12"' EXIT # also when the decode or import fails
  base64 --decode <<<"$APPLE_CERTIFICATE_P12" >"$p12"
  security import "$p12" -k "$kc" -P "${APPLE_CERTIFICATE_PASSWORD:-}" -T /usr/bin/codesign
  rm -f "$p12"
  security set-key-partition-list -S apple-tool:,apple: -s -k "$kcpass" "$kc" >/dev/null
  # shellcheck disable=SC2046  # the existing keychains, one word each
  security list-keychains -d user -s "$kc" $(security list-keychains -d user | tr -d '"')
  codesign --force --options runtime --timestamp --entitlements installer/macos/RadioMaster.entitlements \
    --sign "$APPLE_SIGNING_IDENTITY" "$app"
  codesign --verify --strict --verbose=2 "$app"

  if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
    echo "Notarizing"
    ditto -c -k --keepParent "$app" notarize.zip
    xcrun notarytool submit notarize.zip --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
      --password "$APPLE_APP_PASSWORD" --wait
    rm -f notarize.zip
    xcrun stapler staple "$app"
    spctl --assess --type execute --verbose "$app"
  else
    echo "::warning::Signed but not notarized (APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD are not set)"
  fi
else
  # ad-hoc: Apple Silicon needs a signature to run at all; users allow it once (see the download page)
  codesign --force --sign - "$app"
  codesign --verify --strict --verbose=2 "$app"
fi

ditto -c -k --keepParent "$app" "$zip"
echo "Built $zip"
