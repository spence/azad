pub(super) const INCREMENTAL_STITCH_TAIL_WINDOW_TOKENS: usize = 256;
pub(super) const INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS: usize = 2;
const INCREMENTAL_STITCH_MAX_TAIL_DROP_TOKENS: usize = 24;
const INCREMENTAL_STITCH_MAX_SHRINK_TOKENS: usize = 2;
const INCREMENTAL_STITCH_STRONG_OVERLAP_TOKENS: usize = 3;
const INCREMENTAL_STITCH_SAMPLES_PER_WORD_MAX: usize = 4_000;
const INCREMENTAL_STITCH_RIGHT_START_SLACK_TOKENS: usize = 2;
fn overlap_tail_drop_is_safe(tail_drop: usize, overlap: usize, match_start: usize) -> bool {
  tail_drop <= INCREMENTAL_STITCH_MAX_SHRINK_TOKENS
    || (overlap >= INCREMENTAL_STITCH_STRONG_OVERLAP_TOKENS && match_start == 0)
}

fn overlap_merge_is_safe(
  original_left_len: usize,
  merged_len: usize,
  tail_drop: usize,
  overlap: usize,
  match_start: usize,
) -> bool {
  if !overlap_tail_drop_is_safe(tail_drop, overlap, match_start) {
    return false;
  }

  if merged_len >= original_left_len {
    return true;
  }
  let shrink = original_left_len - merged_len;
  if shrink <= INCREMENTAL_STITCH_MAX_SHRINK_TOKENS {
    return true;
  }

  // Large shrink is acceptable only when overlap is strong and anchored at the beginning of
  // the new segment, which indicates true continuation rather than an incidental phrase match.
  overlap >= INCREMENTAL_STITCH_STRONG_OVERLAP_TOKENS && match_start == 0
}

// Convert the audio-sample overlap between the previously-seen partials and the incoming one into
// an upper bound on how far into the new segment the stitch can anchor. The false-overlap bug
// happens when phrases like `[if, we, have]` recur deep in the new segment and outscore the true
// overlap near the start. Audio alignment tells us the real overlap can only be a few seconds
// long, which bounds how many tokens the true anchor could be past `right[0]`.
pub(super) fn stitch_right_start_cap_from_overlap(overlap_samples: usize) -> usize {
  overlap_samples
    .saturating_div(INCREMENTAL_STITCH_SAMPLES_PER_WORD_MAX)
    .saturating_add(INCREMENTAL_STITCH_RIGHT_START_SLACK_TOKENS)
}

