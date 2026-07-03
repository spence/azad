pub(super) fn build_paste_text(
  text: &str,
  append_trailing_space: bool,
  removed_words: &[String],
  deduplicate_words: bool,
) -> String {
  let mut paste_text = if removed_words.is_empty() {
    text.to_string()
  } else {
    strip_removed_words(text, removed_words)
  };
  if deduplicate_words {
    paste_text = collapse_consecutive_duplicates(&paste_text);
  }
  if append_trailing_space && !paste_text.chars().last().is_some_and(|ch| ch.is_whitespace()) {
    paste_text.push(' ');
  }
  paste_text
}

fn strip_removed_words(text: &str, removed_words: &[String]) -> String {
  let words: Vec<&str> = text.split_whitespace().collect();
  let kept: Vec<&str> = words
    .into_iter()
    .filter(|w| {
      let bare = w.trim_matches(|c: char| c.is_ascii_punctuation());
      !removed_words.iter().any(|rw| rw.eq_ignore_ascii_case(bare))
    })
    .collect();
  kept.join(" ")
}

/// Collapses consecutive duplicate words in `text`. Pure function. Runs after
/// `strip_removed_words` on the paste path; together they shape the final
/// emitted text before `platform::insert_text` fires.
///
/// Safety net for duplicate-word artifacts the azad-asr stitcher's seam-dedup
/// can't see: model-induced doubles inside a single partial and stable
/// user/model duplicates the final pass also produces.
///
/// Rules — exhaustive, each independently auditable:
/// 1. Tokenize on whitespace (matches `strip_removed_words` style).
/// 2. A token whose last character is non-alphanumeric is a "hard-break"
///    token — the next token is never compared against it. Catches sentence
///    boundaries, comma-separated spelled-out letters, parenthetical groups.
/// 3. To dedup, BOTH the previous and current token must be alphabetic-only
///    (after stripping leading/trailing punctuation) and have an alpha-key
///    of length >= 2. Protects digits, mixed-form tokens (`M3`, `1st`), and
///    single-letter spellings.
/// 4. Comparison is case-insensitive on the alpha key.
/// 5. When collapsing, drop the previous token; keep the current one. This
///    preserves any trailing punctuation that was on the duplicate's later
///    occurrence (e.g. `"the the. cat"` → `"the. cat"`).
///
/// Three-or-more-in-a-row collapses to one by induction — the rule is
/// pairwise but iterates left-to-right.
fn collapse_consecutive_duplicates(text: &str) -> String {
  let tokens: Vec<&str> = text.split_whitespace().collect();
  if tokens.len() < 2 {
    return text.to_string();
  }
  // Short-circuit when no pairwise duplicate exists — preserves the input's
  // exact whitespace (leading/trailing spaces, tab characters, etc.) instead
  // of normalising via `split_whitespace().join(" ")`. Same trick the
  // `removed_words.is_empty()` early return uses in `build_paste_text`.
  if !tokens.windows(2).any(|w| is_consecutive_duplicate(w[0], w[1])) {
    return text.to_string();
  }
  let mut kept: Vec<&str> = Vec::new();
  for tok in tokens {
    let should_collapse = kept.last().is_some_and(|prev| is_consecutive_duplicate(prev, tok));
    if should_collapse {
      kept.pop();
    }
    kept.push(tok);
  }
  kept.join(" ")
}

fn is_consecutive_duplicate(prev: &str, curr: &str) -> bool {
  // Rule 2: trailing punctuation on `prev` is a hard break.
  if prev.chars().last().map(|c| !c.is_alphanumeric()).unwrap_or(true) {
    return false;
  }
  // Rule 3: both must be alphabetic-only (after stripping edge punct) and >= 2 chars.
  if !is_alpha_word(prev) || !is_alpha_word(curr) {
    return false;
  }
  let prev_alpha = alpha_key(prev);
  let curr_alpha = alpha_key(curr);
  if prev_alpha.chars().count() < 2 {
    return false;
  }
  // Rule 4: case-insensitive on the alpha key.
  prev_alpha == curr_alpha
}

fn alpha_key(s: &str) -> String {
  s.chars().filter(|c| c.is_alphabetic()).flat_map(|c| c.to_lowercase()).collect()
}

fn is_alpha_word(s: &str) -> bool {
  let core = s.trim_matches(|c: char| !c.is_alphanumeric());
  !core.is_empty() && core.chars().all(|c| c.is_alphabetic())
}

#[cfg(test)]
mod tests {
  use super::{build_paste_text, collapse_consecutive_duplicates};

  #[test]
  fn build_paste_text_appends_trailing_space_when_enabled() {
    assert_eq!(build_paste_text("hello", true, &[], true), "hello ");
    assert_eq!(build_paste_text("hello ", true, &[], true), "hello ");
  }

  #[test]
  fn build_paste_text_preserves_input_when_trailing_space_is_disabled() {
    assert_eq!(build_paste_text("hello", false, &[], true), "hello");
    assert_eq!(build_paste_text("hello ", false, &[], true), "hello ");
  }

