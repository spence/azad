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

  match search_ranked_tracks(query) {
    Ok(candidates) if !candidates.is_empty() => {
      // Try top hits in order — first URI sometimes fails to switch (region /
      // catalog) while a later remaster plays fine.
      let mut last_err = None;
      for track in candidates.iter().take(5) {
        match play_uri(&track.uri) {
          Ok(()) => {
            thread::sleep(Duration::from_millis(500));
            eprintln!(
              "AZAD_SPOTIFY event=play_uri ok uri={} name={:?} artists={:?} score={}",
              track.uri, track.name, track.artists, track.score
            );
            return Ok(format!("Playing {} — {}", track.name, track.artists));
          }
          Err(e) => {
            eprintln!("AZAD_SPOTIFY event=play_uri fail uri={} err={e}", track.uri);
            last_err = Some(e);
          }
        }
      }
      Err(last_err.unwrap_or_else(|| {
        SpotifyClientError::SearchFailed(format!("Couldn’t play any match for “{query}”."))
      }))
    }
    Ok(_) => {
      eprintln!("AZAD_SPOTIFY event=search_api_miss err=empty");
      play_top_search_result(query)
    }
    Err(api_err) => {
      eprintln!("AZAD_SPOTIFY event=search_api_miss err={api_err}");
      play_top_search_result(query)
    }
  }
}