pub(super) fn stitch_incremental_text(
  assembled: &str,
  next_text: &str,
  tail_window_tokens: usize,
  min_overlap_tokens: usize,
  max_right_start: Option<usize>,
  audio_overlap_samples: usize,
) -> String {
  let assembled = assembled.trim();
  let next_text = next_text.trim();
  if assembled.is_empty() {
    return next_text.to_string();
  }
  if next_text.is_empty() {
    return assembled.to_string();
  }

  let left = tokenize_for_stitch(assembled);
  let right = tokenize_for_stitch(next_text);
  if left.is_empty() {
    return next_text.to_string();
  }
  if right.is_empty() {
    return join_with_single_space(assembled, next_text);
  }

  let max_tail_drop = left
    .len()
    .saturating_sub(min_overlap_tokens)
    .min(INCREMENTAL_STITCH_MAX_TAIL_DROP_TOKENS);
  let right_end = right.len().min(tail_window_tokens);
  let right_keys = right[..right_end].iter().map(|t| t.match_key.as_str()).collect::<Vec<_>>();

  let mut best_match: Option<(usize, usize, usize)> = None; // (tail_drop, match_start, overlap)
  for tail_drop in 0..=max_tail_drop {
    let left_len = left.len().saturating_sub(tail_drop);
    if left_len == 0 {
      continue;
    }
    // Audio-range cap applies to the SUM of tail_drop and match_start, not to each in
    // isolation. A genuine overlap has left's tail matching right's head with near-zero
    // slack on both sides; a real audio-overlap window of N tokens can't plausibly need
    // both to be non-trivial simultaneously. Without this, e.g. turn 8 slipped through with
    // tail_drop=6 + match_start=7 (combined slack 13 on a 9-token budget) — left's first 4
    // tokens `[it's, not, clear, to]` matched right's middle 4 tokens of the same phrase
    // and the stitcher used left's prefix as pseudo-suffix, dropping ~10 tokens of real
    // speech.
    //
    // `tail_drop` alone exceeding the cap is the same violation from the *left* side: it
    // discards more of left's tail than the overlapping audio could have re-transcribed, so
    // it can only be anchoring on a repeated phrase deeper in left. Turn 41 (2026-07-07)
    // hit this — the speaker said "add another instance to this cluster that is beefier ...
    // and if we get more traffic then we're going to add another instance to this cluster",
    // and seg 13's head "another instance to this cluster that is beefier" matched the
    // EARLIER occurrence at tail_drop=16 (overlap 8) against a 9-token audio budget,
    // beating the genuine tail-anchored match (overlap 5) and dropping the middle clause.
    // Rejecting `tail_drop > cap` here restores the true anchor. (`cap` is never below the
    // 2-token slack floor, so ordinary ≤2-token tail corrections always survive.)
    if max_right_start.is_some_and(|cap| tail_drop > cap) {
      continue;
    }
    let adjusted_right_cap = max_right_start.map(|c| c.saturating_sub(tail_drop));
    let left_start = left_len.saturating_sub(tail_window_tokens);
    let left_keys = left[left_start..left_len]
      .iter()
      .map(|t| t.match_key.as_str())
      .collect::<Vec<_>>();
    // Audio-cutoff truncation recovery is only safe at the literal end of the prior
    // partial (`tail_drop == 0`) and only when the next partial's audio actually extends
    // past the cutoff (`audio_overlap_samples > 0`). Both conditions together mean the
    // last token of left was clipped mid-word and the next partial covers that word's
    // full audio. Without those signals the recovery would just be loose prefix-matching
    // and would weaken the existing anchor strictness.
    let boundary_recovery_eligible = tail_drop == 0 && audio_overlap_samples > 0;
    if let Some((match_start, overlap)) = best_suffix_overlap(
      &left_keys,
      &right_keys,
      min_overlap_tokens,
      adjusted_right_cap,
      boundary_recovery_eligible,
    ) {
      // Reject pseudo-suffix anchors. Two related shapes:
      //
      // 1. "Overlap covers all of truncated-left + matched in right's middle" (turn 8):
      //    `overlap_start == 0` after the truncation means the whole remaining left is the
      //    "overlap region" — left's prefix being used as a pseudo-suffix.
      //
      // 2. "Combined slack exceeds the matched length" (turn 667): both `tail_drop` and
      //    `match_start` non-zero, and their sum exceeds `overlap`. Stretching the anchor
      //    wider than the actual match length means we're displacing both sides to find a
      //    duplicate phrase rather than a genuine overlap. Turn 667 hit this at
      //    `(tail_drop=6, match_start=3, overlap=3)`: 6+3=9 just equaled the audio-derived
      //    cap so the per-axis cap allowed it, but the slack was 3× the matched length.
      //
      // Genuine end-aligned overlaps have either tail_drop=0, match_start=0, or both
      // small — never both substantially larger than the overlap itself.
      let would_be_overlap_start = left_len.saturating_sub(overlap);
      let is_pseudo_suffix_full = tail_drop > 0 && would_be_overlap_start == 0 && match_start > 0;
      let is_pseudo_suffix_stretched =
        tail_drop > 0 && match_start > 0 && tail_drop + match_start > overlap;
      if is_pseudo_suffix_full || is_pseudo_suffix_stretched {
        continue;
      }
      if !overlap_tail_drop_is_safe(tail_drop, overlap, match_start) {
        continue;
      }
      let replace = best_match
        .map(|(best_drop, best_start, best_overlap)| {
          overlap > best_overlap
            || (overlap == best_overlap && tail_drop < best_drop)
            || (overlap == best_overlap && tail_drop == best_drop && match_start > best_start)
        })
        .unwrap_or(true);
      if replace {
        best_match = Some((tail_drop, match_start, overlap));
      }
    }
  }

  if let Some((tail_drop, match_start, overlap)) = best_match {
    let left_len = left.len().saturating_sub(tail_drop);
    let overlap_start = left_len.saturating_sub(overlap);

    let mut merged_tokens = Vec::new();
    merged_tokens.extend(left[..overlap_start].iter().map(|t| t.original.as_str().to_string()));

    // Preserve lexical stability from the assembled stream when overlap is fuzzy,
    // but prefer the latest segment's exact-token punctuation/casing.
    for i in 0..overlap {
      let left_tok = &left[overlap_start + i];
      let right_tok = &right[match_start + i];
      if left_tok.match_key == right_tok.match_key {
        merged_tokens.push(right_tok.original.clone());
      } else if tokens_differ_only_in_non_alpha(&left_tok.match_key, &right_tok.match_key) {
        // Per-slot fallback: alignment already located this pair as a fuzzy
        // match, and the only diff is a non-alphabetic glyph (apostrophe,
        // hyphen, period). Both partials saw the same audio span; right is
        // the later partial with more context. Prefer its surface form
        // (e.g. `"lets"` over `"let's"` from production turn 28). Distinct
        // from the alphabetic-edit-distance default below where left's
        // longer word is preserved against chunk-boundary letter loss.
        merged_tokens.push(right_tok.original.clone());
      } else if tail_drop == 0
        && i == overlap - 1
        && audio_overlap_samples > 0
        && is_one_char_audio_cutoff_truncation(&left_tok.match_key, &right_tok.match_key)
      {
        // Audio chunk that produced left ended mid-word; right covered audio past the
        // cutoff and emitted the full token. Use right's word, not left's truncated stub.
        merged_tokens.push(right_tok.original.clone());
      } else {
        merged_tokens.push(left_tok.original.clone());
      }
    }

    merged_tokens.extend(right.iter().skip(match_start + overlap).map(|t| t.original.clone()));

    if overlap_merge_is_safe(left.len(), merged_tokens.len(), tail_drop, overlap, match_start) {
      return merged_tokens.join(" ");
    }
  }

  // If the new segment is already fully contained in the assembled tail, ignore it.
  let left_start = left.len().saturating_sub(tail_window_tokens);
  let left_keys = left[left_start..].iter().map(|t| t.match_key.as_str()).collect::<Vec<_>>();
  if let Some((_, overlap)) = best_suffix_overlap(&left_keys, &right_keys, 1, None, false) {
    if overlap == right_keys.len() {
      return assembled.to_string();
    }
  }

  let mut appended_tokens = right.iter().map(|t| t.original.as_str()).collect::<Vec<_>>();
  if appended_tokens.is_empty() {
    return assembled.to_string();
  }

  // Seam-dedup: when the multi-token anchor search and the post-loop k=1
  // boundary anchor both fail, control falls through here and we append the
  // new partial verbatim. If `assembled` ends with token X and `right[0]`
  // is also X (case-insensitive, alphabetic, len >= 2, no trailing punct
  // on the assembled tail), the join produces "X X" at the seam — exactly
  // the 255-turn population the 2026-05-08 stderr.log analysis flagged
  // (turn 62 "...swoop" + "swoop and...", turn 37 "...the" + "the model...",
  // etc.). Drop the leading duplicate so the seam comes through clean.
  if let Some(first) = appended_tokens.first().copied() {
    if is_consecutive_duplicate_at_seam(assembled, first) {
      appended_tokens.remove(0);
      if appended_tokens.is_empty() {
        return assembled.to_string();
      }
    }
  }

  join_with_single_space(assembled, &appended_tokens.join(" "))
}

