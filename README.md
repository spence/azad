# Azad (macOS background app)

Development workflow now targets a launchd-managed app instance.

## Commands

```bash
./dev install   # build + install ~/Applications/Azad.app and LaunchAgent plist
./dev start     # start/restart via launchctl
./dev stop      # stop via launchctl
./dev restart   # stop + start
./dev status    # launchctl print
./dev logs      # tail stdout/stderr logs
./dev uninstall # remove LaunchAgent plist (keeps app bundle)
```

## Defaults

- App bundle: `~/Applications/Azad.app`
- LaunchAgent: `~/Library/LaunchAgents/com.spence.azad.plist`
- Logs: `~/Library/Logs/Azad/{stdout,stderr}.log`

## Microphone Permission

Azad requires macOS microphone permission.

- Reset permission prompt:
  - `tccutil reset Microphone com.spence.azad`
- Then restart Azad:
  - `./dev restart`

Azad also checks Accessibility permission on startup (required for auto-paste) and opens the Accessibility settings pane if it is missing.
