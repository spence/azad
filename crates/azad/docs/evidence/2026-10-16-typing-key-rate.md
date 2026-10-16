# Typing key rate verification — 2026-10-16

Proof for MS-TYPING-KEY-RATE.T1 and MS-TYPING-KEY-RATE.G1.

## Scope and review

The approved [design](../typing-key-rate-design.md) is implemented: one shared
AzadTypingKeyRate preference and a conditional native General-settings slider with
20/40/80/160/320/Max stops. Max remains the default. General alone uses the narrower
leading inset and label allocation; the shared FormRow and other tabs are unchanged.

Finite typing advances one complete Unicode-scalar pair at a time on a dedicated
one-shot main-run-loop timer. The controller serializes overlapping insertion snapshots
and finishes copy, submit, history, and original-overlay cleanup only on completion.
Escape/shutdown discard pending output; handled source identities prevent late retries.

Independent implementation review found and then cleared the history-overlap defects:
all insertion events use the existing synthetic marker, history search stays tap-driven
without native key-window ownership, and actual history navigation restores search capture.
Idle Max/Paste timing, terminal fallback, key bindings, Secure Input behavior, and ASR are
not redesigned. The existing synchronous history failure/submit behavior is preserved.

## C1 — setting, layout, and default paths

- Swift native controls: 3 tests passed, including all 18 method/detent combinations,
  emitted UI-event values, conditional visibility, shared model-copy preservation,
  complete row/control bounds, no overlaps or clipped rate readout, and unchanged
  Text-tab alignment/inset.
- Offscreen AppKit reports row ambiguity in the unchanged Text-tab FormRow baseline.
  General introduces no additional ambiguity relative to that component. No shared
  layout repair was bundled into this feature.
- Rust tests verify allowed/default/unsupported preferences, malformed UI events,
  camelCase settings serialization, and the shared value through method switches.
- Rust tests retain synchronous Max and Paste, including the original synchronous
  history failure behavior. The existing terminal-bundle fallback tests still pass.
- The VM probe round-trips the real NSUserDefaults integer preference in its own process.

Commands from the repository root:

```sh
HOME="$PWD/target/key-rate-native-proof/test-home" \
CARGO_HOME=/Users/spencer/.cargo RUSTUP_HOME=/Users/spencer/.rustup \
CARGO_BUILD_JOBS=2 cargo test -q -p azad -- --test-threads=1
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
swift test --package-path crates/azad-ui \
  --scratch-path target/key-rate-native-proof/swift-ui --jobs 2
```

The full app suite passed 303 tests, with one pre-existing live Spotify test ignored.
The final focused insertion run passed all 9 tests after tightening no-op history ownership.
Rust controller tests do not load the native UI library; Swift tests exercise the actual
native controls without activating or showing a window.

## C2 — paced production logic and native receiver

Production cursor/controller tests cover Unicode, zero/one/many scalars, N−1 waits,
no first/last delay, early/late callbacks without bursts, FIFO ordering, rate/method/submit
snapshots, normal/raw/history completion, old-completion/new-speech ownership,
copy/submit order, partial failure → SessionEnded, cancel → late FinalText,
stale callbacks, and continued history navigation/search.

A real marked pair is constructed and passed through the production tap callback with
history capture enabled, without posting input. Both down and up pass through unchanged.

```sh
CARGO_BUILD_JOBS=2 cargo clippy -q -p azad --bin azad --example typing-probe -- -D warnings
(cd crates/azad && CARGO_BUILD_JOBS=2 just interaction-test)
cargo fmt -p azad --check
git diff --check
```

Clippy/format/diff checks passed. All 7 isolated interaction scenarios passed.

The VM-only [typing-probe](../../examples/typing-probe.rs) imports production paste.rs,
typing.rs, and preferred_store.rs, not their cfg(test) early-success behavior. It
posts into an owned AppKit text receiver beside a non-key history-like panel, checks
the actual received Unicode/newline and event marker, checks clipboard copy only after
typing, and runs a separate 10 ms heartbeat. It refuses to run outside VirtualMac before
accessing preferences, clipboard, or AppKit; the host refusal exited as expected.

Native checks ran in a disposable macOS 26.6.2 VM, with audio and clipboard sharing
disabled. Sample: “Azad café é 🧠 👩🏽‍💻” followed by auto-submit Enter.

| Method | Selected rate | Minimum spacing span (ms) | Received span (ms) | Heartbeat ticks |
| --- | ---: | ---: | ---: | ---: |
| Type | 20 | 900 | 1026 | 126 |
| Type and copy | 80 | 225 | 365 | 65 |
| Type and copy | 320 | 56.25 | 219 | 53 |
| Type and copy | Max | 0 | 89 | 27 |

All four cases passed: exact text plus one trailing newline, correct clipboard policy,
Azad markers on received keydowns, a non-key panel, and unchanged empty search text.
A final 80/s Type-and-copy run with the receiver menu present also passed (341 ms, 67 ticks).

The ordinary Paste probe at finite rate 80 also passed, with the complete text and newline,
no per-character delay, and the complete clipboard value. Its receiver needed a normal
Edit→Paste menu; the initial menu-less fixture received only Enter. Adding that menu to
the fixture fixed the check without changing Azad paste timing or delivery code.

This is evidence for the tested receiver and production event recipe, not an all-app
compatibility guarantee or a claim that the separate intermittent clipboard report is fixed.
The VM does not run the full speech/model pipeline.

## C3 — live release deployment

The signed release was built first in a separate bundle, then installed with the normal
installer. The MLX helper was not rebuilt (AZAD_SKIP_MLX_HELPER=1); its code-directory
identity matched before and after the install. Deep signing refreshes signature bytes.

```sh
AZAD_SKIP_MLX_HELPER=1 CARGO_BUILD_JOBS=2 \
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
  just install
just restart
just status
```

There was one live restart, after the isolated checks. Status reports ai.azad running
as PID 57733; startup logs report Accessibility, Microphone, and Input Monitoring granted
and the tap installed. The mapped executable inode matches the installed executable.
codesign --verify --deep --strict passed. Installed identity captured directly:

```text
32e8c5a3a9216c8e5e5d1e6d87fd8274b658afd7270b812a759091841dd5c875  /Users/spencer/Applications/Azad.app/Contents/MacOS/azad
b078dced796ed296bea3381418ba89adeff123e23691c202af5aa5309adc17e1  /Users/spencer/Applications/Azad.app/Contents/MacOS/libAzadUI.dylib
Identifier=ai.azad
CDHash=63c9c553a64f0931ea19426617c259b674813ed5
TeamIdentifier=35A87BDK48
57733     1 01:15 /Users/spencer/Applications/Azad.app/Contents/MacOS/azad
azad    57733 spencer  txt    REG   1,16   6337808           452694439 /Users/spencer/Applications/Azad.app/Contents/MacOS/azad
452694439 /Users/spencer/Applications/Azad.app/Contents/MacOS/azad
```

The installed app, its microphone/device/prefs, and the user's desktop were not test
controls. No synthetic events were posted to the host. Runtime verification was limited
to deployment, status, signatures, process mapping, and read-only startup logs.

The owned test VM was stopped and deleted, its lease released, and its private Tart home
removed. The cached base image and all other VMs/agents were left untouched.
