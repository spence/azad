# Installed-build VM smoke

The `Azad.app` that `just install` built and installed on catalyst (identities in
[`identities.txt`](identities.txt)) was copied unchanged into the release-candidate VM
(`azad-capture-1790666805`: macOS 26.6.2, SIP enabled, driver approved, existing
`ai.azad.capture` Input Monitoring grant) before the host restarted into it. In the guest,
`codesign` reported app CDHash `7d2063098eef24e0b643fce4b30a32e826818853` and helper CDHash
`ebb686bdfd9a87c6c927162fc63cf06b3e09bb53`, and `codesign --verify --strict` passed. Replacing
the previous app upgraded the running helper without a new permission prompt, and Azad
reported `capture=Capturing permission=Granted driver=Ready`.

The helper is byte-for-byte the release candidate's code; the app binary differs from the
release candidate's only by build metadata (product source is unchanged since `6ded6d1`). This
run covers the app-side scenarios plus the app-crash and hung-app lease scenarios.

```sh
python3 crates/azad-capture/tests/vm/run_matrix.py --vm <vm> --out <dir> --only \
  app_hold_release app_double_tap_on app_double_tap_off app_history_search_us \
  app_history_search_french app_overlay_keys app_caps_lock_and_repeat app_ordinary_typing \
  app_quit_mid_hold
python3 crates/azad-capture/tests/vm/run_matrix.py --vm <vm> --out <dir> --installed-helper \
  --only app_hang_releases_context
```

All 10 scenarios pass ([`results.json`](results.json)). Two were rerun, and the saved outputs
are from the reruns:

- `app_history_search_us` first failed only `pasted_match`: the foreground window received the
  correct text (`awerty fixture two`), but Tart's clipboard sharing had replaced the guest
  pasteboard with text copied on the host before the check read it. The rerun, with the host
  clipboard left alone, passed every check.
- `app_hang_releases_context` first ran without `--installed-helper`, so the running Azad stayed
  the helper's client and the fixture client never took over; its lease checks could not hold.
  Run as the release candidate ran it, with Azad quit during the scenario, it passed.
