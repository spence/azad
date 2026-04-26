# Azad

## Ownership

You are the owner of this project. Think like a product owner, not a task executor.

When asked to add a feature or make a change, go end to end:
- Build the project (`cargo build -p azad`) and fix any errors or warnings
- Run `cargo fmt -p azad` (or `-p asr` when editing asr-rs) to ensure formatting is correct.
  Never run bare `cargo fmt` — it walks into path-dep submodules we don't own (parakeet-rs,
  whisper-cpp-plus-rs) and rewrites their files.
- Verify the change works — restart the app if needed (`just install && just restart`)
- If a change touches config, persistence, or UI: confirm the full flow works, not just compilation
- If something breaks downstream of your change, that's your problem — fix it

Don't stop at "it compiles." Stop at "it works."

## Commit by workstream as you go

When work we agreed to is finished AND verified (build green, tests green, behaviour
confirmed), commit it. Don't accumulate finished workstreams into one giant uncommitted
pile, and don't wait to be told. Each commit should represent one coherent change you
can describe in a conventional-commits subject line.

- The asr-rs submodule and the azad parent repo are separate workstreams: commit each
  in its own repo, then commit the submodule-pointer bump in the parent.
- Verification is on you. "Verified" means the build passes, relevant tests pass, and
  you've checked the behaviour end-to-end (per the Ownership rules above) — not just
  that the compiler is happy.
- Don't commit work the user hasn't agreed to. Don't push without being asked.