/// Returns true when `next` is a consecutive duplicate of the last
/// whitespace-tokenized word in `assembled`, per the same four rules used by
/// the post-paste `collapse_consecutive_duplicates` in `crates/azad/src/app.rs`:
///
/// 1. Trailing punctuation on `assembled`'s last token is a hard break
///    (sentence boundaries, comma-separated letter-spellings, etc.).
/// 2. Both the assembled-tail word and `next` must be alphabetic-only
///    (after stripping leading/trailing punctuation). Protects digits and
///    mixed-form tokens (`M3`, `1st`, `2288`).
/// 3. The shared alpha key must be at least 2 chars. Protects single-letter
///    spellings (`M M`, `S S`).
/// 4. Comparison is case-insensitive on the alpha key.
fn is_consecutive_duplicate_at_seam(assembled: &str, next: &str) -> bool {
  let Some(prev) = assembled.split_whitespace().next_back() else {
    return false;
  };
  // Rule 1.
  if prev.chars().last().map(|c| !c.is_alphanumeric()).unwrap_or(true) {
    return false;
  }
  // Rule 2.
  if !is_alpha_word_seam(prev) || !is_alpha_word_seam(next) {
    return false;
  }
  let prev_alpha = alpha_key_seam(prev);
  let next_alpha = alpha_key_seam(next);
  // Rule 3.
  if prev_alpha.chars().count() < 2 {
    return false;
  }
  // Rule 4.
  prev_alpha == next_alpha
}

