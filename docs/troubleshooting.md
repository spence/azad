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
