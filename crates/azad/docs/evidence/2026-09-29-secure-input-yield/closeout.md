# Secure Input yield: ownership handback verification

Verified on catalyst, 2026-09-29, against `a3ff536`. This is a verification record;
Burndown remains the authority for work state.

## Accepted direction and ownership

`azad-hotkeys` handed ownership back to `azad-root`, confirmed no outstanding claims or
writes, and supplied its evidence and omissions. Gateway conversation:
`conversation-18d9f8603930ff00-44`; correlated reply:
`cmsg-18d9f86453d72ec0-47-outcome`.

The owner direction already recorded in `OBJ-SECURE-INPUT-YIELD`, and corroborated by
that handoff, is: "i dont want azad to work while secure input is open", followed by
"all right, get it done". The handoff also preserves the paste instruction:
"we're just not participating we're explicitly just not doing anything about secure
input and you know leaving it up to mac to do the right thing". Paste is unchanged.
The current behavior contract is in [the app specification](../../../SPECIFICATION.md).

`OBJ-RELIABLE-HOTKEYS` records a historical device-capture delivery, subsequently
reverted. Its original criteria and owner exceptions remain intact:
`GDISP-01M3QA2PZCPZYANYX7VSMQZCC4` and `GDISP-01M3QA2PZN3CJCD37N7GC2GYBM`.
Its context now explicitly names `OBJ-SECURE-INPUT-YIELD` as superseding the Secure Input
priority requirement and device-capture design. Neither the old criteria nor those
exceptions describe the currently installed architecture. Historical capture evidence
remains accessible at its registered Git revision even where its path is absent at HEAD.

## Independent verification

- `cargo test -q -p azad`: 287 passed, one ignored, zero failures.
- `just interaction-test`: all seven process-local scenarios passed.
- `git diff 3883996 a3ff536 -- crates/azad/src crates/azad/Cargo.toml`: empty.
- `just status`: `ai.azad` running, PID 21604, at
  `/Users/spencer/Applications/Azad.app/Contents/MacOS/azad`.
- Installed identity: team `35A87BDK48`, CDHash
  `7e0ec1937881bbeee871e0238f8df8d534bda954`; executable SHA-256
  `b3595b4584765892f68f6f0022d5647d92303bf860a22f07580d9f7520ba5fde`.
  This matches the [recorded VM build](README.md) and the handing agent's artifact.
- Raw committed VM records show Option+Space claimed before and after Secure Input;
  the secure phase has no Azad actions and the foreground sink receives Space.
- The installed bundle has no capture helper or `Contents/Library`; the capture launchd
  job and known helper/plist paths are absent. `hidutil list` has no Azad/0xfeed virtual
  keyboard. `systemextensionsctl list` shows only Tailscale, no pqrs driver. The pqrs
  application-support directory and package receipt are absent.
- Repository search found no device-capture implementation or installer integration.
  The interaction-test script retains forbidden API names as safety checks.

The saved guest `run.sh` now refuses non-`VirtualMac` hardware before its first mutation.
`bash -n` passed; running its refusal path on catalyst exited 64, and shell tracing showed
only the hardware query, refusal message and exit. No desktop input was generated.
The original VM results are retained at `a3ff536`; this audit did not rerun that VM.

## Research retirement and residual observations

The stopped, session-owned VM and scratch trees were moved to recoverable Reap quarantine,
not purged: `d7b4d576` (SIP research VM), `2bed1736` (hotkey probe), `dc617a38`
(nested driver source), and `4e8e0f4f` (HID research tree). Other agents' VMs and leases
were not changed.

The macOS background-items database retains a capture-helper registration pointing at
absent bundle files. It is not a running helper. No global background-items reset or
host reboot was performed. The driver is absent from the current extension inventory;
an additional post-reboot check was not performed or claimed.

Physical F1/F2 brightness keys were not exercised in this audit. Native routing follows
from removing the device-seizing path, not a new physical-key test. The OS-level VM proof
covers Option+Space; other gestures have process-local regression evidence, not a fresh
full OS matrix. This does not claim priority over a consuming competing event tap.
Read-only logs also show existing `stale_latency` tap recreations through generation 75;
their cause was not investigated here and they are not proof of universal hotkey reliability.

No production source, running application, permissions, keyboard settings, or foreign
agent processes were changed during handback verification. Application deployment was
already complete and its identity was verified rather than restarted for a docs/test-guard change.
