//! Spotify control for the Hey Spotify connector.
//!
//! Transport and play-by-URI use AppleScript against the desktop Spotify app
//! (`com.spotify.client`). Catalog search uses the `spotify:search:` URL scheme
//! (opens Spotify’s search UI) when we cannot resolve a track URI without OAuth.

use std::process::Command;

/// Bundle id of the macOS Spotify desktop app.
pub const SPOTIFY_BUNDLE_ID: &str = "com.spotify.client";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpotifyClientError {
  AppNotInstalled,
  ScriptFailed(String),
  NoTrack,
  Unsupported(String),
}

impl std::fmt::Display for SpotifyClientError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::AppNotInstalled => {
        write!(f, "Spotify is not installed. Install it from spotify.com, then try again.")
      }
      Self::ScriptFailed(msg) => write!(f, "{msg}"),
      Self::NoTrack => write!(f, "Nothing is playing right now."),
      Self::Unsupported(msg) => write!(f, "{msg}"),
    }
  }
}

/// True if Spotify.app is installed (Launch Services / Applications).
pub fn spotify_app_installed() -> bool {
  // NSWorkspace path via `mdfind` / open -Ra is heavy; use known install locations + mdfind.
  let candidates = [
    "/Applications/Spotify.app",
    &format!(
      "{}/Applications/Spotify.app",
      std::env::var("HOME").unwrap_or_default()
    ),
  ];
  if candidates.iter().any(|p| std::path::Path::new(p).is_dir()) {
    return true;
  }
  // Launch Services query
  let output = Command::new("/usr/bin/mdfind")
    .arg(format!("kMDItemCFBundleIdentifier == '{SPOTIFY_BUNDLE_ID}'"))
    .output();
  match output {
    Ok(out) if out.status.success() => {
      let s = String::from_utf8_lossy(&out.stdout);
      s.lines().any(|l| l.trim().ends_with("Spotify.app"))
    }
    _ => false,
  }
}

fn run_osascript(source: &str) -> Result<String, SpotifyClientError> {
  let output = Command::new("/usr/bin/osascript")
    .args(["-e", source])
    .output()
    .map_err(|e| SpotifyClientError::ScriptFailed(format!("osascript: {e}")))?;
  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    let err = err.trim();
    return Err(SpotifyClientError::ScriptFailed(if err.is_empty() {
      "Spotify did not respond. Is the app open?".into()
    } else {
      err.to_string()
    }));
  }
  Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn ensure_app() -> Result<(), SpotifyClientError> {
  if spotify_app_installed() {
    Ok(())
  } else {
    Err(SpotifyClientError::AppNotInstalled)
  }
}

/// Activate Spotify so Connect / local playback is available.
#[allow(dead_code)] // used when Shazam/play-uri paths expand
pub fn activate_spotify() -> Result<(), SpotifyClientError> {
  ensure_app()?;
  run_osascript(r#"tell application "Spotify" to activate"#).map(|_| ())
}

pub fn play() -> Result<(), SpotifyClientError> {
  ensure_app()?;
  run_osascript(r#"tell application "Spotify" to play"#).map(|_| ())
}

pub fn pause() -> Result<(), SpotifyClientError> {
  ensure_app()?;
  run_osascript(r#"tell application "Spotify" to pause"#).map(|_| ())
}

pub fn play_pause() -> Result<(), SpotifyClientError> {
  ensure_app()?;
  run_osascript(r#"tell application "Spotify" to playpause"#).map(|_| ())
}

pub fn next_track() -> Result<(), SpotifyClientError> {
  ensure_app()?;
  run_osascript(r#"tell application "Spotify" to next track"#).map(|_| ())
}

pub fn previous_track() -> Result<(), SpotifyClientError> {
  ensure_app()?;
  run_osascript(r#"tell application "Spotify" to previous track"#).map(|_| ())
}

pub fn like_current() -> Result<(), SpotifyClientError> {
  // Desktop AppleScript has no first-class "like"; open the track in Spotify as fallback.
  // Prefer telling Spotify to star via menu is fragile — report current and open like in app.
  ensure_app()?;
  Err(SpotifyClientError::Unsupported(
    "Liking tracks from voice isn’t wired yet — use the heart in Spotify for now.".into(),
  ))
}

pub fn current_track() -> Result<String, SpotifyClientError> {
  ensure_app()?;
  let name = run_osascript(r#"tell application "Spotify" to name of current track"#)?;
  let artist = run_osascript(r#"tell application "Spotify" to artist of current track"#)?;
  if name.is_empty() {
    return Err(SpotifyClientError::NoTrack);
  }
  if artist.is_empty() {
    Ok(name)
  } else {
    Ok(format!("{name} — {artist}"))
  }
}

pub fn volume_delta(delta: i32) -> Result<(), SpotifyClientError> {
  ensure_app()?;
  let script = format!(
    r#"tell application "Spotify"
  set v to sound volume + ({delta})
  if v > 100 then set v to 100
  if v < 0 then set v to 0
  set sound volume to v
end tell"#
  );
  run_osascript(&script).map(|_| ())
}

/// Play a Spotify URI (`spotify:track:…`).
#[allow(dead_code)] // Phase 2: Shazam → resolve URI → play
pub fn play_uri(uri: &str) -> Result<(), SpotifyClientError> {
  ensure_app()?;
  if !uri.starts_with("spotify:") {
    return Err(SpotifyClientError::ScriptFailed("Invalid Spotify URI".into()));
  }
  let escaped = uri.replace('\\', "\\\\").replace('"', "\\\"");
  let script = format!(r#"tell application "Spotify" to play track "{escaped}""#);
  run_osascript(&script).map(|_| ())
}

/// Open Spotify’s in-app search for `query` (no Web API client id required).
pub fn open_search(query: &str) -> Result<(), SpotifyClientError> {
  ensure_app()?;
  let encoded: String = query
    .chars()
    .map(|c| match c {
      ' ' => "%20".to_string(),
      c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => c.to_string(),
      c => format!("%{:02X}", c as u8),
    })
    .collect();
  // spotify:search: opens the search UI with the query.
  let url = format!("spotify:search:{encoded}");
  let status = Command::new("/usr/bin/open")
    .arg(&url)
    .status()
    .map_err(|e| SpotifyClientError::ScriptFailed(format!("open: {e}")))?;
  if status.success() {
    Ok(())
  } else {
    Err(SpotifyClientError::ScriptFailed("Could not open Spotify search".into()))
  }
}

/// Play by free-text query: open search and try to play — v1 opens search UI so the user
/// can confirm. Later: rspotify resolve top track → play_uri.
pub fn play_query(query: &str) -> Result<String, SpotifyClientError> {
  open_search(query)?;
  Ok(format!("Opened Spotify search for “{query}”"))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn install_check_does_not_panic() {
    // Pure probe; result depends on machine.
    let _ = spotify_app_installed();
  }
}
