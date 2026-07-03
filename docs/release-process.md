# Release Process

Public builds are produced from tags by `.github/workflows/release.yml`. The
workflow imports a Developer ID certificate from GitHub Actions secrets, signs
the app with hardened runtime, notarizes/staples the app and DMG, verifies the
result, and uploads the DMG to the GitHub Release.

Required release secrets:

- `APPLE_DEVELOPER_ID_CERTIFICATE_BASE64`
- `APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD`
- `APPLE_KEYCHAIN_PASSWORD`
- `APPLE_ID`
- `APPLE_TEAM_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`

For local release builds, `.codesign.env` provides defaults and explicit
environment variables override values from that file.
