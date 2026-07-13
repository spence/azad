//! Heuristic intent parser for the Hey Spotify connector (no LLM).

use serde::{Deserialize, Serialize};

/// Allowlisted Spotify voice intents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpotifyIntent {
  Play,
  Pause,
  PlayPause,
  Next,
  Previous,
  Like,
  Current,
  /// Search catalog and start the top track.
  PlayQuery {
    query: String,
  },
  /// Play Spotify's "This Is {Artist}" (or best public match) curated playlist.
  PlayArtist {
    artist: String,
  },
  /// Song radio. `query` is None = radio for whatever is playing now.
  PlayRadio {
    query: Option<String>,
  },
  /// Mood / genre continuous listen via public playlist search.
  PlayGenre {
    genre: String,
  },
  Search {
    query: String,
  },
  /// Shazam identify; `play` means start the match on Spotify after identify.
  Identify {
    play: bool,
  },
  VolumeUp,
  VolumeDown,
  Help,
  Unsupported {
    message: String,
  },
}

impl SpotifyIntent {
  pub fn confirmation_label(&self) -> String {
    match self {
      Self::Play => "Resuming playback".into(),
      Self::Pause => "Paused".into(),
      Self::PlayPause => "Toggled play/pause".into(),
      Self::Next => "Skipped to next track".into(),
      Self::Previous => "Went to previous track".into(),
      Self::Like => "Liked current track".into(),
      Self::Current => "Fetching what's playing…".into(),
      Self::PlayQuery { query } => format!("Playing “{query}”"),
      Self::PlayArtist { artist } => format!("Playing This Is {artist}"),
      Self::PlayRadio { query: Some(q) } => format!("Playing radio for “{q}”"),
      Self::PlayRadio { query: None } => "Playing radio for this song".into(),
      Self::PlayGenre { genre } => format!("Playing {genre}"),
      Self::Search { query } => format!("Searching for “{query}”"),
      Self::Identify { play: true } => "Listening, then will play the match…".into(),
      Self::Identify { play: false } => "Listening to identify the song…".into(),
      Self::VolumeUp => "Turning volume up".into(),
      Self::VolumeDown => "Turning volume down".into(),
      Self::Help => {
        "Try: pause, next, play <song>, play <artist>, play radio for <song>, play workout music."
          .into()
      }
      Self::Unsupported { message } => message.clone(),
    }
  }

  #[allow(dead_code)] // used when overlay gates expand
  pub fn is_actionable(&self) -> bool {
    !matches!(self, Self::Unsupported { .. } | Self::Help)
  }

  /// True if this intent needs the Shazam helper (mic).
  #[allow(dead_code)] // Phase 2 Shazam
  pub fn needs_identify(&self) -> bool {
    matches!(self, Self::Identify { .. })
  }
}

/// Parse a clean query (trigger already stripped) into a Spotify intent.
pub fn interpret_spotify_query(query: &str) -> SpotifyIntent {
  let q = normalize(query);
  if q.is_empty() {
    return SpotifyIntent::Help;
  }

  if let Some(intent) = match_identify(&q) {
    return intent;
  }

  // Exact / whole-phrase transport commands first.
  match q.as_str() {
    "pause" | "stop" | "stop playing" => return SpotifyIntent::Pause,
    "play" | "resume" | "unpause" | "continue" => return SpotifyIntent::Play,
    "play pause" | "playpause" | "toggle" | "toggle play" => return SpotifyIntent::PlayPause,
    "next" | "skip" | "skip song" | "next song" | "next track" => return SpotifyIntent::Next,
    "previous" | "prev" | "back" | "last song" | "previous song" | "previous track" | "go back" => {
      return SpotifyIntent::Previous;
    }
    "like" | "heart" | "love" | "save" | "save this" | "like this" | "like song" => {
      return SpotifyIntent::Like;
    }
    "current"
    | "now playing"
    | "whats playing"
    | "what is playing"
    | "what's playing"
    | "what song is playing" => return SpotifyIntent::Current,
    "volume up" | "louder" | "turn it up" | "turn up" => return SpotifyIntent::VolumeUp,
    "volume down" | "quieter" | "turn it down" | "turn down" => return SpotifyIntent::VolumeDown,
    "help" | "commands" | "what can i say" => return SpotifyIntent::Help,
    // Radio for whatever is currently playing.
    "radio" | "play radio" | "song radio" | "this song radio" | "radio this"
    | "radio this song" | "start radio" | "play song radio" => {
      return SpotifyIntent::PlayRadio { query: None };
    }
    _ => {}
  }

  // "radio for <query>" / "play radio for <query>" / "play the radio for <query>"
  if let Some(rest) = strip_one_of(
    &q,
    &[
      "play the radio for ",
      "play radio for ",
      "radio for ",
      "start radio for ",
      "song radio for ",
    ],
  ) {
    if !rest.is_empty() {
      return SpotifyIntent::PlayRadio { query: Some(rest.to_string()) };
    }
  }

  // "search <query>"
  if let Some(rest) = q.strip_prefix("search ") {
    let rest = rest.trim();
    if !rest.is_empty() {
      return SpotifyIntent::Search { query: rest.to_string() };
    }
  }

  // "play this is <artist>" / "this is <artist>"
  if let Some(rest) = strip_one_of(&q, &["play this is ", "this is "]) {
    if !rest.is_empty() {
      return SpotifyIntent::PlayArtist { artist: rest.to_string() };
    }
  }

  // "play the artist <name>" / "play artist <name>"
  if let Some(rest) = strip_one_of(&q, &["play the artist ", "play artist "]) {
    if !rest.is_empty() {
      return SpotifyIntent::PlayArtist { artist: rest.to_string() };
    }
  }

  // Genre / mood: "play workout music", "play some lo fi", "play lofi hip hop"
  if let Some(genre) = match_genre(&q) {
    return SpotifyIntent::PlayGenre { genre };
  }

  // "play <query>" — song (with "by") vs artist vs free-text track.
  if let Some(rest) = q.strip_prefix("play ") {
    let rest = rest.trim();
    if !rest.is_empty() {
      return classify_play_rest(rest);
    }
  }

  // Bare remainder without "play".
  if q.split_whitespace().count() >= 1 {
    return classify_play_rest(&q);
  }

  SpotifyIntent::Unsupported {
    message: "Try pause, next, play <song>, play <artist>, or play radio for <song>.".into(),
  }
}