fn alpha_key_seam(s: &str) -> String {
  s.chars().filter(|c| c.is_alphabetic()).flat_map(|c| c.to_lowercase()).collect()
}

fn is_alpha_word_seam(s: &str) -> bool {
  let core = s.trim_matches(|c: char| !c.is_alphanumeric());
  !core.is_empty() && core.chars().all(|c| c.is_alphabetic())
}

fn best_suffix_overlap(
  left_tail_keys: &[&str],
  right_keys: &[&str],
  min_overlap_tokens: usize,
  max_right_start: Option<usize>,
  boundary_recovery_eligible: bool,
) -> Option<(usize, usize)> {
  if left_tail_keys.is_empty() || right_keys.is_empty() {
    return None;
  }

  let max_overlap = left_tail_keys.len().min(right_keys.len());
  // Run the standard k>=min_overlap_tokens search whenever the configured floor is
  // achievable. When it isn't (tiny windows where max_overlap < min) we skip the
  // loop and fall through to the post-loop k=1 boundary anchor below.
  if max_overlap >= min_overlap_tokens {
    for k in (min_overlap_tokens..=max_overlap).rev() {
      let left_suffix = &left_tail_keys[left_tail_keys.len() - k..];
      let last_possible_start = right_keys.len() - k;
      let start_cap = max_right_start
        .map(|c| c.min(last_possible_start))
        .unwrap_or(last_possible_start);
      // Prefer later matches in the right segment so we drop as much repeated prefix as possible.
      for start in (0..=start_cap).rev() {
        if slice_tokens_match(left_suffix, &right_keys[start..start + k]) {
          return Some((start, k));
        }
        // Audio-chunk-boundary truncation recovery: same slice except the last position
        // differs by a 1-character extension on right's side (e.g. `"ur"` vs `"url"`).
        // The leading k-1 positions still must match by ordinary `slice_tokens_match`
        // rules, so we never fire on a single-token "overlap" without surrounding
        // context. Caller guarantees `boundary_recovery_eligible` is only set at the
        // literal end of the prior partial AND when the next partial's audio extends
        // past the cutoff.
        if boundary_recovery_eligible
          && k >= 2
          && slice_tokens_match(&left_suffix[..k - 1], &right_keys[start..start + k - 1])
          && is_one_char_audio_cutoff_truncation(left_suffix[k - 1], right_keys[start + k - 1])
        {
          return Some((start, k));
        }
      }
    }
  }

  // Single-token seam anchor. When the standard k>=min search finds nothing AND
  // boundary recovery is eligible, fire iff left's literal LAST token equals
  // right's literal FIRST token AND the matched key is substantive (>= 3 chars).
  // Captures the "same word at the seam, both partials saw it" pattern from
  // production turn 23 (2026-04-30): partial 1 ended `"...outcome."` and partial
  // 2 started `"outcome uh ..."`; the audio overlap was 30_720 samples (~1.92 s)
  // and both decoders independently transcribed the same `"outcome"`. Without
  // this branch the `min_overlap_tokens=2` floor rejected the seam and the
  // stitcher emitted `"outcome. outcome uh"` — a duplicated word.
  //
  // Strictness preserved: requires literal key equality, never widens to fuzzy
  // match. The substantive-length filter keeps short particles (`"of"`, `"is"`,
  // `"i"`) from anchoring on coincidence; the `boundary_recovery_eligible` gate
  // (caller-side: `tail_drop == 0 && audio_overlap_samples > 0`) keeps it tied
  // to the actual end-of-prior-partial seam.
  if boundary_recovery_eligible {
    if let (Some(&last), Some(&first)) = (left_tail_keys.last(), right_keys.first()) {
      if tokens_match_substantive_boundary(last, first) {
        return Some((0, 1));
      }
    }
  }

  None
}

fn slice_tokens_match(left: &[&str], right: &[&str]) -> bool {
  if left.len() != right.len() {
    return false;
  }
  left.iter().zip(right.iter()).all(|(a, b)| tokens_equivalent(a, b))
}

