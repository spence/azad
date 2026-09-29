# Sleep and wake on the host

Recorded by `tests/physical/sleep_wake_watch.py` on catalyst (MacBookPro18,4) while the tested
build ran (app CDHash `7d206309…`, helper `ebb686bd…`; see
[host deployment](../2026-09-29-installed-build/host-deployment.txt)). Nothing typed was
recorded. A Tart VM cannot sleep (`pmset sleepnow`: `0xe00002e2`), so this is the only
sleep/wake evidence. [`report.json`](report.json) passes every check.

- **Sleep and wake:** the owner slept the Mac at 19:04:00 UTC (`kern.sleeptime`) and it woke
  4 s later (`kern.waketime`).
- **Devices after wake:** the USB devices were torn down during sleep. The helper log shows it
  re-seizing them one by one after wake: built-in keyboard, then both Razer interfaces, then
  the Keychron, back to four seized devices with `capturing` throughout. The app's context
  lease lapsed for 2.3 s while its main loop resumed (`context_lease_expired`), then renewed.
- **Azad's view:** it logged capture reconnected 19–20 s after wake. An Option+Space hold 48 s
  after wake arrived as `hotkey_pressed`/`hotkey_released`, and the helper's claimed edges
  rose by exactly 2 (Space down and up).
- **Ordinary typing after wake:** forwarded edges rose from 3632 before sleep to 3710 after
  wake, with 0 forward errors.
- **Held modifiers:** the combined modifier state showed no held Shift, Control, Option or
  Command in any sample after wake, so nothing was stuck.