/// After stripping a leading "play ", decide track vs artist vs genre.
fn classify_play_rest(rest: &str) -> SpotifyIntent {
  // Explicit song form always wins.
  if rest.contains(" by ") {
    return SpotifyIntent::PlayQuery { query: rest.to_string() };
  }
  // "play workout music" already handled before we get here with "play " prefix —
  // re-check bare rest for "workout music" style.
  if let Some(genre) = match_genre(&format!("play {rest}")) {
    return SpotifyIntent::PlayGenre { genre };
  }
  if let Some(genre) = match_genre(rest) {
    return SpotifyIntent::PlayGenre { genre };
  }

  // Short-ish name without "by": treat as artist → This Is {Artist}.
  // Long free text is more often a song title.
  let tokens = rest.split_whitespace().count();
  if tokens >= 1 && tokens <= 4 {
    return SpotifyIntent::PlayArtist { artist: rest.to_string() };
  }
  SpotifyIntent::PlayQuery { query: rest.to_string() }
}

fn match_genre(q: &str) -> Option<String> {
  // "play some workout music" / "play workout music" / "play workout"
  let mut body = q
    .strip_prefix("play some ")
    .or_else(|| q.strip_prefix("play "))
    .unwrap_or(q)
    .trim();
  if let Some(stripped) = body.strip_suffix(" music") {
    body = stripped.trim();
  }
  if body.is_empty() {
    return None;
  }

  // Canonical genre labels we accept (voice forms → search phrase).
  const GENRES: &[(&[&str], &str)] = &[
    (&["workout", "gym", "exercise", "cardio", "lifting"], "workout"),
    (&["lofi", "lo fi", "lo-fi", "lofi hip hop", "lo fi hip hop", "chillhop"], "lofi hip hop"),
    (&["chill", "chill out", "chilled"], "chill"),
    (&["focus", "study", "concentration", "deep work"], "focus study"),
    (&["sleep", "sleeping", "bedtime"], "sleep"),
    (&["party", "dance party"], "party"),
    (&["jazz"], "jazz"),
    (&["classical", "classic music"], "classical"),
    (&["rap", "hip hop", "hiphop"], "hip hop"),
    (&["metal", "heavy metal"], "metal"),
    (&["rock", "classic rock"], "rock"),
    (&["country"], "country"),
    (&["r and b", "rnb", "r n b", "rhythm and blues"], "r&b"),
    (&["soul"], "soul"),
    (&["reggae"], "reggae"),
    (&["ambient"], "ambient"),
    (&["running", "run", "jog", "jogging"], "running"),
    (&["edm", "electronic", "dance", "house music", "house"], "edm"),
    (&["indie"], "indie"),
    (&["pop", "top 40", "top forty"], "pop"),
    (&["acoustic"], "acoustic"),
    (&["sad", "sad songs", "heartbreak"], "sad songs"),
    (&["happy", "feel good", "feelgood"], "feel good"),
    (&["romantic", "love songs"], "romantic love songs"),
  ];

  let body_l = body.to_ascii_lowercase();
  for (aliases, label) in GENRES {
    for a in *aliases {
      if body_l == *a {
        return Some((*label).to_string());
      }
    }
  }
  None
}