/// Boundary-only recovery: returns true when `left` is a strict 1-character-shorter prefix
/// of `right` (`"ur"` of `"url"`, `"thi"` of `"this"`). NEVER call this from
/// `tokens_equivalent` or anywhere outside the narrow "actual end of prior partial, next
/// partial covers audio past the cutoff" branch — the whole point is that the audio cut
/// off mid-word, so the model emitted one phoneme short of the full token. Generalising
/// would weaken the stitcher's anchor strictness and re-open the pseudo-suffix and
/// combined-slack regressions (turns 16, 80, 667, 8, 237).
///
/// `left.len() >= 2` rejects single-character noise like `"a"` or `"i"`. The strict 1-char
/// extension (rather than `≤ N`) keeps the rule tied to the "one phoneme of audio missing"
/// shape; if future audio cutoffs need 2-char tolerance we revisit with fresh evidence.
pub(super) fn is_one_char_audio_cutoff_truncation(left: &str, right: &str) -> bool {
  left.len() >= 2 && right.len() == left.len() + 1 && right.starts_with(left)
}

/// Per-slot merge-time fallback: returns true when `left` and `right` are equal
/// once non-alphabetic characters are filtered out — i.e. the only diff is a
/// punctuation glyph (apostrophe, hyphen, period, …) somewhere in the token.
///
/// Used by the `stitch_incremental_text` merge loop to decide which side's
/// `original` to write into the merged output for a token slot whose alignment
/// is already locked in but whose `match_key`s differ. When the disagreement is
/// purely punctuation, right (the later, higher-context partial) is preferred;
/// when the disagreement is alphabetic (e.g. `"caused"` vs `"cause"`) the
/// existing "left wins" default applies and preserves the longer/older form
/// against chunk-boundary letter loss.
///
/// Does NOT participate in tokenization, key normalization, or anchor search:
/// punctuation remains significant everywhere alignment is decided. Two
/// distinct tokens `"let's"` and `"lets"` keep distinct keys and never collapse
/// at search time; this check only fires after the slot-to-slot mapping is
/// fixed, so it cannot fold separate words together.
pub(super) fn tokens_differ_only_in_non_alpha(left: &str, right: &str) -> bool {
  if left == right {
    return false;
  }
  let left_alpha: String = left.chars().filter(|c| c.is_alphabetic()).collect();
  let right_alpha: String = right.chars().filter(|c| c.is_alphabetic()).collect();
  !left_alpha.is_empty() && left_alpha == right_alpha
}

/// Single-token boundary anchor: returns true when `left` and `right` are the SAME
/// normalized match-key AND the token is substantive enough that anchoring at the
/// seam is structurally informative — not a coincidence on a 1-2 char particle.
///
/// Used solely from the post-loop branch in `best_suffix_overlap` when boundary
/// recovery is eligible (`tail_drop == 0 && audio_overlap_samples > 0`). Never call
/// from `tokens_equivalent` or any general-purpose comparison — the whole point is
/// that this is a structural exception (the same word reappears at the audio seam
/// because both partials transcribed the same overlapping audio), not a generic
/// equivalence rule.
///
/// `len >= 3` mirrors the short-token rejection in `tokens_equivalent` and rules
/// out 1-2 char particles (`"i"`, `"a"`, `"of"`, `"to"`, `"is"`, `"at"`) where the
/// false-anchor risk dominates. 3-char common words (`"the"`, `"and"`, `"for"`,
/// etc.) are safe because the structural gate — audio overlap exists AND
/// `tail_drop == 0` — means partial 2's first token covers the same audio span
/// as partial 1's last token; same-token-at-seam is genuine evidence of overlap.
pub(super) fn tokens_match_substantive_boundary(left: &str, right: &str) -> bool {
  left == right && left.len() >= 3
}

pub(super) fn tokens_equivalent(a: &str, b: &str) -> bool {
  if a == b {
    return true;
  }
  if a.is_empty() || b.is_empty() {
    return false;
  }
  if a.len().abs_diff(b.len()) > 1 {
    return false;
  }
  // Edit-distance-1 is only meaningful as a typo signal when both tokens are long enough that
  // one differing character leaves most of the token intact. Short tokens like `I` vs `s`,
  // `at` vs `it`, or `of` vs `if` are distinct words, not typos — allowing them to match
  // fuzzily produces false overlaps that anchor the stitcher several tokens into the new
  // segment and drop real content. Require both sides to be ≥3 chars before fuzzing.
  if a.len() < 3 || b.len() < 3 {
    return false;
  }
  edit_distance_at_most_one(a, b)
}

