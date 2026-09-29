# Physical keyboard session, first run

Owner-attended run of `tests/physical/session.py` on catalyst (MacBookPro18,4, macOS 26.5,
SIP custom configuration: debugging restrictions disabled, filesystem and kernel integrity
protections enabled). The installed build was app CDHash
`7d2063098eef24e0b643fce4b30a32e826818853` and helper CDHash
`ebb686bdfd9a87c6c927162fc63cf06b3e09bb53`, as recorded in [`report.json`](report.json)
([VM smoke of the same bundle](../2026-09-29-installed-build/)). The helper seized the built-in
keyboard (SPI), the Keychron Q6 HE (USB, with its pointer collection) and two keyboard
interfaces of a Razer DeathAdder V3 Pro mouse.

`report.json` shows `passed: false`. Its results fall into three groups.

## Void: every check that reads the test window

The test window crashed at launch. The session passed it a relative output path, `open`
starts applications in `/`, and the first write through the resulting null file segfaulted.
With no window recording, every foreground check is void. That includes the ones that read
`true` only because nothing was recorded: `space_not_delivered`,
`claimed_keys_not_delivered` and `search_keys_not_delivered`. Also void:
`shift_return_delivered_once`, `each_key_once`, `typing_delivered_once`, `no_stuck_modifier`,
`home_delivered`, `caps_applied_then_released`, `repeats` and `w_delivered_once`.

The tool now fails loudly instead (`6bc2b59`) and was validated end to end in the VM.

## Not run

- **Consuming tap:** `tap_failed`. The owner's terminal may not install an event tap, so no
  competing tap ran.
- **Sleep and wake:** no sleep happened; `kern.sleeptime` stayed at the previous night's value.
  With an external display attached, closing the lid does not sleep the Mac.
- **Caps Lock LED, Touch ID/power button, Keychron mouse keys:** skipped by the owner.

## Valid

These are from Azad's own input log, independent of the test window. The owner answered yes
for all four captured devices and noted that the steps may not all have been followed exactly,
so the per-keyboard labels are the session's prompts, not verified sources.

| Keyboard (as prompted) | Result |
|---|---|
| Built-in | Option+Space hold -> `hotkey_pressed`, `hotkey_released`; double-tap toggled listening; history search -> `arrow_navigate(-1)`, two `history_search_edit`, `overlay_cancel`; with Secure Input on (`enabled:1`), Option+Space -> `hotkey_pressed`, `hotkey_released` |
| Keychron Q6 HE | Hold, double-tap and double-tap back all worked; history search as above; the overlay step claimed Down (`arrow_navigate(+1)`); ordinary typing produced no Azad events; Option+Space worked with Secure Input on |
| Third run (Razer label) | Hold, both double-taps and history search worked; Down and Escape claimed. |

Two built-in steps are inconclusive. "Double tap back" registered no events, which left Always
Listening switched from its starting value. The overlay step recorded only `hotkey_pressed`, and
its release arrived during the next step.

The owner confirmed that F1/F2 brightness, F11/F12 volume and the Globe key behaved as usual
on the built-in keyboard, and that the Keychron's media keys worked.

The helper's content-free counters at the end of the session, since capture started:

- 922 unclaimed key edges forwarded, with 0 forward errors;
- 94 claimed edges;
- 75 actions delivered to Azad;
- 0 rejected clients.

The owner's messages in the working session during and after this run were typed through the
capture path.
