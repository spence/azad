# Numeric key-rate readout — 2026-10-16

Proof for MS-TYPING-READOUT.T1 and MS-TYPING-READOUT.G1.

C1: the readout displays the raw unpaced value `0` instead of “Max”.
Finite readouts, detents, saved values, defaults, accessibility meaning, and insertion
behavior are unchanged. The only production edit is the Swift display string.

All 3 native Swift settings tests passed, including all method/detent combinations,
the fixture-bound readout, UI-event values, and existing layout/model preservation.
Command: `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer swift test --package-path crates/azad-ui --scratch-path target/key-rate-native-proof/swift-ui --jobs 2`.
The windows were prepared offscreen without activation. No synthetic host input was posted.

C2: built the release UI with `--jobs 2`, then ran `just install` with
`AZAD_SKIP_MLX_HELPER=1 CARGO_BUILD_JOBS=2`, followed by one `just restart` and
`just status`. Signature verification passed. Installed identity:

```text
./scripts/azad-dev.sh status
	state = running
	program = /Users/spencer/Applications/Azad.app/Contents/MacOS/azad
	pid = 61180
	job state = running
Identifier=ai.azad
CDHash=9763ec7e665ed50ea28ae310c1e8df299eee0330
TeamIdentifier=35A87BDK48
07ecd6af540f0d740a88efe0c2d6939773afc32358be750a2d4c78a81829792e  /Users/spencer/Applications/Azad.app/Contents/MacOS/libAzadUI.dylib
```

The typing pipeline was not modified or re-exercised through the daily-driver app.
