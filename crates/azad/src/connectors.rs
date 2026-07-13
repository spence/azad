//! Connectors: built-in trigger phrases that add context to a dictation turn.
//!
//! When an utterance opens with a connector's `trigger` (e.g. "hey claude"), the
//! turn is tagged with that connector and the leading phrase is stripped to form a
//! "clean query". This module is the platform-free detection core; persistence of
//! the enabled set lives in `preferred_store`, and the per-turn latch + overlay
//! tag live in `app`. Routing the clean query onward is a deferred follow-up.

/// A recognized lead-in phrase. The set is defined in code (see
/// [`builtin_connectors`]); only `enabled` changes at runtime, so the other fields
/// stay `&'static str` and the seed is allocation-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
  pub id: &'static str,
  pub display_name: &'static str,
  /// The leading phrase that activates this connector, lowercase and
  /// punctuation-free (matching is case-insensitive and punctuation-tolerant).
  pub trigger: &'static str,
  /// Extra ASR-friendly phrases that latch the same connector (same rules as `trigger`).
  /// Prefer the same token count as `trigger` so strip-by-count stays consistent when
  /// the matched phrase is not stored; `detect` still returns the exact phrase matched.
  pub trigger_aliases: &'static [&'static str],
  /// Short label shown in the overlay chip when the connector is active.
  pub tag_label: &'static str,
  /// Asset file (in `assets/`, bundled into `Contents/Resources/`) rendered as an
  /// icon in the overlay chip, left of `tag_label`. Empty for no icon.
  pub tag_icon: &'static str,
  pub enabled: bool,
}

/// The result of matching an utterance against the enabled connectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorMatch {
  pub id: &'static str,
  pub tag_label: &'static str,
  pub tag_icon: &'static str,
  /// The exact trigger phrase that matched (primary or alias) — use for strip.
  pub matched_trigger: &'static str,
  /// The utterance with the leading trigger phrase removed.
  pub clean_query: String,
}

/// Built-in Claude connector id (Local Agent Gateway).
pub const CLAUDE_CONNECTOR_ID: &str = "claude";
/// Built-in Azad connector id (on-device Apple Intelligence tools).
pub const AZAD_CONNECTOR_ID: &str = "azad";
/// Built-in Spotify connector id (heuristic → Spotify.app / Shazam).
pub const SPOTIFY_CONNECTOR_ID: &str = "spotify";

/// Common ASR mis-hearings of "azad" after "hey" (observed: "Hey azod, …").
const AZAD_TRIGGER_ALIASES: &[&str] = &[
  "hey azod",
  "hey asad",
  "hey assad",
  "hey asod",
  "hey az id",
  "hey as odd",
  "hey a zod",
  "hey a zad",
  "hey a sad",
  "hey az at",
  "hey as at",
  "hey is odd",
];

/// Common ASR mis-hearings of "spotify" after "hey".
const SPOTIFY_TRIGGER_ALIASES: &[&str] = &[
  "hey spotsify",
  "hey spot ify",
  "hey spot a fy",
  "hey spotify",
];

/// The built-in connector registry. Order is stable (settings UI indexes into it).
/// Claude and Azad default on; Spotify defaults off until the user enables it
/// (and Spotify.app is installed — enforced in settings).
pub fn builtin_connectors() -> Vec<Connector> {
  vec![
    Connector {
      id: CLAUDE_CONNECTOR_ID,
      display_name: "Claude",
      trigger: "hey claude",
      trigger_aliases: &[],
      tag_label: "Claude",
      tag_icon: "claude.svg",
      enabled: true,
    },
    Connector {
      id: AZAD_CONNECTOR_ID,
      display_name: "Azad",
      trigger: "hey azad",
      trigger_aliases: AZAD_TRIGGER_ALIASES,
      tag_label: "Azad",
      // Text-only chip until a dedicated template asset is bundled.
      tag_icon: "",
      enabled: true,
    },
    Connector {
      id: SPOTIFY_CONNECTOR_ID,
      display_name: "Spotify",
      trigger: "hey spotify",
      trigger_aliases: SPOTIFY_TRIGGER_ALIASES,
      tag_label: "Spotify",
      tag_icon: "",
      enabled: false,
    },
  ]
}

/// Every phrase that can activate `connector` (primary first, then aliases).
pub fn connector_triggers(connector: &Connector) -> impl Iterator<Item = &'static str> + '_ {
  std::iter::once(connector.trigger).chain(connector.trigger_aliases.iter().copied())
}