/// Open Spotify search for `query`, wait for results, press Return to play the
/// highlighted/top hit. Uses Accessibility synthetic keys (not System Events).
///
/// Only reports success if `current track` actually changed — otherwise the UI
/// often still shows the previous song while search is open.
fn play_top_search_result(query: &str) -> Result<String, SpotifyClientError> {
  let before = current_track().ok();
  let _ = run_osascript(r#"tell application "Spotify" to activate"#);
  open_search(query)?;
  thread::sleep(Duration::from_millis(1800));
  if !crate::platform::post_down_then_return() {
    let _ = open_search(query);
    return Err(SpotifyClientError::SearchFailed(format!(
      "Couldn’t start playback for “{query}”. Grant Accessibility if needed, or add \
       spotify.toml (see spotify.example.toml) for catalog search."
    )));
  }
  thread::sleep(Duration::from_millis(900));
  let after = current_track().ok();
  match (&before, &after) {
    (_, Some(now)) if before.as_ref() != after.as_ref() => Ok(format!("Playing {now}")),
    _ => {
      // Don't claim success with the previous track name.
      Err(SpotifyClientError::SearchFailed(format!(
        "Opened Spotify search for “{query}” — pick a result to play."
      )))
    }
  }
}

#[derive(Debug, Clone)]
struct TrackHit {
  uri: String,
  name: String,
  artists: String,
  score: i32,
}

/// Search with several query shapes so ASR quirks don't zero out results.
///
/// Example: “like a stone by audio slave” → fielded `artist:audio slave` is empty,
/// but free-text / collapsed `audioslave` finds the track.
fn search_ranked_tracks(query: &str) -> Result<Vec<TrackHit>, SpotifyClientError> {
  let (title, artist) = split_title_artist(query);
  let token = client_credentials_token()?;
  let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;

  let mut best_by_uri: std::collections::HashMap<String, TrackHit> =
    std::collections::HashMap::new();

  for q in search_query_variants(&title, artist.as_deref(), query) {
    let items = match search_track_items(&client, &token, &q) {
      Ok(items) => items,
      Err(e) => {
        eprintln!("AZAD_SPOTIFY event=search_variant_fail q={q:?} err={e}");
        continue;
      }
    };
    if items.is_empty() {
      eprintln!("AZAD_SPOTIFY event=search_variant_empty q={q:?}");
      continue;
    }
    eprintln!("AZAD_SPOTIFY event=search_variant_hits q={q:?} n={}", items.len());
    for item in items {
      let artists = item.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
      let score = score_track(&title, artist.as_deref(), &item.name, &artists);
      let hit = TrackHit { uri: item.uri.clone(), name: item.name, artists, score };
      best_by_uri
        .entry(item.uri)
        .and_modify(|existing| {
          if hit.score > existing.score {
            *existing = hit.clone();
          }
        })
        .or_insert(hit);
    }
  }

  let mut ranked: Vec<TrackHit> = best_by_uri.into_values().collect();
  ranked.sort_by(|a, b| b.score.cmp(&a.score));
  // Drop near-zero garbage when we have better hits.
  if ranked.iter().any(|h| h.score >= 40) {
    ranked.retain(|h| h.score >= 20);
  }
  if ranked.is_empty() {
    return Err(SpotifyClientError::SearchFailed(format!("No Spotify tracks matched “{query}”.")));
  }
  Ok(ranked)
}

fn search_query_variants(title: &str, artist: Option<&str>, raw: &str) -> Vec<String> {
  let mut out = Vec::new();
  let push = |v: &mut Vec<String>, s: String| {
    let t = s.trim().to_string();
    if !t.is_empty() && !v.iter().any(|x| x == &t) {
      v.push(t);
    }
  };

  if let Some(a) = artist {
    push(&mut out, format!("track:{title} artist:{a}"));
    // “audio slave” → “audioslave”; “ac dc” → “acdc”
    let collapsed = collapse_ws(a);
    if collapsed != a {
      push(&mut out, format!("track:{title} artist:{collapsed}"));
      push(&mut out, format!("{title} {collapsed}"));
    }
    push(&mut out, format!("{title} {a}"));
  }
  push(&mut out, title.to_string());
  push(&mut out, raw.trim().to_string());
  // Free-text without the word “by” often ranks better than field operators.
  if let Some(a) = artist {
    push(&mut out, format!("{title} {a}"));
  }
  out
}

fn search_track_items(
  client: &reqwest::blocking::Client,
  token: &str,
  q: &str,
) -> Result<Vec<TrackItem>, SpotifyClientError> {
  let url =
    format!("https://api.spotify.com/v1/search?type=track&limit=10&q={}", urlencode_component(q));
  let resp = client
    .get(&url)
    .bearer_auth(token)
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
  Ok(parsed.tracks.map(|t| t.items).unwrap_or_default())
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

fn collapse_ws(s: &str) -> String {
  s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn score_track(title_q: &str, artist_q: Option<&str>, name: &str, artists: &str) -> i32 {
  let tq = title_q.to_ascii_lowercase();
  let n = name.to_ascii_lowercase();
  // Prefer the base title before “ - Live …” / “ (Remastered)”
  let n_base = n.split(" - ").next().unwrap_or(&n);
  let n_base = n_base.split(" (").next().unwrap_or(n_base).trim();
  let a = artists.to_ascii_lowercase();
  let mut score = 0i32;
  if n_base == tq || n == tq {
    score += 100;
  } else if n_base.contains(&tq) || tq.contains(n_base) {
    score += 50;
  } else {
    for tok in tq.split_whitespace() {
      if tok.len() > 1 && n.contains(tok) {
        score += 10;
      }
    }
  }
  // Penalize live/remix when the query didn't ask for them.
  if !tq.contains("live") && (n.contains(" - live") || n.contains("(live")) {
    score -= 25;
  }
  if let Some(aq) = artist_q {
    let aq = aq.to_ascii_lowercase();
    let aq_c = collapse_ws(&aq);
    let a_c = collapse_ws(&a);
    if a == aq || a_c == aq_c {
      score += 80;
    } else if a.contains(&aq) || aq.contains(&a) || a_c.contains(&aq_c) || aq_c.contains(&a_c) {
      score += 40;
    } else {
      for tok in aq.split_whitespace() {
        if tok.len() > 1 && a.contains(tok) {
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

  #[test]
  fn score_matches_asr_split_artist() {
    // Spoken “audio slave” should still match catalog “Audioslave”.
    let high = score_track("like a stone", Some("audio slave"), "Like a Stone", "Audioslave");
    let low = score_track("like a stone", Some("audio slave"), "Fuss and Fight", "Koe Wetzel");
    assert!(high > low, "high={high} low={low}");
    assert!(high >= 100);
  }

  #[test]
  fn search_variants_include_collapsed_artist() {
    let v =
      search_query_variants("like a stone", Some("audio slave"), "like a stone by audio slave");
    assert!(v.iter().any(|q| q.contains("audioslave")));
    assert!(v.iter().any(|q| q.contains("track:like a stone")));
  }
}
