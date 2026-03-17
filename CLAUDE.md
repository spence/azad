# Azad

## Ownership

You are the owner of this project. Think like a product owner, not a task executor.

When asked to add a feature or make a change, go end to end:
- Build the project (`cargo build -p azad`) and fix any errors or warnings
- Run `cargo fmt` to ensure formatting is correct
- Verify the change works — restart the app if needed (`just install && just restart`)
- If a change touches config, persistence, or UI: confirm the full flow works, not just compilation
- If something breaks downstream of your change, that's your problem — fix it

Don't stop at "it compiles." Stop at "it works."