fn edit_distance_at_most_one(a: &str, b: &str) -> bool {
  let a = a.as_bytes();
  let b = b.as_bytes();
  let mut i = 0usize;
  let mut j = 0usize;
  let mut edits = 0usize;
  while i < a.len() && j < b.len() {
    if a[i] == b[j] {
      i += 1;
      j += 1;
      continue;
    }
    edits += 1;
    if edits > 1 {
      return false;
    }
    if a.len() == b.len() {
      i += 1;
      j += 1;
    } else if a.len() > b.len() {
      i += 1;
    } else {
      j += 1;
    }
  }
  edits += (a.len() - i) + (b.len() - j);
  edits <= 1
}

pub(super) fn tokenize_for_stitch(text: &str) -> Vec<StitchToken> {
  let raw: Vec<&str> = text.split_whitespace().collect();
  let mut tokens = Vec::new();
  let mut i = 0;
  while i < raw.len() {
    if let Some((consumed, digits)) = try_consume_number_run(&raw, i) {
      let original = raw[i..i + consumed].join(" ");
      tokens.push(StitchToken { original, match_key: digits });
      i += consumed;
      continue;
    }
    let word = raw[i];
    let key = normalize_stitch_token(word);
    if !key.is_empty() {
      tokens.push(StitchToken { original: word.to_string(), match_key: key });
    }
    i += 1;
  }
  tokens
}

pub(super) fn normalize_stitch_token(token: &str) -> String {
  token.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// Number-form classification for the cardinal-run grouper. Returns the canonical
/// digit-string per token (`"eighteen"` → `"18"`) plus the structural role used to
/// decide between the tens-rule and the concat-rule when several words form a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberWord {
  /// A bare digit string in the input, e.g. `"318"` → `Digit("318")`.
  Digit,
  /// `zero`–`nine` (cardinal). Held as decimal "0".."9".
  Ones,
  /// `ten`–`nineteen`. Held as decimal "10".."19".
  Teen,
  /// `twenty`,`thirty`,…,`ninety`. Held as decimal "20","30",…,"90".
  Tens,
  /// The literal word `"hundred"`. Multiplier with a prior `Ones` (`one hundred` = 100).
  Hundred,
}

/// Classify a normalized cardinal word for number-run grouping. Returns `None` for
/// ordinals (`"third"`, `"eighteenth"` — they refer to position, not value), thousands
/// or higher (out of scope), and any non-cardinal token. The `digits` return is the
/// per-word decimal string the grouper concatenates under the concat-rule.
fn classify_number_word(token: &str) -> Option<(NumberWord, String)> {
  if token.is_empty() {
    return None;
  }
  if token.bytes().all(|b| b.is_ascii_digit()) {
    return Some((NumberWord::Digit, token.to_string()));
  }
  match token {
    "zero" => Some((NumberWord::Ones, "0".into())),
    "one" => Some((NumberWord::Ones, "1".into())),
    "two" => Some((NumberWord::Ones, "2".into())),
    "three" => Some((NumberWord::Ones, "3".into())),
    "four" => Some((NumberWord::Ones, "4".into())),
    "five" => Some((NumberWord::Ones, "5".into())),
    "six" => Some((NumberWord::Ones, "6".into())),
    "seven" => Some((NumberWord::Ones, "7".into())),
    "eight" => Some((NumberWord::Ones, "8".into())),
    "nine" => Some((NumberWord::Ones, "9".into())),
    "ten" => Some((NumberWord::Teen, "10".into())),
    "eleven" => Some((NumberWord::Teen, "11".into())),
    "twelve" => Some((NumberWord::Teen, "12".into())),
    "thirteen" => Some((NumberWord::Teen, "13".into())),
    "fourteen" => Some((NumberWord::Teen, "14".into())),
    "fifteen" => Some((NumberWord::Teen, "15".into())),
    "sixteen" => Some((NumberWord::Teen, "16".into())),
    "seventeen" => Some((NumberWord::Teen, "17".into())),
    "eighteen" => Some((NumberWord::Teen, "18".into())),
    "nineteen" => Some((NumberWord::Teen, "19".into())),
    "twenty" => Some((NumberWord::Tens, "20".into())),
    "thirty" => Some((NumberWord::Tens, "30".into())),
    "forty" => Some((NumberWord::Tens, "40".into())),
    "fifty" => Some((NumberWord::Tens, "50".into())),
    "sixty" => Some((NumberWord::Tens, "60".into())),
    "seventy" => Some((NumberWord::Tens, "70".into())),
    "eighty" => Some((NumberWord::Tens, "80".into())),
    "ninety" => Some((NumberWord::Tens, "90".into())),
    "hundred" => Some((NumberWord::Hundred, "100".into())),
    _ => None,
  }
}

