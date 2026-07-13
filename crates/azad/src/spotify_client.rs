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

/// `spotify:track:…` for the currently playing track.
pub fn current_track_uri() -> Result<String, SpotifyClientError> {
  ensure_app()?;
  let uri = run_osascript(r#"tell application "Spotify" to spotify url of current track"#)?;
  if uri.starts_with("spotify:track:") {
    Ok(uri)
  } else if uri.is_empty() {
    Err(SpotifyClientError::NoTrack)
  } else {
    // Some builds return https open.spotify.com links — normalize if we can.
    if let Some(id) = uri.rsplit('/').next().filter(|s| s.len() == 22) {
      return Ok(format!("spotify:track:{id}"));
    }
    Err(SpotifyClientError::NoTrack)
  }
}

pub fn player_position_secs() -> Result<f64, SpotifyClientError> {
  ensure_app()?;
  let raw = run_osascript(r#"tell application "Spotify" to player position"#)?;
  raw
    .parse::<f64>()
    .map_err(|_| SpotifyClientError::ScriptFailed(format!("Bad player position: {raw}")))
}

pub fn set_player_position_secs(secs: f64) -> Result<(), SpotifyClientError> {
  ensure_app()?;
  let secs = secs.max(0.0);
  let script = format!(r#"tell application "Spotify" to set player position to {secs}"#);
  run_osascript(&script).map(|_| ())
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

/// Play a Spotify URI (`spotify:track:…`, playlist, album, radio, …).
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

/// Play Spotify’s official **This Is {Artist}** editorial playlist.
///
/// Official “This Is …” lists are Spotify editorial (`37i9d…` IDs). The Web API
/// search endpoint no longer returns them under client-credentials, so we:
/// 1. Resolve the artist via search
/// 2. Read the public artist page HTML for the linked This Is playlist
/// 3. Fall back to catalog playlist search (exact title), then artist-context play
pub fn play_artist_this_is(artist: &str) -> Result<String, SpotifyClientError> {
  ensure_app()?;
  let artist = artist.trim();
  if artist.is_empty() {
    return Err(SpotifyClientError::SearchFailed("Empty artist".into()));
  }

  let resolved = resolve_artist(artist)?;
  eprintln!(
    "AZAD_SPOTIFY event=artist_resolved spoken={artist:?} name={:?} id={}",
    resolved.name, resolved.id
  );

  // 1) Official editorial This Is from the artist page (preferred).
  if let Some(uri) = scrape_this_is_playlist_uri(&resolved.id, &resolved.name) {
    eprintln!("AZAD_SPOTIFY event=this_is_official uri={uri} artist={:?}", resolved.name);
    play_uri(&uri)?;
    thread::sleep(Duration::from_millis(400));
    return Ok(format!("Playing This Is {}", resolved.name));
  }

  // 2) Catalog search for an exact-titled “This Is …” playlist (limit ≤ 10).
  let playlist_q = format!("This Is {}", resolved.name);
  let prefer = format!("this is {}", resolved.name.to_ascii_lowercase());
  match search_best_playlist(&playlist_q, Some(&prefer)) {
    Ok(pl) if pl.score >= 100 => {
      eprintln!(
        "AZAD_SPOTIFY event=this_is_catalog uri={} name={:?} score={}",
        pl.uri, pl.name, pl.score
      );
      play_uri(&pl.uri)?;
      thread::sleep(Duration::from_millis(400));
      return Ok(format!("Playing {}", pl.name));
    }
    Ok(pl) => {
      eprintln!(
        "AZAD_SPOTIFY event=this_is_weak uri={} name={:?} score={}",
        pl.uri, pl.name, pl.score
      );
    }
    Err(e) => {
      eprintln!("AZAD_SPOTIFY event=this_is_search_miss artist={:?} err={e}", resolved.name);
    }
  }

  // 3) Continuous artist context in the desktop app (not a single track).
  let artist_uri = format!("spotify:artist:{}", resolved.id);
  play_uri(&artist_uri)?;
  thread::sleep(Duration::from_millis(400));
  Ok(format!("Playing {} (This Is playlist unavailable — artist mix)", resolved.name))
}

/// Start song radio. `query = None` uses the currently playing track.
///
/// Desktop `spotify:radio:track:…` usually switches context immediately (often to a
/// related track). We try to preserve playback position when the seed track is still
/// current after the switch; otherwise radio simply continues with similar music.
pub fn play_radio(query: Option<&str>) -> Result<String, SpotifyClientError> {
  ensure_app()?;
  let (seed_uri, label, saved_pos) = match query {
    Some(q) if !q.trim().is_empty() => {
      let tracks = search_ranked_tracks(q.trim())?;
      let t = tracks
        .into_iter()
        .next()
        .ok_or_else(|| SpotifyClientError::SearchFailed(format!("No track matched “{q}”.")))?;
      (t.uri, format!("{} — {}", t.name, t.artists), None)
    }
    _ => {
      let uri = current_track_uri()?;
      let label = current_track().unwrap_or_else(|_| "this song".into());
      let pos = player_position_secs().ok();
      (uri, label, pos)
    }
  };

  let track_id = seed_uri
    .strip_prefix("spotify:track:")
    .ok_or_else(|| SpotifyClientError::ScriptFailed("Not a track URI".into()))?;
  let radio_uri = format!("spotify:radio:track:{track_id}");
  play_uri(&radio_uri)?;
  thread::sleep(Duration::from_millis(600));

  // Best-effort: if radio left us on the seed track, restore position.
  if let (Some(pos), Ok(now_uri)) = (saved_pos, current_track_uri()) {
    if now_uri == seed_uri && pos > 1.0 {
      let _ = set_player_position_secs(pos);
    }
  }

  Ok(format!("Playing radio for {label}"))
}

/// Continuous listen from a public playlist matching a mood/genre phrase.
pub fn play_genre(genre: &str) -> Result<String, SpotifyClientError> {
  ensure_app()?;
  let genre = genre.trim();
  if genre.is_empty() {
    return Err(SpotifyClientError::SearchFailed("Empty genre".into()));
  }
  let pl = search_best_playlist(genre, None)?;
  play_uri(&pl.uri)?;
  thread::sleep(Duration::from_millis(400));
  Ok(format!("Playing {} ({genre})", pl.name))
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
  playlists: Option<PlaylistsPage>,
  artists: Option<ArtistsPage>,
}

#[derive(Debug, Deserialize)]
struct TracksPage {
  items: Vec<TrackItem>,
}

#[derive(Debug, Deserialize)]
struct PlaylistsPage {
  items: Vec<Option<PlaylistItem>>,
}

#[derive(Debug, Deserialize)]
struct ArtistsPage {
  items: Vec<ArtistSearchItem>,
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

#[derive(Debug, Deserialize)]
struct ArtistSearchItem {
  id: String,
  name: String,
  #[allow(dead_code)]
  uri: String,
}

#[derive(Debug, Deserialize)]
struct PlaylistItem {
  name: String,
  uri: String,
  owner: Option<PlaylistOwner>,
}

#[derive(Debug, Deserialize)]
struct PlaylistOwner {
  id: Option<String>,
  #[allow(dead_code)]
  display_name: Option<String>,
}

#[derive(Debug, Clone)]
struct PlaylistHit {
  uri: String,
  name: String,
  score: i32,
}

#[derive(Debug, Clone)]
struct ResolvedArtist {
  id: String,
  name: String,
}

fn resolve_artist(spoken: &str) -> Result<ResolvedArtist, SpotifyClientError> {
  let token = client_credentials_token()?;
  let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  // Try as-spoken and space-collapsed (audio slave → audioslave).
  let mut queries = vec![spoken.to_string()];
  let collapsed = collapse_ws(spoken);
  if collapsed != spoken {
    queries.push(collapsed);
  }
  let mut best: Option<(i32, ResolvedArtist)> = None;
  for q in queries {
    let url = format!(
      "https://api.spotify.com/v1/search?type=artist&limit=5&q={}",
      urlencode_component(&q)
    );
    let resp = client
      .get(&url)
      .bearer_auth(&token)
      .send()
      .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
    if !resp.status().is_success() {
      continue;
    }
    let parsed: SearchResponse =
      resp.json().map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
    for item in parsed.artists.map(|a| a.items).unwrap_or_default() {
      let score = score_artist_name(spoken, &item.name);
      let resolved = ResolvedArtist { id: item.id, name: item.name };
      if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
        best = Some((score, resolved));
      }
    }
  }
  best
    .filter(|(s, _)| *s >= 40)
    .map(|(_, a)| a)
    .ok_or_else(|| SpotifyClientError::SearchFailed(format!("No artist matched “{spoken}”.")))
}

/// Spotify hides official “This Is” playlists from client-credentials search, but
/// still links them on the public artist page as editorial `37i9d…` playlists.
fn scrape_this_is_playlist_uri(artist_id: &str, artist_name: &str) -> Option<String> {
  let url = format!("https://open.spotify.com/artist/{artist_id}");
  let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .ok()?;
  let resp = client
    .get(&url)
    .header(
      "User-Agent",
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko)",
    )
    .header("Accept-Language", "en-US,en;q=0.9")
    .send()
    .ok()?;
  if !resp.status().is_success() {
    eprintln!(
      "AZAD_SPOTIFY event=this_is_scrape_http status={} artist_id={artist_id}",
      resp.status()
    );
    return None;
  }
  let html = resp.text().ok()?;
  let want = format!("this is {}", artist_name.to_ascii_lowercase());
  let want_c = collapse_ws(&want);

  // Editorial cards look like: href="/playlist/37i9d…"> … This Is BTS
  // Floor all slices to char boundaries (HTML has curly quotes / emoji).
  let mut i = 0;
  while i < html.len() {
    let Some(rel) = html[i..].find("/playlist/") else {
      break;
    };
    let start = i + rel;
    let id_start = start + "/playlist/".len();
    if id_start >= html.len() {
      break;
    }
    let rest = &html[id_start..];
    let id_end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
    let pid = &rest[..id_end];
    if !pid.starts_with("37i9d") {
      i = id_start + pid.len().max(1);
      continue;
    }
    let approx_end = id_start.saturating_add(900).min(html.len());
    let window_end = floor_char_boundary(&html, approx_end);
    let window = &html[id_start..window_end];
    let mut plain = String::with_capacity(window.len());
    let mut in_tag = false;
    for ch in window.chars() {
      match ch {
        '<' => in_tag = true,
        '>' => in_tag = false,
        _ if !in_tag => plain.push(ch),
        _ => {}
      }
    }
    let plain_l = plain.to_ascii_lowercase();
    let plain_c = collapse_ws(&plain_l);
    if plain_l.contains(&want) || plain_c.contains(&want_c) {
      let uri = format!("spotify:playlist:{pid}");
      eprintln!("AZAD_SPOTIFY event=this_is_scrape_hit uri={uri}");
      return Some(uri);
    }
    i = id_start + pid.len();
  }
  eprintln!("AZAD_SPOTIFY event=this_is_scrape_miss artist_id={artist_id} want={want:?}");
  None
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
  if idx >= s.len() {
    return s.len();
  }
  while idx > 0 && !s.is_char_boundary(idx) {
    idx -= 1;
  }
  idx
}

fn score_artist_name(spoken: &str, catalog: &str) -> i32 {
  let s = spoken.to_ascii_lowercase();
  let c = catalog.to_ascii_lowercase();
  let s_c = collapse_ws(&s);
  let c_c = collapse_ws(&c);
  if s == c || s_c == c_c {
    return 100;
  }
  if c.contains(&s) || s.contains(&c) || c_c.contains(&s_c) || s_c.contains(&c_c) {
    return 70;
  }
  let mut score = 0;
  for tok in s.split_whitespace() {
    if tok.len() > 1 && c.contains(tok) {
      score += 15;
    }
  }
  score
}

fn search_best_playlist(
  query: &str,
  prefer_title: Option<&str>,
) -> Result<PlaylistHit, SpotifyClientError> {
  let token = client_credentials_token()?;
  let client = reqwest::blocking::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  // Playlist search rejects limit > 10 (400 Invalid limit) on current Web API.
  let url = format!(
    "https://api.spotify.com/v1/search?type=playlist&limit=10&q={}",
    urlencode_component(query)
  );
  let resp = client
    .get(&url)
    .bearer_auth(&token)
    .send()
    .map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  if !resp.status().is_success() {
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    return Err(SpotifyClientError::SearchFailed(format!(
      "Playlist search failed ({status}): {}",
      body.chars().take(120).collect::<String>()
    )));
  }
  let parsed: SearchResponse =
    resp.json().map_err(|e| SpotifyClientError::SearchFailed(e.to_string()))?;
  let items: Vec<PlaylistItem> = parsed
    .playlists
    .map(|p| p.items.into_iter().flatten().collect())
    .unwrap_or_default();
  if items.is_empty() {
    return Err(SpotifyClientError::SearchFailed(format!("No playlists matched “{query}”.")));
  }

  let prefer = prefer_title.map(|s| s.to_ascii_lowercase());
  let mut best: Option<PlaylistHit> = None;
  for item in items {
    let name_l = item.name.to_ascii_lowercase();
    let mut score = 0i32;
    if let Some(ref pref) = prefer {
      if name_l == *pref {
        score += 120;
      } else if name_l.starts_with(pref) || name_l.contains(pref) {
        score += 60;
      }
    } else {
      // Free genre search: prefer shorter titles that contain the query tokens.
      let q_l = query.to_ascii_lowercase();
      if name_l == q_l {
        score += 80;
      }
      for tok in q_l.split_whitespace() {
        if tok.len() > 2 && name_l.contains(tok) {
          score += 15;
        }
      }
      // Mild preference for shorter playlist names (less spammy).
      score += (40 - name_l.len().min(40) as i32).max(0);
    }
    let owner_id = item.owner.as_ref().and_then(|o| o.id.as_deref()).unwrap_or("");
    if owner_id == "spotify" {
      score += 50;
    }
    let hit = PlaylistHit { uri: item.uri, name: item.name, score };
    if best.as_ref().map(|b| hit.score > b.score).unwrap_or(true) {
      best = Some(hit);
    }
  }
  let best =
    best.ok_or_else(|| SpotifyClientError::SearchFailed("No playlist candidates".into()))?;
  if best.score < 20 {
    return Err(SpotifyClientError::SearchFailed(format!(
      "No strong playlist match for “{query}”."
    )));
  }
  Ok(best)
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

  /// Live: requires network + Spotify.app + spotify.toml. Verifies official This Is.
  #[test]
  #[ignore]
  fn live_this_is_bts_resolves_editorial() {
    let artist = resolve_artist("BTS").expect("resolve BTS");
    assert_eq!(artist.name, "BTS");
    let uri = scrape_this_is_playlist_uri(&artist.id, &artist.name)
      .expect("scrape This Is BTS from artist page");
    assert!(uri.starts_with("spotify:playlist:37i9d"), "got {uri}");
    let msg = play_artist_this_is("BTS").expect("play");
    assert!(msg.to_ascii_lowercase().contains("this is bts"), "msg={msg}");
  }
}