fn match_identify(q: &str) -> Option<SpotifyIntent> {
  let identify_play: &[&str] = &[
    "identify and play",
    "identify then play",
    "play what song is this",
    "play the song that is playing",
    "play the song thats playing",
    "play that song",
  ];
  for p in identify_play {
    if q == *p || q.starts_with(&format!("{p} ")) {
      return Some(SpotifyIntent::Identify { play: true });
    }
  }

  let identify_only: &[&str] = &[
    "what song is this",
    "whats this song",
    "what's this song",
    "what is this song",
    "identify this song",
    "identify the song",
    "identify song",
    "identify",
    "shazam",
    "name this song",
    "name that song",
  ];
  for p in identify_only {
    if q == *p || q.starts_with(&format!("{p} ")) {
      return Some(SpotifyIntent::Identify { play: false });
    }
  }
  None
}

fn strip_one_of<'a>(q: &'a str, prefixes: &[&str]) -> Option<&'a str> {
  for p in prefixes {
    if let Some(rest) = q.strip_prefix(p) {
      let rest = rest.trim();
      if !rest.is_empty() {
        return Some(rest);
      }
    }
  }
  None
}

fn normalize(query: &str) -> String {
  query
    .chars()
    .map(
      |c| {
        if c.is_ascii_alphanumeric() || c.is_whitespace() { c.to_ascii_lowercase() } else { ' ' }
      },
    )
    .collect::<String>()
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn transport_commands() {
    assert_eq!(interpret_spotify_query("pause"), SpotifyIntent::Pause);
    assert_eq!(interpret_spotify_query("next"), SpotifyIntent::Next);
    assert_eq!(interpret_spotify_query("previous"), SpotifyIntent::Previous);
    assert_eq!(interpret_spotify_query("play"), SpotifyIntent::Play);
    assert_eq!(interpret_spotify_query("like"), SpotifyIntent::Like);
  }

  #[test]
  fn identify_phrases_do_not_become_play_query() {
    assert_eq!(
      interpret_spotify_query("what song is this"),
      SpotifyIntent::Identify { play: false }
    );
    assert_eq!(
      interpret_spotify_query("identify this song"),
      SpotifyIntent::Identify { play: false }
    );
    assert_eq!(interpret_spotify_query("identify"), SpotifyIntent::Identify { play: false });
    assert_eq!(
      interpret_spotify_query("identify and play"),
      SpotifyIntent::Identify { play: true }
    );
  }

  #[test]
  fn play_song_with_by_is_track() {
    assert_eq!(
      interpret_spotify_query("play them changes by thundercat"),
      SpotifyIntent::PlayQuery { query: "them changes by thundercat".into() }
    );
  }

  #[test]
  fn play_artist_is_this_is_playlist() {
    assert_eq!(
      interpret_spotify_query("play bts"),
      SpotifyIntent::PlayArtist { artist: "bts".into() }
    );
    assert_eq!(
      interpret_spotify_query("play audio slave"),
      SpotifyIntent::PlayArtist { artist: "audio slave".into() }
    );
    assert_eq!(
      interpret_spotify_query("play this is oasis"),
      SpotifyIntent::PlayArtist { artist: "oasis".into() }
    );
  }

  #[test]
  fn play_radio_forms() {
    assert_eq!(interpret_spotify_query("radio"), SpotifyIntent::PlayRadio { query: None });
    assert_eq!(interpret_spotify_query("play radio"), SpotifyIntent::PlayRadio { query: None });
    assert_eq!(
      interpret_spotify_query("play radio for champagne supernova"),
      SpotifyIntent::PlayRadio { query: Some("champagne supernova".into()) }
    );
    assert_eq!(
      interpret_spotify_query("radio for like a stone by audio slave"),
      SpotifyIntent::PlayRadio { query: Some("like a stone by audio slave".into()) }
    );
  }

  #[test]
  fn play_genre_forms() {
    assert_eq!(
      interpret_spotify_query("play workout music"),
      SpotifyIntent::PlayGenre { genre: "workout".into() }
    );
    assert_eq!(
      interpret_spotify_query("play lo fi"),
      SpotifyIntent::PlayGenre { genre: "lofi hip hop".into() }
    );
    assert_eq!(
      interpret_spotify_query("play some chill music"),
      SpotifyIntent::PlayGenre { genre: "chill".into() }
    );
  }

  #[test]
  fn empty_is_help() {
    assert_eq!(interpret_spotify_query(""), SpotifyIntent::Help);
    assert_eq!(interpret_spotify_query("  "), SpotifyIntent::Help);
  }
}
