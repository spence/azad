# Troubleshooting

## Service Not Running

```bash
just status
```

If the service is not loaded:

```bash
just install
just start
```

## Check Launchd State

```bash
launchctl print gui/$(id -u)/ai.azad
```

## Check Running App Identity

```bash
lsappinfo info -app Azad
pgrep -fl '/Applications/Azad.app/Contents/MacOS/azad|\\bazad\\b'
```

## Check Bundle Metadata

```bash
plutil -p "$HOME/Applications/Azad.app/Contents/Info.plist"
codesign -dv --verbose=4 "$HOME/Applications/Azad.app"
```

If `Signature=adhoc` appears, remove the ad-hoc config and set a stable local
identity in `.codesign.env`:

```bash
security find-identity -v -p codesigning "$HOME/Library/Keychains/login.keychain-db"
```

If no `AZAD_CODESIGN_IDENTITY` is configured, `just install` still works; it
installs an unsigned development build and does not run `codesign`.

## View Logs

```bash
just logs
```

Direct paths:

- `~/Library/Logs/Azad/stdout.log`
- `~/Library/Logs/Azad/stderr.log`

## Keyboard Shortcuts Not Working

```bash
just status
sudo tail -n 20 /var/log/azad-capture.log
"$HOME/Applications/Azad.app/Contents/Library/Helpers/Azad Capture.app/Contents/MacOS/azad-capture" --list-devices
```

The helper's latest `status` line names the blocker:

- `permission_denied`: enable "Azad Capture" in Privacy & Security -> Input Monitoring. It
  appears after Azad's first launch shows the macOS prompt.
- `driver_unavailable`: run `just install-capture-driver` and allow the driver extension; check
  `systemextensionsctl list | grep pqrs`.
- `idle` with no helper connection: allow "Azad" in Login Items & Extensions and relaunch.
- `unavailable_devices` lists keyboards another app holds exclusively; shortcuts from those
  keyboards are unavailable. Karabiner-Elements and Kanata are supported: Azad captures their
  output keyboard instead.

The helper never logs typed keys; its log holds status changes and counters.

## Reset Permissions

```bash
just reset-permissions
just restart
```

## Verify Local Setup

```bash
just doctor
```
