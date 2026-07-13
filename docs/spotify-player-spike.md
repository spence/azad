# Spike: spotify_player as Azad dependency (2026-07-13)

## Crate audited

[aome510/spotify-player](https://github.com/aome510/spotify-player) v0.24.0 (shallow clone).

## Findings

| Item | Result |
|---|---|
| License | MIT |
| Layout | Workspace member `spotify_player` — **binary only** (`src/main.rs`, **no `lib.rs`**) |
| Deps | ratatui, crossterm, rspotify, librespot-*, clap CLI modules under `src/cli/` |
| Library API | **None** published for embedding; modules are private to the binary |

## Decision

**Do not** add `spotify_player` as a Cargo dependency of Azad (would not compile as a library without forking).

**Do:**

1. **AppleScript / Spotify.app** for transport (play/pause/next/prev/volume/current) when `com.spotify.client` is installed.
2. **Optional rspotify** (or Spotify search URI / later Web API) for catalog search + URI resolution.
3. **ShazamKit helper** for identify → then play URI via Spotify.app.

This matches the product gate (Spotify desktop required) and avoids TUI/librespot weight in the menu-bar binary.