/// First enabled connector whose trigger (or alias) leads the utterance, with its clean query.
pub fn detect(utterance: &str, connectors: &[Connector]) -> Option<ConnectorMatch> {
  connectors.iter().filter(|c| c.enabled).find_map(|c| {
    // Prefer the longest matching phrase so multi-token aliases beat shorter ones.
    let mut best: Option<&'static str> = None;
    for phrase in connector_triggers(c) {
      if trigger_matches_prefix(utterance, phrase) {
        let better = best.map_or(true, |b| {
          phrase.split_whitespace().count() > b.split_whitespace().count()
        });
        if better {
          best = Some(phrase);
        }
      }
    }
    best.map(|matched_trigger| ConnectorMatch {
      id: c.id,
      tag_label: c.tag_label,
      tag_icon: c.tag_icon,
      matched_trigger,
      clean_query: strip_trigger(utterance, matched_trigger),
    })
  })
}

/// True when every trigger token leads `utterance` in order, comparing bare tokens
/// case-insensitively. False when `utterance` has fewer tokens than the trigger, so
/// a partial draft ("hey") never matches a longer trigger ("hey claude") — this is
/// what lets the caller latch without flicker.
pub fn trigger_matches_prefix(utterance: &str, trigger: &str) -> bool {
  let trigger_tokens: Vec<&str> = trigger.split_whitespace().collect();
  if trigger_tokens.is_empty() {
    return false;
  }
  let utterance_tokens: Vec<&str> = utterance.split_whitespace().collect();
  if utterance_tokens.len() < trigger_tokens.len() {
    return false;
  }
  trigger_tokens
    .iter()
    .zip(utterance_tokens.iter())
    .all(|(t, u)| bare(u).eq_ignore_ascii_case(bare(t)))
}

/// Drops the leading trigger tokens, preserving the rest verbatim (interior
/// punctuation kept; surrounding whitespace normalized, like `strip_removed_words`).
pub fn strip_trigger(utterance: &str, trigger: &str) -> String {
  let skip = trigger.split_whitespace().count();
  utterance.split_whitespace().skip(skip).collect::<Vec<_>>().join(" ")
}

