//! Spotify control for the Hey Spotify connector.
//!
//! Transport and play-by-URI use AppleScript against the desktop Spotify app
//! (`com.spotify.client`). Catalog search prefers the Spotify Web API (client
//! credentials) when credentials are available via env or a local TOML file
//! (`spotify.toml` — see `spotify.example.toml`). Without credentials, falls
//! back to opening in-app search and committing the top hit with a synthetic
//! Return key (same Accessibility path as paste).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

/// Bundle id of the macOS Spotify desktop app.
pub const SPOTIFY_BUNDLE_ID: &str = "com.spotify.client";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpotifyClientError {
  AppNotInstalled,
  ScriptFailed(String),
  NoTrack,
  SearchFailed(String),
  Unsupported(String),
}

impl std::fmt::Display for SpotifyClientError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::AppNotInstalled => {
        write!(f, "Spotify is not installed. Install it from spotify.com, then try again.")
      }
      Self::ScriptFailed(msg) | Self::SearchFailed(msg) | Self::Unsupported(msg) => {
        write!(f, "{msg}")
      }
      Self::NoTrack => write!(f, "Nothing is playing right now."),
    }
  }
}

/// True if Spotify.app is installed (Launch Services / Applications).
pub fn spotify_app_installed() -> bool {
  let candidates = [
    "/Applications/Spotify.app",
    &format!("{}/Applications/Spotify.app", std::env::var("HOME").unwrap_or_default()),
  ];
  if candidates.iter().any(|p| std::path::Path::new(p).is_dir()) {
    return true;
  }
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
  if spotify_app_installed() { Ok(()) } else { Err(SpotifyClientError::AppNotInstalled) }
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
  if artist.is_empty() { Ok(name) } else { Ok(format!("{name} — {artist}")) }
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
pub fn play_uri(uri: &str) -> Result<(), SpotifyClientError> {
  ensure_app()?;
  if !uri.starts_with("spotify:") {
    return Err(SpotifyClientError::ScriptFailed("Invalid Spotify URI".into()));
  }
  let escaped = uri.replace('\\', "\\\\").replace('"', "\\\"");
  // Activate so the desktop client is the active Connect target, then play.
  let script = format!(
    r#"tell application "Spotify"
  activate
  play track "{escaped}"
end tell"#
  );
  run_osascript(&script).map(|_| ())
}

/// Open Spotify’s in-app search for `query`.
pub fn open_search(query: &str) -> Result<(), SpotifyClientError> {
  ensure_app()?;
  let encoded = urlencode_component(query);
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

/// Resolve a free-text query to a track and play it in Spotify.app.
///
/// Parses optional “title by artist” form, searches the Spotify catalog (Web API
/// client-credentials when credentials are available), scores candidates, then
/// `play track` via AppleScript. Without API credentials, opens in-app search
/// and presses Return to start the top result.
pub fn play_query(query: &str) -> Result<String, SpotifyClientError> {
  ensure_app()?;
  let query = query.trim();
  if query.is_empty() {
    return Err(SpotifyClientError::SearchFailed("Empty play query".into()));
  }

  match search_best_track(query) {
    Ok(track) => {
      play_uri(&track.uri)?;
      // Brief settle so name-of-current-track is reliable.
      thread::sleep(Duration::from_millis(400));
      Ok(format!("Playing {} — {}", track.name, track.artists))
    }
    Err(api_err) => {
      eprintln!("AZAD_SPOTIFY event=search_api_miss err={api_err}");
      play_top_search_result(query)
    }
  }
}

/// Open Spotify search for `query`, wait for results, press Return to play the
/// highlighted/top hit. Uses Accessibility synthetic keys (not System Events).
fn play_top_search_result(query: &str) -> Result<String, SpotifyClientError> {
  // Activate + open search so the desktop client is focused for the keystroke.
  let _ = run_osascript(r#"tell application "Spotify" to activate"#);
  open_search(query)?;
  // Search UI needs a beat to populate the first row; Down moves off the
  // search field onto the top track before Return starts playback.
  thread::sleep(Duration::from_millis(1800));
  if !crate::platform::post_down_then_return() {
    let _ = open_search(query);
    return Err(SpotifyClientError::SearchFailed(format!(
      "Couldn’t start playback for “{query}”. Grant Accessibility if needed, or add \
       spotify.toml (see spotify.example.toml) for catalog search."
    )));
  }
  thread::sleep(Duration::from_millis(900));
  match current_track() {
    Ok(t) => Ok(format!("Playing {t}")),
    Err(_) => Ok(format!("Playing “{query}”")),
  }
}

#[derive(Debug, Clone)]
struct TrackHit {
  uri: String,
  name: String,
  artists: String,
  score: i32,
}

fn search_best_track(query: &str) -> Result<TrackHit, SpotifyClientError> {
  let (title, artist) = split_title_artist(query);
  let q = build_search_q(&title, artist.as_deref());
  let token = client_credentials_token()?;
  let url =
    format!("https://api.spotify.com/v1/search?type=track&limit=10&q={}", urlencode_component(&q));
  let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  let resp = client
    .get(&url)
    .bearer_auth(&token)
    .send()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  if !resp.status().is_success() {
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    return Err(SpotifyClientError::SearchFailed(format!(
      "Spotify search failed ({status}): {}",
      body.chars().take(120).collect::<String>()
    )));
  }
  let parsed: SearchResponse =
    resp.json().map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  let items = parsed.tracks.map(|t| t.items).unwrap_or_default();
  if items.is_empty() {
    return Err(SpotifyClientError::SearchFailed(format!("No Spotify tracks matched “{query}”.")));
  }
  let mut best: Option<TrackHit> = None;
  for item in items {
    let artists = item.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
    let score = score_track(&title, artist.as_deref(), &item.name, &artists);
    let hit = TrackHit { uri: item.uri, name: item.name, artists, score };
    if best.as_ref().map(|b| hit.score > b.score).unwrap_or(true) {
      best = Some(hit);
    }
  }
  best.ok_or_else(|| SpotifyClientError::SearchFailed("No track candidates".into()))
}

fn split_title_artist(query: &str) -> (String, Option<String>) {
  // “butter by bts” / “Butter by BTS”
  let lower = query.to_ascii_lowercase();
  if let Some(idx) = lower.rfind(" by ") {
    let title = query[..idx].trim().to_string();
    let artist = query[idx + 4..].trim().to_string();
    if !title.is_empty() && !artist.is_empty() {
      return (title, Some(artist));
    }
  }
  (query.trim().to_string(), None)
}

fn build_search_q(title: &str, artist: Option<&str>) -> String {
  match artist {
    Some(a) => format!("track:{title} artist:{a}"),
    None => title.to_string(),
  }
}

fn score_track(title_q: &str, artist_q: Option<&str>, name: &str, artists: &str) -> i32 {
  let tq = title_q.to_ascii_lowercase();
  let n = name.to_ascii_lowercase();
  let a = artists.to_ascii_lowercase();
  let mut score = 0i32;
  if n == tq {
    score += 100;
  } else if n.contains(&tq) || tq.contains(&n) {
    score += 50;
  } else {
    // token overlap
    for tok in tq.split_whitespace() {
      if n.contains(tok) {
        score += 10;
      }
    }
  }
  if let Some(aq) = artist_q {
    let aq = aq.to_ascii_lowercase();
    if a == aq {
      score += 80;
    } else if a.contains(&aq) || aq.contains(&a) {
      score += 40;
    } else {
      for tok in aq.split_whitespace() {
        if a.contains(tok) {
          score += 15;
        }
      }
    }
  }
  score
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
  tracks: Option<TracksPage>,
}

#[derive(Debug, Deserialize)]
struct TracksPage {
  items: Vec<TrackItem>,
}

#[derive(Debug, Deserialize)]
struct TrackItem {
  name: String,
  uri: String,
  artists: Vec<ArtistItem>,
}

#[derive(Debug, Deserialize)]
struct ArtistItem {
  name: String,
}

struct CachedToken {
  access_token: String,
  expires_at: Instant,
}

static TOKEN_CACHE: Mutex<Option<CachedToken>> = Mutex::new(None);

fn client_credentials() -> Result<(String, String), SpotifyClientError> {
  if let Some(pair) = credentials_from_env() {
    return Ok(pair);
  }
  if let Some(pair) = credentials_from_toml_files() {
    return Ok(pair);
  }
  Err(SpotifyClientError::SearchFailed(
    "No Spotify API credentials. Copy spotify.example.toml → spotify.toml \
     (or ~/Library/Application Support/Azad/spotify.toml), or set \
     AZAD_SPOTIFY_CLIENT_ID / AZAD_SPOTIFY_CLIENT_SECRET."
      .into(),
  ))
}

fn credentials_from_env() -> Option<(String, String)> {
  let id = std::env::var("AZAD_SPOTIFY_CLIENT_ID")
    .or_else(|_| std::env::var("SPOTIPY_CLIENT_ID"))
    .ok()?;
  let secret = std::env::var("AZAD_SPOTIFY_CLIENT_SECRET")
    .or_else(|_| std::env::var("SPOTIPY_CLIENT_SECRET"))
    .ok()?;
  if id.is_empty() || secret.is_empty() {
    return None;
  }
  Some((id, secret))
}

/// Search order for TOML credentials (first readable valid file wins):
/// 1. `AZAD_SPOTIFY_CONFIG` (explicit path)
/// 2. `~/Library/Application Support/Azad/spotify.toml` (installed app)
/// 3. `./spotify.toml` (cwd — useful in dev)
#[derive(Debug, Deserialize)]
struct SpotifyTomlCreds {
  client_id: String,
  client_secret: String,
}

fn credentials_from_toml_files() -> Option<(String, String)> {
  for path in credential_toml_candidates() {
    if let Some(pair) = read_spotify_toml(&path) {
      eprintln!("AZAD_SPOTIFY event=credentials_loaded path={}", path.display());
      return Some(pair);
    }
  }
  None
}

fn credential_toml_candidates() -> Vec<PathBuf> {
  let mut paths = Vec::new();
  if let Ok(explicit) = std::env::var("AZAD_SPOTIFY_CONFIG") {
    let p = PathBuf::from(explicit.trim());
    if !p.as_os_str().is_empty() {
      paths.push(p);
    }
  }
  if let Ok(home) = std::env::var("HOME") {
    paths.push(PathBuf::from(home).join("Library/Application Support/Azad/spotify.toml"));
  }
  if let Ok(cwd) = std::env::current_dir() {
    paths.push(cwd.join("spotify.toml"));
  }
  paths
}

fn read_spotify_toml(path: &Path) -> Option<(String, String)> {
  let raw = std::fs::read_to_string(path).ok()?;
  let creds: SpotifyTomlCreds = toml::from_str(&raw).ok()?;
  if creds.client_id.is_empty() || creds.client_secret.is_empty() {
    return None;
  }
  Some((creds.client_id, creds.client_secret))
}

fn client_credentials_token() -> Result<String, SpotifyClientError> {
  {
    let guard = TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = guard.as_ref() {
      if Instant::now() < cached.expires_at {
        return Ok(cached.access_token.clone());
      }
    }
  }
  let (id, secret) = client_credentials()?;
  let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  let resp = client
    .post("https://accounts.spotify.com/api/token")
    .basic_auth(&id, Some(&secret))
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body("grant_type=client_credentials")
    .send()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  if !resp.status().is_success() {
    return Err(SpotifyClientError::SearchFailed(format!(
      "Spotify auth failed ({})",
      resp.status()
    )));
  }
  #[derive(Deserialize)]
  struct TokenResp {
    access_token: String,
    expires_in: u64,
  }
  let token: TokenResp =
    resp.json().map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  let expires_at = Instant::now() + Duration::from_secs(token.expires_in.saturating_sub(60));
  if let Ok(mut guard) = TOKEN_CACHE.lock() {
    *guard = Some(CachedToken { access_token: token.access_token.clone(), expires_at });
  }
  Ok(token.access_token)
}

fn urlencode_component(s: &str) -> String {
  let mut out = String::with_capacity(s.len() * 2);
  for b in s.as_bytes() {
    match *b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
        out.push(*b as char);
      }
      b' ' => out.push_str("%20"),
      _ => out.push_str(&format!("%{b:02X}")),
    }
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn install_check_does_not_panic() {
    let _ = spotify_app_installed();
  }

  #[test]
  fn split_title_artist_parses_by() {
    let (t, a) = split_title_artist("butter by BTS");
    assert_eq!(t.to_ascii_lowercase(), "butter");
    assert_eq!(a.unwrap().to_ascii_lowercase(), "bts");
  }

  #[test]
  fn score_prefers_exact_artist() {
    let high = score_track("butter", Some("bts"), "Butter", "BTS");
    let low = score_track("butter", Some("bts"), "Butter", "Someone Else");
    assert!(high > low);
  }

  #[test]
  fn toml_creds_parse() {
    let raw = r#"
client_id = "abc123"
client_secret = "secret456"
"#;
    let c: SpotifyTomlCreds = toml::from_str(raw).unwrap();
    assert_eq!(c.client_id, "abc123");
    assert_eq!(c.client_secret, "secret456");
  }
}
