# Shortcuts stand down during Secure Input

Owner rule, 2026-09-29: "i dont want azad to work while secure input is open." Azad therefore
captures shortcuts only through its event tap, which macOS blinds while any app holds Secure
Input. This record checks the installed build against that rule. Nothing ran on the host desktop.

## Build under test

- The `Azad.app` that `just install` built at `3883996` and installed on catalyst: `ai.azad`,
  team `35A87BDK48`, CDHash `7e0ec1937881bbeee871e0238f8df8d534bda954`.
- The same bundle was copied unchanged into the SIP-enabled VM `azad-capture-1790666805`
  (macOS 26.6.2). [`run/guest-identity.txt`](run/guest-identity.txt) shows the same CDHash there.
- In the guest it reported `accessibility=Granted input_monitoring=Granted` and
  `AZAD_HOTKEY_TAP action=installed`.

## Method

[`run.sh`](run.sh) runs three phases with a foreground key-recording window (`Sink.app`):

1. Secure Input off: [`hold.json`](hold.json), an Option+Space hold of 700 ms.
2. Secure Input on: [`secure-hold.json`](secure-hold.json), the same hold. The source process
   turns Secure Input on before the first report and off after the last.
3. Secure Input off again: the same hold as phase 1.

Supporting details:

- **Keyboard:** a guest-only virtual source keyboard (Karabiner DriverKit VirtualHIDDevice)
  posts the keys as HID input.
- **Fixtures:** `fixture-keyboard` and `Sink.app` are built from
  `crates/azad-capture/fixtures` at `e0364b9`.
- **Secure Input holder:** each phase samples `kCGSSessionSecureInputPID` from the console
  session.

## Result

| Phase | Secure Input (source reports / console holder) | Azad received | Foreground window received |
|---|---|---|---|
| before | off / none | `hotkey_pressed`, `hotkey_released` | nothing (Space claimed) |
| secure | on for every key report / PID 894, seen only in this phase | nothing | Space down with Option, auto-repeat, then Space up: delivered unclaimed |
| after | off / none | `hotkey_pressed`, `hotkey_released` | nothing (Space claimed) |

While another process held Secure Input, Azad received and suppressed nothing, and the keys went
to the foreground app as if Azad were absent. Once it was released, shortcuts worked again. Raw
outputs are in [`run/`](run/).

## Host

[`host.txt`](host.txt) holds read-only checks on catalyst:

- the tested CDHash is running;
- its tap is installed;
- no capture helper process runs, and none is in the bundle;
- no Azad virtual keyboard is present.

The tap recreated itself once after a stall (`stale_latency`, generation 2 -> 3), which is the
existing self-healing path.