fn bare(token: &str) -> &str {
  token.trim_matches(|c: char| c.is_ascii_punctuation())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn connectors() -> Vec<Connector> {
    builtin_connectors()
  }

  fn enable_azad(cs: &mut [Connector]) {
    for c in cs {
      if c.id == AZAD_CONNECTOR_ID {
        c.enabled = true;
      }
    }
  }

  #[test]
  fn matches_exact_leading_phrase() {
    assert!(trigger_matches_prefix("hey claude what's the weather", "hey claude"));
  }

  #[test]
  fn match_is_case_insensitive_and_punctuation_tolerant() {
    assert!(trigger_matches_prefix("Hey, Claude! what's up", "hey claude"));
    assert!(trigger_matches_prefix("HEY CLAUDE", "hey claude"));
  }

  #[test]
  fn match_requires_phrase_at_the_start() {
    assert!(!trigger_matches_prefix("well hey claude there", "hey claude"));
  }

  #[test]
  fn partial_prefix_does_not_match() {
    assert!(!trigger_matches_prefix("hey", "hey claude"));
    assert!(!trigger_matches_prefix("hey there", "hey claude"));
  }

  #[test]
  fn token_order_matters() {
    assert!(!trigger_matches_prefix("claude hey what's up", "hey claude"));
  }

  #[test]
  fn empty_utterance_does_not_match() {
    assert!(!trigger_matches_prefix("", "hey claude"));
  }

  #[test]
  fn strip_yields_clean_query() {
    assert_eq!(strip_trigger("hey claude what's the weather", "hey claude"), "what's the weather");
  }

  #[test]
  fn strip_trigger_only_yields_empty_query() {
    assert_eq!(strip_trigger("hey claude", "hey claude"), "");
  }

  #[test]
  fn strip_preserves_interior_punctuation() {
    assert_eq!(
      strip_trigger("Hey, Claude, what's the weather, today?", "hey claude"),
      "what's the weather, today?"
    );
  }

  #[test]
  fn detect_returns_match_for_enabled_connector() {
    let m = detect("hey claude open the door", &connectors()).expect("should match");
    assert_eq!(m.id, "claude");
    assert_eq!(m.tag_label, "Claude");
    assert_eq!(m.tag_icon, "claude.svg");
    assert_eq!(m.matched_trigger, "hey claude");
    assert_eq!(m.clean_query, "open the door");
  }

  #[test]
  fn detect_ignores_disabled_connector() {
    let mut cs = connectors();
    cs[0].enabled = false;
    assert_eq!(detect("hey claude open the door", &cs), None);
  }

  #[test]
  fn detect_returns_none_without_trigger() {
    assert_eq!(detect("open the door please", &connectors()), None);
  }

  #[test]
  fn azad_defaults_enabled_and_matches() {
    let cs = connectors();
    let azad = cs.iter().find(|c| c.id == AZAD_CONNECTOR_ID).expect("azad connector");
    assert!(azad.enabled);
    let m = detect("hey azad disable numbers", &cs).expect("default-on azad should match");
    assert_eq!(m.id, AZAD_CONNECTOR_ID);
    assert_eq!(m.tag_label, "Azad");
    assert_eq!(m.clean_query, "disable numbers");
  }

  #[test]
  fn azad_disabled_does_not_match() {
    let mut cs = connectors();
    for c in &mut cs {
      if c.id == AZAD_CONNECTOR_ID {
        c.enabled = false;
      }
    }
    assert_eq!(detect("hey azad disable numbers", &cs), None);
  }

  #[test]
  fn azad_matches_when_enabled() {
    let mut cs = connectors();
    enable_azad(&mut cs);
    let m = detect("Hey Azad, disable number text replacement", &cs).expect("should match");
    assert_eq!(m.id, AZAD_CONNECTOR_ID);
    assert_eq!(m.tag_label, "Azad");
    assert_eq!(m.matched_trigger, "hey azad");
    assert_eq!(m.clean_query, "disable number text replacement");
  }

  /// Real transcript from Spencer's machine (metrics): "Hey azod, I want you to…"
  #[test]
  fn azad_matches_observed_hey_azod_transcript() {
    let cs = connectors();
    let m = detect("Hey azod, I want you to disable the automatic", &cs).expect("hey azod");
    assert_eq!(m.id, AZAD_CONNECTOR_ID);
    assert_eq!(m.tag_label, "Azad");
    assert_eq!(m.matched_trigger, "hey azod");
    assert_eq!(m.clean_query, "I want you to disable the automatic");
  }

  #[test]
  fn azad_matches_asr_aliases() {
    let mut cs = connectors();
    enable_azad(&mut cs);
    for spoken in [
      "hey azod disable numbers",
      "Hey Azod, turn off emoji",
      "hey asad enable hesitations",
      "hey a zod turn on numbers",
    ] {
      let m = detect(spoken, &cs).unwrap_or_else(|| panic!("should match alias: {spoken}"));
      assert_eq!(m.id, AZAD_CONNECTOR_ID, "spoken={spoken}");
      assert_eq!(m.tag_label, "Azad", "spoken={spoken}");
      assert!(!m.clean_query.is_empty() || spoken.ends_with("azod"), "spoken={spoken}");
      // Trigger is out of the clean query (chip owns the brand; body is the request).
      assert!(!m.clean_query.to_ascii_lowercase().contains("hey "));
    }
  }

  #[test]
  fn azad_alias_strips_three_token_phrase() {
    let mut cs = connectors();
    enable_azad(&mut cs);
    let m = detect("hey a zod disable numbers", &cs).expect("alias");
    assert_eq!(m.matched_trigger, "hey a zod");
    assert_eq!(m.clean_query, "disable numbers");
  }

  #[test]
  fn claude_and_azad_triggers_do_not_collide() {
    let mut cs = connectors();
    for c in &mut cs {
      c.enabled = true;
    }
    let claude = detect("hey claude summarize this", &cs).expect("claude");
    assert_eq!(claude.id, CLAUDE_CONNECTOR_ID);
    let azad = detect("hey azad turn off emoji", &cs).expect("azad");
    assert_eq!(azad.id, AZAD_CONNECTOR_ID);
  }

  #[test]
  fn spotify_defaults_disabled() {
    let cs = connectors();
    let sp = cs.iter().find(|c| c.id == SPOTIFY_CONNECTOR_ID).expect("spotify");
    assert!(!sp.enabled);
    assert_eq!(detect("hey spotify pause", &cs), None);
  }

  #[test]
  fn spotify_matches_when_enabled() {
    let mut cs = connectors();
    for c in &mut cs {
      if c.id == SPOTIFY_CONNECTOR_ID {
        c.enabled = true;
      }
    }
    let m = detect("Hey Spotify, pause", &cs).expect("spotify");
    assert_eq!(m.id, SPOTIFY_CONNECTOR_ID);
    assert_eq!(m.tag_label, "Spotify");
    assert_eq!(m.clean_query, "pause");
  }
}
