# Codex long-text typing — October 28, 2026

## Result and limit

The reported delay has a recipient-side reproduction in Codex 0.163.0.
A 338-character raw input stream completes its writes in less than 1 ms.
The prompt output then changes in groups over approximately one second.
Bracketed paste produces one prompt update within milliseconds.
The same behavior occurs without Azad, Zed, macOS keyboard events, or a remote desktop in the input path.

This reproduces the delayed, grouped output class, not the exact historical native event sequence.
The observations are terminal output, not physical screen frames or Zed input receipts.
The fixture comparison includes no Azad behavior change or global Codex setting change.
The separate fleet configuration update below does not resolve the full typing delay.

## Reported turns and source path

The [log extract](2026-10-28-codex-typing/reported-turns.log) identifies turns 111, 113, and 115 in the current app run.
Their payloads contain 338, 156, and 96 characters. All three use `direct_typing`; their insertion times are 2, 1, and 1 ms, respectively.
The [payload fixture](2026-10-28-codex-typing/reported-payloads.json) preserves the user's text.
The user identified the Codex prompt inside Zed as the destination.

Read-only process ancestry identifies the current client as private Codex under Zed, not a remote client.
Its terminal has 186 columns and 56 rows. The retained final comparisons use those dimensions.
Earlier 1024-column parser controls are not the target-size results; their larger delays are not substituted here.

Azad's Max path calls `send_direct_text_input`, which posts one down/up pair per Unicode scalar.
It does not use the finite-rate typing timer. A main-thread timer replacement is therefore not a demonstrated correction for this report.
Producer completion confirms event submission, not receiver completion.

## Controlled consumer comparison

The [fixture](2026-10-28-codex-typing/codex-input-timing.py) starts an owned, private `--no-daemon` Codex 0.163.0 client in a PTY.
It submits no model prompt and sends no desktop input. The single-line payloads contain only printable characters.
A separate writer process records actual post times, independently from output parsing.
The parser records completed terminal-output frames and reconstructs wrapped display words.
It verifies visible words and punctuation, not invisible whitespace or exact internal buffer bytes.
Frames within one read share the read's timestamp; these are observation times, not separate render deadlines.

The table measures from the first actual input post, excluding Python writer startup:

| 338-character case | First observed text | Complete observed text | Prompt changes |
| --- | ---: | ---: | ---: |
| Default, raw per-character input | 165.4 ms | 935.8 ms | 12 |
| Default, one raw write | 33.4 ms | 33.4 ms | 1 |
| Default, bracketed paste | 51.2 ms | 51.2 ms | 1 |
| Burst detection disabled, raw per-character input | 0.2 ms | 976.7 ms | 271 |
| Burst detection disabled, bracketed paste | 6.2 ms | 6.2 ms | 1 |

Both [default](2026-10-28-codex-typing/codex-private-186x56-default-results.jsonl) and
[burst-off](2026-10-28-codex-typing/codex-private-186x56-burst-off-results.jsonl) captures contain seven scenarios.
All fourteen final visible-content comparisons pass. Each run names its UUID-specific raw capture and exit receipt.
Both private clients exit 0 without forced shutdown. Existing agents and Zed remain untouched.

Raw single-write results vary across retained experiments. Batching unframed bytes is not a verified general prevention.
Finite-rate controls also record actual post intervals; the selected rate is an upper bound, not measured throughput.
In the final captures, nominal 80/s requires approximately five seconds for the long paragraph and still has grouped output.
It is not an instant-insertion solution.

An earlier finite-rate trial failed its visible-content comparison. Its raw capture was overwritten before UUID-specific paths were added.
The partial rows and stderr remain in the external evidence root. That observation is not independent proof of punctuation loss.
The final passing run does not establish universal typing reliability.

Native VM attempts failed the paste positive control with no DOM receipt. They are excluded from native timing conclusions.
The experimental probe flag was removed; the owned VM, tunnel, and keep-awake process were stopped.

## Practical response

Codex documents `tui.disable_paste_burst` as a process-configurable control, with detection enabled by default.
[OpenAI configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)

For a new session, a process-local control is:

```sh
codex -c tui.disable_paste_burst=true
```

In this comparison, the setting removes most initial holding and increases visible updates.
It does not remove the approximately one-second completion cost.
Disabling detection also changes how unframed keyboard input is classified; it is not a universal text-delivery guarantee.
During the fixture comparison, no existing client was restarted and no user configuration file was edited.

The user reports immediate output with Azad Paste. The fixture verifies bracketed-paste behavior only; it does not verify native clipboard delivery through Zed.
A clipboard-independent, paste-like operation would need a supported terminal/Codex literal-text integration.
It is not established by changing Azad's emitter thread or clock precision.
Unicode event chunks remain a separate native compatibility candidate, not a proven fix for this consumer path.

## Verification and retained evidence

This evidence-only change does not affect the installed app. No app rebuild or restart is required.
The final Python fixture passed syntax validation and independent source review.
Review conversation: `conversation-18e2dc903a24b420-576833`; final path review: `cmsg-18e2de99dbc9e538-582181`.
The final burst-off review is recorded in the same conversation.

The fixture requires Python `pyte==0.8.2` and `wcwidth==0.9.4` in its local `python-libs` directory.
Copy the evidence fixture into an owned external evidence target before execution; do not create its output in the checkout.
The pinned Codex executable path and Gateway control belong to this machine's fixture, not a portable application interface.
The final captures and exit receipts are retained. This work did not delete conversation history.
The external evidence root is `/Users/spencer/projects/azad/typing-20261028`, with a keep declaration.

After copying the fixture and payload file into the owned external target, run:

```sh
python3 -m pip install --target python-libs pyte==0.8.2 wcwidth==0.9.4
python3 codex-input-timing.py --private-client
python3 codex-input-timing.py --private-client --disable-burst
```

## Fleet configuration, 2026-10-28

The owner requested the Codex setting on both Macs and prohibited session restarts.
Each Mac now has this entry in `/Users/spencer/.codex/config.toml`:

```toml
[tui]
disable_paste_burst = true
```

| Mac | Setting line | Account paths that read `true` |
| --- | ---: | --- |
| catalyst | 4522 | `.codex`, `.codex-sudo`, `.codex-gmail`, `.codex-micro` |
| catalyst-mini | 354 | `.codex`, `.codex-sudo`, `.codex-gmail`, `.codex-micro` |

On each Mac, all four paths resolve to the same configuration file.
The update added only the setting line. TOML parsing before and after the write confirmed that all other settings were unchanged.
Separate read-only commands parsed all account paths again and confirmed `true`.
The account links, file permissions, and file inode were preserved. No profile override was found.
Read-only process snapshots before and after each write matched for existing Codex and Zed processes.
No session restart, process termination, desktop input, or Azad deployment command was used.
The saved configuration is for future Codex sessions; this evidence does not claim that an existing TUI loaded the new value.

Azad's far-right key rate, `0`, already means no added character delay.
It is separate from the Codex input-burst setting. No extra slider position or app change was necessary.
The controlled comparison above supports faster initial display, not faster completion of the full text.

The official Claude Code [settings reference](https://code.claude.com/docs/en/settings-reference) and
[environment-variable reference](https://code.claude.com/docs/en/env-vars) have no documented equivalent to this Codex control.
Its [terminal guide](https://code.claude.com/docs/en/terminal-config#paste-large-content) describes collapsed large pastes, not a switch to disable input-burst detection.
No Claude Code setting was changed.