fn raw_token_ends_run(raw: &str) -> bool {
  let trimmed = raw.trim_end_matches(|c: char| c.is_whitespace());
  matches!(trimmed.chars().last(), Some('.') | Some('!') | Some('?'))
}

/// Greedily consume a run of number-form tokens starting at `raw[start]`. Returns
/// `Some((consumed, canonical_digits))` where `consumed` is the number of input tokens
/// absorbed and `canonical_digits` is the resulting `match_key` (digit-only string).
/// Returns `None` if the token at `start` isn't a recognised number word.
///
/// Resolution order:
/// - **Tens-rule** when the run reads as a normal English cardinal expression:
///   `[Ones]`, `[Teen]`, `[Tens]`, `[Tens, Ones]`, `[Ones, Hundred]`,
///   `[Ones, Hundred, And, <tail>]`, `[Ones, Hundred, <tail>]`. Decimal value.
/// - **Concat-rule fallback** when every consumed token is a digit-bearing cardinal
///   (no `Hundred`, no `And`) and the tens-rule didn't apply. Concatenates per-token
///   digit strings: `[Ones("3"), Teen("18")]` → `"318"`. Captures the flight-number /
///   phone-number / room-number reading common in ASR.
///
/// A token whose ORIGINAL surface form ends with `.`, `!`, or `?` terminates the run
/// after being consumed (the next iteration starts a fresh run).
///
/// Out of scope: ordinals, thousands+, decimals, and the indefinite-article `"a"` =
/// `"one"` reading (`"a hundred"`). Add grammar arms when real recordings need them.
pub(super) fn try_consume_number_run(raw: &[&str], start: usize) -> Option<(usize, String)> {
  if start >= raw.len() {
    return None;
  }
  // Greedy phase: consume cardinals + at most one valid `and` connector.
  // `classified` only stores cardinals; the `and` token is tracked separately because
  // it's a connector, not a value-carrying word.
  let mut classified: Vec<(NumberWord, String)> = Vec::new();
  let mut and_position: Option<usize> = None;
  while start + classified.len() + and_position.map_or(0, |_| 1) < raw.len() {
    let pos = start + classified.len() + and_position.map_or(0, |_| 1);
    let raw_word = raw[pos];
    let normalized = normalize_stitch_token(raw_word);
    if normalized == "and" {
      // Only valid AFTER a `Hundred` and only once per run.
      let prior_hundred = classified
        .last()
        .map(|(w, _)| matches!(w, NumberWord::Hundred))
        .unwrap_or(false);
      if !prior_hundred || and_position.is_some() {
        break;
      }
      and_position = Some(classified.len());
      if raw_token_ends_run(raw_word) {
        break;
      }
      continue;
    }
    let Some(class) = classify_number_word(&normalized) else {
      break;
    };
    classified.push(class);
    if raw_token_ends_run(raw_word) {
      break;
    }
  }
  // Retract a dangling `and` (no cardinal followed it).
  if let Some(and_idx) = and_position {
    if classified.len() == and_idx {
      and_position = None;
    }
  }
  if classified.is_empty() {
    return None;
  }
  // Resolution: prefer the longest prefix that resolves cleanly via tens-rule, then
  // fall back to concat-rule on shorter prefixes if needed. Single-cardinal runs
  // resolve via tens-rule (`[Ones]`, `[Teen]`, `[Tens]`, `[Digit]`), so a chain like
  // `"one two three"` collapses position-by-position into individual digit-keyed
  // tokens (`"1"`, `"2"`, `"3"`), preserving the existing token-by-token anchoring
  // behaviour for those cases.
  for k in (1..=classified.len()).rev() {
    let prefix = &classified[..k];
    let has_and_in_prefix = and_position.map(|p| p < k).unwrap_or(false);
    let consumed_at_k = if has_and_in_prefix { k + 1 } else { k };
    if let Some(value) = resolve_tens_rule(prefix, has_and_in_prefix) {
      return Some((consumed_at_k, value.to_string()));
    }
    // Concat-rule: only for runs of 2+ cardinals, no `Hundred`, no `and`, and at
    // least one Teen/Tens/multi-digit Digit (so we don't over-group chains of
    // single-digit cardinals like `"one two three"` into a phantom `"123"` — those
    // should remain individual tokens so left and right anchor position-by-position
    // when both partials use the same form).
    if k >= 2
      && !prefix.iter().any(|(w, _)| matches!(w, NumberWord::Hundred))
      && !has_and_in_prefix
      && prefix.iter().any(|(w, d)| {
        matches!(w, NumberWord::Teen | NumberWord::Tens)
          || (matches!(w, NumberWord::Digit) && d.len() >= 2)
      })
    {
      let mut digits = String::new();
      for (_, d) in prefix {
        digits.push_str(d);
      }
      if !digits.is_empty() {
        return Some((consumed_at_k, digits));
      }
    }
  }
  None
}

