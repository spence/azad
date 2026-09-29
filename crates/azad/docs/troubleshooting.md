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

## Shortcuts Do Nothing

While any app holds Secure Input (a focused password field, a password manager unlocking, some
remote-desktop prompts), macOS delivers no keys to Azad's event tap, so shortcuts are unavailable
by design. Find the app holding it:

```bash
ioreg -l -w 0 | grep -o 'kCGSSessionSecureInputPID"=[0-9]*'
ps -o comm= -p <pid>
```

Shortcuts return when that app releases Secure Input.

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

## Reset Permissions

```bash
just reset-permissions
just restart
```

## Verify Local Setup

```bash
just doctor
```
