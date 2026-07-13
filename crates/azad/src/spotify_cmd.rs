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
      Self::Search { query } => format!("Searching for “{query}”"),
      Self::Identify { play: true } => "Listening, then will play the match…".into(),
      Self::Identify { play: false } => "Listening to identify the song…".into(),
      Self::VolumeUp => "Turning volume up".into(),
      Self::VolumeDown => "Turning volume down".into(),
      Self::Help => {
        "Try: pause, next, play <song>, what song is this, identify, like, volume up."
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
    "previous" | "prev" | "back" | "last song" | "previous song" | "previous track"
    | "go back" => return SpotifyIntent::Previous,
    "like" | "heart" | "love" | "save" | "save this" | "like this" | "like song" => {
      return SpotifyIntent::Like;
    }
    "current" | "now playing" | "whats playing" | "what is playing" | "what's playing"
    | "what song is playing" => return SpotifyIntent::Current,
    "volume up" | "louder" | "turn it up" | "turn up" => return SpotifyIntent::VolumeUp,
    "volume down" | "quieter" | "turn it down" | "turn down" => return SpotifyIntent::VolumeDown,
    "help" | "commands" | "what can i say" => return SpotifyIntent::Help,
    _ => {}
  }

  // "search <query>"
  if let Some(rest) = q.strip_prefix("search ") {
    let rest = rest.trim();
    if !rest.is_empty() {
      return SpotifyIntent::Search {
        query: rest.to_string(),
      };
    }
  }

  // "play <query>" — not identify (those already returned).
  if let Some(rest) = q.strip_prefix("play ") {
    let rest = rest.trim();
    if !rest.is_empty() {
      return SpotifyIntent::PlayQuery {
        query: rest.to_string(),
      };
    }
  }

  // Bare remainder: treat as play query if it looks like a title, else unsupported.
  if q.split_whitespace().count() >= 1 {
    return SpotifyIntent::PlayQuery { query: q };
  }

  SpotifyIntent::Unsupported {
    message: "Try pause, next, play <song>, or what song is this.".into(),
  }
}

fn match_identify(q: &str) -> Option<SpotifyIntent> {
  // Order: longer / more specific first; play variants set play=true.
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

fn normalize(query: &str) -> String {
  query
    .chars()
    .map(|c| {
      if c.is_ascii_alphanumeric() || c.is_whitespace() {
        c.to_ascii_lowercase()
      } else {
        ' '
      }
    })
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
    assert_eq!(
      interpret_spotify_query("identify"),
      SpotifyIntent::Identify { play: false }
    );
    assert_eq!(
      interpret_spotify_query("identify and play"),
      SpotifyIntent::Identify { play: true }
    );
  }

  #[test]
  fn play_query() {
    assert_eq!(
      interpret_spotify_query("play them changes"),
      SpotifyIntent::PlayQuery {
        query: "them changes".into()
      }
    );
    assert_eq!(
      interpret_spotify_query("Them Changes"),
      SpotifyIntent::PlayQuery {
        query: "them changes".into()
      }
    );
  }

  #[test]
  fn empty_is_help() {
    assert_eq!(interpret_spotify_query(""), SpotifyIntent::Help);
    assert_eq!(interpret_spotify_query("  "), SpotifyIntent::Help);
  }
}