/// Try to read `classified` as a normal English cardinal expression. Returns the
/// decimal value when the shape matches, `None` otherwise. `has_and` reports whether
/// an `"and"` connector appeared between `Hundred` and the tail.
fn resolve_tens_rule(classified: &[(NumberWord, String)], has_and: bool) -> Option<u64> {
  use NumberWord::*;
  let words: Vec<NumberWord> = classified.iter().map(|(w, _)| *w).collect();
  let value_at = |i: usize| -> u64 { classified[i].1.parse::<u64>().unwrap_or(0) };
  match words.as_slice() {
    [Ones] | [Teen] | [Tens] | [Digit] => {
      if has_and {
        return None;
      }
      Some(value_at(0))
    }
    [Tens, Ones] => {
      if has_and {
        return None;
      }
      Some(value_at(0) + value_at(1))
    }
    [Ones, Hundred] => {
      if has_and {
        return None;
      }
      Some(value_at(0) * 100)
    }
    [Ones, Hundred, Ones]
    | [Ones, Hundred, Teen]
    | [Ones, Hundred, Tens]
    | [Ones, Hundred, Tens, Ones] => {
      let head = value_at(0) * 100;
      let tail: u64 = classified[2..].iter().map(|(_, d)| d.parse::<u64>().unwrap_or(0)).sum();
      Some(head + tail)
    }
    _ => None,
  }
}

// The streaming tokenizer can emit capitalized word tokens (e.g. `That`) when it treats a brief
// audio pause as a sentence boundary. A single chunk can contain many such words. At every word
// boundary inside the chunk, lower the leading ASCII capital unless (a) the last non-whitespace
// char across prior+chunk is terminal punctuation, or (b) the word looks like an acronym or a
// single-letter word. Acronyms are detected by peeking at the next char: if it's another
// uppercase ASCII letter, preserve (CPU, NASA, USA, I'd). A single-letter word (`I`) is
// preserved because lowercasing it mangles the pronoun. Non-ASCII uppercase passes through so
// non-Latin scripts aren't touched.
pub(super) fn normalize_chunk_case(prior: &str, chunk: String) -> String {
  let mut last_non_ws: Option<char> = prior.trim_end().chars().last();
  let mut prev_alpha = last_non_ws.map(|c| c.is_alphabetic()).unwrap_or(false);
  let chars: Vec<char> = chunk.chars().collect();
  let mut out = String::with_capacity(chunk.len());

  for (i, &c) in chars.iter().enumerate() {
    let at_word_start = c.is_alphabetic() && !prev_alpha;
    let pushed = if at_word_start && c.is_ascii_uppercase() {
      let at_sentence_start = match last_non_ws {
        None => true,
        Some(x) => matches!(x, '.' | '!' | '?'),
      };
      let next = chars.get(i + 1).copied();
      let preserve_acronym_or_single = match next {
        None => true,
        Some(n) if !n.is_alphabetic() => true,
        Some(n) if n.is_ascii_uppercase() => true,
        _ => false,
      };
      if at_sentence_start || preserve_acronym_or_single { c } else { c.to_ascii_lowercase() }
    } else {
      c
    };
    out.push(pushed);
    if !pushed.is_whitespace() {
      last_non_ws = Some(pushed);
    }
    prev_alpha = pushed.is_alphabetic();
  }
  out
}

fn join_with_single_space(left: &str, right: &str) -> String {
  let left = left.trim();
  let right = right.trim();
  if left.is_empty() {
    return right.to_string();
  }
  if right.is_empty() {
    return left.to_string();
  }
  format!("{left} {right}")
}

pub(super) struct StitchToken {
  pub(super) original: String,
  pub(super) match_key: String,
}