  #[test]
  fn build_paste_text_strips_removed_words() {
    let words = vec!["um".to_string(), "ah".to_string()];
    assert_eq!(
      build_paste_text("um I think ah this is right um", false, &words, true),
      "I think this is right"
    );
    assert_eq!(build_paste_text("Um hello Ah world", false, &words, true), "hello world");
  }

  #[test]
  fn build_paste_text_strips_removed_word_at_boundaries() {
    let words = vec!["um".to_string()];
    assert_eq!(build_paste_text("um", false, &words, true), "");
    assert_eq!(build_paste_text("um hello", false, &words, true), "hello");
    assert_eq!(build_paste_text("hello um", false, &words, true), "hello");
    assert_eq!(build_paste_text("yummy", false, &words, true), "yummy");
  }

  #[test]
  fn build_paste_text_strips_removed_words_with_punctuation() {
    let words = vec!["um".to_string(), "ah".to_string()];
    assert_eq!(
      build_paste_text("Um, I think this is right.", false, &words, true),
      "I think this is right."
    );
    assert_eq!(build_paste_text("Ah. Hello world.", false, &words, true), "Hello world.");
    assert_eq!(build_paste_text("um, ah, hello", false, &words, true), "hello");
  }

  #[test]
  fn collapse_dup_basic_two_in_a_row() {
    assert_eq!(collapse_consecutive_duplicates("the the cat"), "the cat");
  }

  #[test]
  fn collapse_dup_three_or_more_in_a_row() {
    // Pairwise iteration collapses N-in-a-row down to 1.
    assert_eq!(collapse_consecutive_duplicates("that that that idea"), "that idea");
    assert_eq!(collapse_consecutive_duplicates("uh uh uh uh hello"), "uh hello");
  }

  #[test]
  fn collapse_dup_period_acts_as_barrier() {
    // Trailing period on the previous token blocks dedup — it's a sentence boundary.
    assert_eq!(collapse_consecutive_duplicates("the. the cat"), "the. the cat");
    assert_eq!(collapse_consecutive_duplicates("end. End of sentence."), "end. End of sentence.");
  }

  #[test]
  fn collapse_dup_comma_acts_as_barrier_for_letter_spelling() {
    // Spelled-out letter sequences must survive: every comma is a hard break,
    // and the single-letter alpha-key fails the len-≥-2 rule on top of that.
    assert_eq!(collapse_consecutive_duplicates("S, P, E, N, C, E, R"), "S, P, E, N, C, E, R");
  }

  #[test]
  fn collapse_dup_single_letter_no_collapse() {
    // No commas — len-≥-2 alpha-key rule still protects single-letter spellings.
    assert_eq!(collapse_consecutive_duplicates("M M alpha"), "M M alpha");
    assert_eq!(collapse_consecutive_duplicates("A A B B"), "A A B B");
  }

  #[test]
  fn collapse_dup_digits_no_collapse() {
    // `is_alpha_word` rejects digit-only tokens; numeric codes survive.
    assert_eq!(collapse_consecutive_duplicates("2288 2288"), "2288 2288");
    // User's own example — codes read aloud with comma/period pauses.
    assert_eq!(collapse_consecutive_duplicates("2288. Eight, eight."), "2288. Eight, eight.");
  }

  #[test]
  fn collapse_dup_preserves_trailing_punct_on_survivor() {
    // When a duplicate has trailing punctuation on its later occurrence, drop
    // the previous (no-punct) copy and keep the punctuation-bearing one.
    assert_eq!(collapse_consecutive_duplicates("the the. cat"), "the. cat");
    assert_eq!(collapse_consecutive_duplicates("uh uh, hello"), "uh, hello");
  }

  #[test]
  fn collapse_dup_case_insensitive() {
    // Match modulo case; survivor is the LATER token, so its casing wins.
    assert_eq!(collapse_consecutive_duplicates("The the cat"), "the cat");
    assert_eq!(collapse_consecutive_duplicates("the The cat"), "The cat");
  }

  /// Documents the ACCEPTED-trade-off: spelled-out number words spoken
  /// without comma/period pauses ("two two eight eight" as a code) DO get
  /// collapsed today. Speakers naturally pause when reading codes, which
  /// produces the punctuation barriers that protect the case. If real-world
  /// false-positives become a problem, add a small static whitelist of number
  /// words ("one"-"twelve", "twenty"-"ninety", "hundred", "thousand",
  /// "million") to `is_consecutive_duplicate` as a fifth rule. This test
  /// trips when that change lands and forces an explicit decision.
  #[test]
  fn collapse_dup_known_false_positive_unpunctuated_number_words() {
    assert_eq!(collapse_consecutive_duplicates("two two eight eight"), "two eight");
  }

  #[test]
  fn build_paste_text_runs_filler_then_dedup_in_order() {
    // End-to-end: filler removal first, dedup second. The "um the the cat"
    // input first becomes "the the cat" (filler stripped), then "the cat"
    // (dedup'd). Pins ordering inside `build_paste_text`.
    let words = vec!["um".to_string()];
    assert_eq!(build_paste_text("um the the cat", false, &words, true), "the cat");
  }

  #[test]
  fn build_paste_text_can_preserve_duplicate_words() {
    let words = vec!["um".to_string()];
    assert_eq!(build_paste_text("um no no", false, &words, false), "no no");
  }
}
