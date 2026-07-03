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

Store these as GitHub Actions secrets, preferably on a protected `release`
environment with required reviewers. Do not commit certificates, passwords,
exported keychains, or notarization profiles to the repository.

The workflow expects `APPLE_DEVELOPER_ID_CERTIFICATE_BASE64` to be a base64
encoded `.p12` export of a Developer ID Application certificate. The `.p12`
password goes in `APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD`. `APPLE_KEYCHAIN_PASSWORD`
is any strong temporary password used only for the ephemeral CI keychain.

For local release builds, `.codesign.env` provides defaults and explicit
environment variables override values from that file.
