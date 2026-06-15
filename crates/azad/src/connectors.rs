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
  /// The utterance with the leading trigger phrase removed.
  pub clean_query: String,
}

/// The built-in connector registry. Seeded with one entry; add more here.
pub fn builtin_connectors() -> Vec<Connector> {
  vec![Connector {
    id: "claude",
    display_name: "Claude",
    trigger: "hey claude",
    tag_label: "Claude",
    tag_icon: "claude.svg",
    enabled: true,
  }]
}

/// First enabled connector whose trigger leads the utterance, with its clean query.
pub fn detect(utterance: &str, connectors: &[Connector]) -> Option<ConnectorMatch> {
  connectors.iter().filter(|c| c.enabled).find_map(|c| {
    trigger_matches_prefix(utterance, c.trigger).then(|| ConnectorMatch {
      id: c.id,
      tag_label: c.tag_label,
      tag_icon: c.tag_icon,
      clean_query: strip_trigger(utterance, c.trigger),
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
}
