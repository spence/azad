# Release Process

Public builds are produced locally by a maintainer, then uploaded to GitHub
Releases. GitHub Actions verifies source builds, but it does not hold signing
certificates or Apple notarization credentials.

## One-Time Maintainer Setup

Create a local release config:

```bash
cp .release.env.example .release.env
```

Set:

```bash
AZAD_SIGNING_IDENTITY="<Developer ID Application certificate hash>"
AZAD_NOTARIZATION_PROFILE="azad-notarization"
```

Store Apple notarization credentials in the macOS Keychain:

```bash
xcrun notarytool store-credentials "azad-notarization" \
  --apple-id "you@example.com" \
  --team-id "35A87BDK48"
```

Paste the Apple app-specific password at the secure prompt. Do not put Apple
passwords, `.p12` exports, or keychain material in `.release.env`.

## Build

```bash
just dist
```

`just dist` builds `dist/Azad.app`, signs it with hardened runtime, notarizes
and staples the app, creates `dist/Azad-<version>.dmg`, then signs, notarizes,
and staples the DMG.

The release version is read from the workspace version in the root
`Cargo.toml`.

## Verify

```bash
AZAD_VERSION="$(awk -F '"' '/^version =/ { print $2; exit }' Cargo.toml)"
codesign --verify --deep --strict --verbose=4 dist/Azad.app
xcrun stapler validate dist/Azad.app
xcrun stapler validate "dist/Azad-${AZAD_VERSION}.dmg"
spctl -a -vvv -t open --context context:primary-signature "dist/Azad-${AZAD_VERSION}.dmg"
```

## Upload

Create or update a GitHub Release for the matching tag and upload the DMG:

```bash
AZAD_VERSION="$(awk -F '"' '/^version =/ { print $2; exit }' Cargo.toml)"
gh release view "v${AZAD_VERSION}" >/dev/null 2>&1 || \
  gh release create "v${AZAD_VERSION}" --title "Azad v${AZAD_VERSION}" --notes ""
gh release upload "v${AZAD_VERSION}" "dist/Azad-${AZAD_VERSION}.dmg" --clobber
```
