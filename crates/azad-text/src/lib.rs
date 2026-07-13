// English number-word parsing is adapted from allo-media/text2num-rs' English
// language rules, licensed MIT:
// https://github.com/allo-media/text2num-rs/blob/master/src/lang/en/mod.rs
// Copyright (c) 2021-2024 Groupe Allo-Media.

mod generated {
    pub(crate) mod emoji_phrases {
        include!(concat!(env!("OUT_DIR"), "/emoji_phrases.rs"));
    }
}

use generated::emoji_phrases::{EMOJI_PHRASES, MAX_EMOJI_PHRASE_WORDS};

pub struct PasteTextOptions<'a> {
    pub append_trailing_space: bool,
    pub removed_words: &'a [String],
    pub deduplicate_words: bool,
    pub convert_number_words: bool,
    pub convert_spoken_emoji: bool,
    pub lowercase_except_uppercase_words: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayTextOptions<'a> {
    pub removed_words: &'a [String],
    pub deduplicate_words: bool,
    pub convert_number_words: bool,
    pub convert_spoken_emoji: bool,
    pub lowercase_except_uppercase_words: bool,
}

impl<'a> PasteTextOptions<'a> {
    fn display_options(&self) -> DisplayTextOptions<'a> {
        DisplayTextOptions {
            removed_words: self.removed_words,
            deduplicate_words: self.deduplicate_words,
            convert_number_words: self.convert_number_words,
            convert_spoken_emoji: self.convert_spoken_emoji,
            lowercase_except_uppercase_words: self.lowercase_except_uppercase_words,
        }
    }
}

pub fn build_display_text(text: &str, options: DisplayTextOptions<'_>) -> String {
    let mut display_text = if options.removed_words.is_empty() {
        text.to_string()
    } else {
        strip_removed_words(text, options.removed_words)
    };
    if options.convert_number_words {
        display_text = replace_english_number_words(&display_text);
        display_text = attach_percent_sign_to_numbers(&display_text);
    }
    if options.convert_spoken_emoji {
        display_text = replace_spoken_emoji_names(&display_text);
    }
    if options.deduplicate_words {
        display_text = collapse_consecutive_duplicates(&display_text);
    }
    if options.lowercase_except_uppercase_words {
        display_text = lowercase_except_uppercase_words(&display_text);
    }
    display_text
}

pub fn build_paste_text(text: &str, options: PasteTextOptions<'_>) -> String {
    let mut paste_text = build_display_text(text, options.display_options());
    paste_text = strip_single_word_terminal_period(&paste_text);
    if options.append_trailing_space
        && !paste_text
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
    {
        paste_text.push(' ');
    }
    paste_text
}

fn strip_single_word_terminal_period(text: &str) -> String {
    let trimmed = text.trim_end_matches(char::is_whitespace);
    if !trimmed.ends_with('.') {
        return text.to_string();
    }

    let before_period = &trimmed[..trimmed.len() - 1];
    let tokens: Vec<TextToken<'_>> = tokenize(before_period).collect();
    let word_count = tokens.iter().filter(|token| token.is_word).count();
    let is_single_word = word_count == 1;
    // A lone emoji is a single "unit" too: saying "happy emoji" yields "😊", which should
    // paste as "😊", not "😊." — auto-punctuation shouldn't tack a period onto a bare emoji.
    let visible_token_count = tokens
        .iter()
        .filter(|token| !token.text.chars().all(char::is_whitespace))
        .count();
    let is_single_emoji =
        word_count == 0 && visible_token_count == 1 && before_period.chars().any(is_emoji_char);
    if !is_single_word && !is_single_emoji {
        return text.to_string();
    }

    let trailing_whitespace = &text[trimmed.len()..];
    format!("{before_period}{trailing_whitespace}")
}

/// True for characters in the common emoji / pictographic Unicode blocks. Used to recognize a
/// bare-emoji paste so auto-punctuation doesn't append a period to it.
fn is_emoji_char(ch: char) -> bool {
    matches!(ch as u32,
        0x1F000..=0x1FAFF   // symbols, pictographs, emoticons, supplemental symbols
        | 0x2600..=0x27BF   // misc symbols + dingbats (✅ ⚫ ⚪ …)
        | 0x2B00..=0x2BFF   // misc symbols & arrows (⭐ ⬛ …)
        | 0xFE00..=0xFE0F   // variation selectors
        | 0x200D            // zero-width joiner (emoji sequences)
    )
}

fn lowercase_except_uppercase_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            word.push(ch);
        } else {
            flush_lowercase_word(&mut out, &mut word);
            out.push(ch);
        }
    }
    flush_lowercase_word(&mut out, &mut word);
    out
}

fn flush_lowercase_word(out: &mut String, word: &mut String) {
    if word.is_empty() {
        return;
    }

    if should_preserve_uppercase_word(word) {
        out.push_str(word);
    } else {
        out.extend(word.chars().flat_map(|ch| ch.to_lowercase()));
    }
    word.clear();
}

fn should_preserve_uppercase_word(word: &str) -> bool {
    uppercase_alphanumeric_identifier(word)
        || uppercase_alpha_count(word) >= 2
        || uppercase_alpha_count(word.strip_suffix("es").unwrap_or_default()) >= 2
        || uppercase_alpha_count(word.strip_suffix('s').unwrap_or_default()) >= 2
}

fn uppercase_alphanumeric_identifier(word: &str) -> bool {
    let mut has_alpha = false;
    let mut has_digit = false;
    for ch in word.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
        } else if ch.is_ascii_alphabetic() {
            if !ch.is_ascii_uppercase() {
                return false;
            }
            has_alpha = true;
        } else {
            return false;
        }
    }
    has_alpha && has_digit
}

fn uppercase_alpha_count(text: &str) -> usize {
    let mut alpha_count = 0;
    for ch in text.chars().filter(|ch| ch.is_alphabetic()) {
        alpha_count += 1;
        if !ch.is_uppercase() {
            return 0;
        }
    }
    alpha_count
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

fn collapse_consecutive_duplicates(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 2 {
        return text.to_string();
    }
    if !tokens
        .windows(2)
        .any(|w| is_consecutive_duplicate(w[0], w[1]))
    {
        return text.to_string();
    }
    let mut kept: Vec<&str> = Vec::new();
    for tok in tokens {
        let should_collapse = kept
            .last()
            .is_some_and(|prev| is_consecutive_duplicate(prev, tok));
        if should_collapse {
            kept.pop();
        }
        kept.push(tok);
    }
    kept.join(" ")
}

fn replace_spoken_emoji_names(text: &str) -> String {
    let tokens: Vec<TextToken> = tokenize(text).collect();
    if tokens.is_empty() || !tokens.iter().any(|token| is_emoji_trigger_word(token.text)) {
        return text.to_string();
    }

    let replacements = emoji_replacements(&tokens);
    if replacements.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut replacement_index = 0;
    while i < tokens.len() {
        if let Some((start, end, emoji)) = replacements.get(replacement_index).copied() {
            if i == start {
                out.push_str(emoji);
                i = end + 1;
                replacement_index += 1;
                continue;
            }
        }
        out.push_str(tokens[i].text);
        i += 1;
    }
    out
}

fn emoji_replacements<'a>(tokens: &[TextToken<'a>]) -> Vec<(usize, usize, &'static str)> {
    let mut replacements = Vec::new();
    let mut covered_until = None;
    for (trigger_index, token) in tokens.iter().enumerate() {
        if covered_until.is_some_and(|end| trigger_index <= end)
            || !is_emoji_trigger_word(token.text)
        {
            continue;
        }

        if let Some((start, emoji)) = emoji_replacement_ending_at(tokens, trigger_index) {
            replacements.push((start, trigger_index, emoji));
            covered_until = Some(trigger_index);
        }
    }
    replacements
}

fn emoji_replacement_ending_at(
    tokens: &[TextToken<'_>],
    trigger_index: usize,
) -> Option<(usize, &'static str)> {
    let mut word_indices = Vec::new();
    let mut word_index = previous_whitespace_separated_word(tokens, trigger_index)?;
    loop {
        word_indices.push(word_index);
        if word_indices.len() >= MAX_EMOJI_PHRASE_WORDS {
            break;
        }
        let Some(prev_word_index) = previous_whitespace_separated_word(tokens, word_index) else {
            break;
        };
        word_index = prev_word_index;
    }
    word_indices.reverse();

    for start_offset in 0..word_indices.len() {
        let candidate = &word_indices[start_offset..];
        let key = emoji_phrase_key(tokens, candidate);
        if let Some(emoji) = EMOJI_PHRASES.get(&key) {
            return Some((candidate[0], *emoji));
        }
    }
    None
}

fn previous_whitespace_separated_word(
    tokens: &[TextToken<'_>],
    word_index: usize,
) -> Option<usize> {
    if word_index < 2 {
        return None;
    }
    let sep_index = word_index - 1;
    let prev_index = word_index - 2;
    if tokens[sep_index].is_word
        || !tokens[sep_index].text.chars().all(char::is_whitespace)
        || !tokens[prev_index].is_word
    {
        return None;
    }
    Some(prev_index)
}

fn emoji_phrase_key(tokens: &[TextToken<'_>], word_indices: &[usize]) -> String {
    let mut out = String::new();
    for token_index in word_indices.iter().copied() {
        push_normalized_emoji_phrase_part(&mut out, tokens[token_index].text);
    }
    out.trim().to_string()
}

fn push_normalized_emoji_phrase_part(out: &mut String, part: &str) {
    for ch in part.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
}

fn is_emoji_trigger_word(word: &str) -> bool {
    word.eq_ignore_ascii_case("emoji") || word.eq_ignore_ascii_case("emojis")
}

fn is_consecutive_duplicate(prev: &str, curr: &str) -> bool {
    if prev
        .chars()
        .last()
        .map(|c| !c.is_alphanumeric())
        .unwrap_or(true)
    {
        return false;
    }
    if !is_alpha_word(prev) || !is_alpha_word(curr) {
        return false;
    }
    let prev_alpha = alpha_key(prev);
    let curr_alpha = alpha_key(curr);
    if prev_alpha.chars().count() < 2 {
        return false;
    }
    prev_alpha == curr_alpha
}

fn alpha_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphabetic())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn is_alpha_word(s: &str) -> bool {
    let core = s.trim_matches(|c: char| !c.is_alphanumeric());
    !core.is_empty() && core.chars().all(|c| c.is_alphabetic())
}

fn replace_english_number_words(text: &str) -> String {
    let tokens: Vec<TextToken> = tokenize(text).collect();
    if tokens.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut i = 0;
    while i < tokens.len() {
        if !tokens[i].is_word {
            out.push_str(tokens[i].text);
            i += 1;
            continue;
        }

        if let Some((end, replacement)) = hash_number_phrase(&tokens, i) {
            out.push_str(&replacement);
            i = end + 1;
            changed = true;
            continue;
        }

        if let Some((end, replacement)) = identifier_number_phrase(&tokens, i) {
            out.push_str(&replacement);
            i = end + 1;
            changed = true;
            continue;
        }

        let Some(candidates) = number_candidates(&tokens, i) else {
            out.push_str(tokens[i].text);
            i += 1;
            continue;
        };

        if let Some((end, replacement)) = candidates.into_iter().rev().find_map(|(end, words)| {
            parse_number_words(&words).map(|replacement| (end, replacement))
        }) {
            out.push_str(&replacement);
            i = end + 1;
            changed = true;
        } else {
            out.push_str(tokens[i].text);
            i += 1;
        }
    }

    if changed { out } else { text.to_string() }
}

/// Spoken issue/ticket form: "number three" / "number twenty five" / "number 3" → "#3" / "#25".
/// Single unit digits are allowed here (unlike bare "three", which stays prose) because the leading
/// "number" is the intentional signal. Ordinals and decimals are left alone.
fn hash_number_phrase(tokens: &[TextToken<'_>], start: usize) -> Option<(usize, String)> {
    let start_token = tokens.get(start)?;
    if !start_token.is_word || !start_token.text.eq_ignore_ascii_case("number") {
        return None;
    }
    let next_word_index = next_whitespace_separated_word(tokens, start)?;
    let next = tokens[next_word_index];
    if !next.text.is_empty() && next.text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some((next_word_index, format!("#{}", next.text)));
    }
    let (number_end, number) = hash_number_at(tokens, next_word_index)?;
    Some((number_end, format!("#{number}")))
}

fn hash_number_at(tokens: &[TextToken<'_>], start: usize) -> Option<(usize, String)> {
    let candidates = number_candidates(tokens, start)?;
    // Decimals/ordinals after "number" are not issue IDs — leave the whole phrase for generic
    // conversion ("number three point five" → "number 3.5", "number twenty first" → "number 21st").
    // Falling back past those words would yield "#3 point five" / "#20 first".
    if candidates.iter().any(|(_, words)| {
        words.iter().any(|word| {
            word.as_str() == "point"
                || (ordinal_word_value(word).is_some() && decimal_digit(word).is_none())
        })
    }) {
        return None;
    }
    // Longest match first so "number one two three and …" becomes "#123 and …" (trailing "and"
    // is a number-part for "one hundred and three", so the maximal span can overshoot).
    candidates
        .into_iter()
        .rev()
        .find_map(|(end, words)| parse_hash_number_words(&words).map(|number| (end, number)))
}

fn parse_hash_number_words(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    if let [word] = words {
        if let Some(digit) = decimal_digit(word) {
            return Some(char::from(b'0' + digit).to_string());
        }
    }
    if let Some(digits) = parse_digit_word_sequence(words) {
        return Some(digits);
    }
    let cardinal = parse_cardinal(words)?;
    if scale_only_magnitude(words) {
        return None;
    }
    Some(cardinal)
}

/// After spoken-number replacement, rewrite `<number> percent` as `<number>%` (e.g. "12 percent"
/// -> "12%"), gated by the same number-words setting. The digit run adjacent to the word "percent"
/// is always the number's last segment, so integers, decimals ("3.5 percent" -> "3.5%"), and
/// grouped numbers ("1,000 percent" -> "1,000%") all rewrite correctly. The whole-word match leaves
/// "percentage" and a bare "percent" with no preceding number untouched.
fn attach_percent_sign_to_numbers(text: &str) -> String {
    let tokens: Vec<TextToken> = tokenize(text).collect();
    if tokens.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        let number_before_percent = token.is_word
            && !token.text.is_empty()
            && token.text.bytes().all(|byte| byte.is_ascii_digit())
            && tokens
                .get(i + 1)
                .is_some_and(|sep| !sep.is_word && sep.text.chars().all(char::is_whitespace))
            && tokens
                .get(i + 2)
                .is_some_and(|word| word.is_word && word.text.eq_ignore_ascii_case("percent"));
        if number_before_percent {
            out.push_str(token.text);
            out.push('%');
            i += 3;
            changed = true;
        } else {
            out.push_str(token.text);
            i += 1;
        }
    }

    if changed { out } else { text.to_string() }
}

fn identifier_number_phrase(tokens: &[TextToken<'_>], start: usize) -> Option<(usize, String)> {
    let start_token = tokens.get(start)?;
    if !is_identifier_phrase_start_token(start_token.text) {
        return None;
    }

    let mut out = start_token.text.to_string();
    let mut word_index = start;
    let mut consumed_spoken_number = false;

    while let Some(next_word_index) = next_whitespace_separated_word(tokens, word_index) {
        let next = tokens[next_word_index];
        if is_identifier_code_token(next.text) {
            out.push_str(next.text);
            word_index = next_word_index;
            continue;
        }

        let Some((number_end, number)) = identifier_number_at(tokens, next_word_index) else {
            break;
        };
        out.push_str(&number);
        word_index = number_end;
        consumed_spoken_number = true;
    }

    consumed_spoken_number.then_some((word_index, out))
}

fn identifier_number_at(tokens: &[TextToken<'_>], start: usize) -> Option<(usize, String)> {
    number_candidates(tokens, start)?
        .into_iter()
        .rev()
        .find_map(|(end, words)| parse_identifier_number_words(&words).map(|number| (end, number)))
}

fn parse_identifier_number_words(words: &[String]) -> Option<String> {
    if let [word] = words {
        if let Some(digit) = decimal_digit(word) {
            return Some(char::from(b'0' + digit).to_string());
        }
    }
    parse_number_words(words)
}

fn next_whitespace_separated_word(tokens: &[TextToken<'_>], word_index: usize) -> Option<usize> {
    let sep_index = word_index + 1;
    let next_index = word_index + 2;
    if !tokens.get(word_index).is_some_and(|token| token.is_word) {
        return None;
    }
    if tokens
        .get(sep_index)
        .is_some_and(|token| !token.is_word && token.text.chars().all(char::is_whitespace))
        && tokens.get(next_index).is_some_and(|token| token.is_word)
    {
        Some(next_index)
    } else {
        None
    }
}

fn is_identifier_phrase_start_token(token: &str) -> bool {
    if matches!(token, "A" | "I") {
        return false;
    }
    is_identifier_code_token(token)
}

fn is_identifier_code_token(token: &str) -> bool {
    let mut has_alpha = false;
    for ch in token.chars() {
        if ch.is_ascii_alphabetic() {
            if !ch.is_ascii_uppercase() {
                return false;
            }
            has_alpha = true;
        } else if !ch.is_ascii_digit() {
            return false;
        }
    }
    has_alpha
}

#[derive(Debug, Clone, Copy)]
struct TextToken<'a> {
    text: &'a str,
    is_word: bool,
}

fn tokenize(source: &str) -> impl Iterator<Item = TextToken<'_>> {
    struct Tokens<'a> {
        source: &'a str,
        chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    }

    impl<'a> Iterator for Tokens<'a> {
        type Item = TextToken<'a>;

        fn next(&mut self) -> Option<Self::Item> {
            let (start, first) = self.chars.next()?;
            let is_word = first.is_alphanumeric();
            let end = loop {
                match self.chars.peek().copied() {
                    Some((_pos, ch)) if token_char_matches(ch, is_word) => {
                        self.chars.next();
                        if self.chars.peek().is_none() {
                            break self.source.len();
                        }
                    }
                    Some((pos, _)) => break pos,
                    None => break self.source.len(),
                }
            };
            Some(TextToken {
                text: &self.source[start..end],
                is_word,
            })
        }
    }

    fn token_char_matches(ch: char, is_word: bool) -> bool {
        if is_word {
            ch.is_alphanumeric() || ch == '-' || ch == '\''
        } else {
            !ch.is_alphanumeric()
        }
    }

    Tokens {
        source,
        chars: source.char_indices().peekable(),
    }
}

fn number_candidates(tokens: &[TextToken<'_>], start: usize) -> Option<Vec<(usize, Vec<String>)>> {
    let mut out = Vec::new();
    let mut words = Vec::new();
    let mut word_index = start;
    loop {
        if !tokens.get(word_index).is_some_and(|t| t.is_word) {
            break;
        }
        let parts = number_token_parts(tokens[word_index].text)?;
        words.extend(parts);
        out.push((word_index, words.clone()));

        let sep_index = word_index + 1;
        let next_word_index = word_index + 2;
        let Some(sep) = tokens.get(sep_index) else {
            break;
        };
        if sep.is_word || !sep.text.chars().all(char::is_whitespace) {
            break;
        }
        let Some(next_word) = tokens.get(next_word_index) else {
            break;
        };
        if !next_word.is_word || number_token_parts(next_word.text).is_none() {
            break;
        }
        word_index = next_word_index;
    }
    (!out.is_empty()).then_some(out)
}

fn number_token_parts(token: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = token
        .split('-')
        .map(|part| part.to_ascii_lowercase())
        .collect();
    if parts.is_empty() || parts.iter().any(|part| !is_number_part(part)) {
        return None;
    }
    Some(parts)
}

fn is_number_part(word: &str) -> bool {
    cardinal_unit_value(word).is_some()
        || ordinal_unit_value(word).is_some()
        || cardinal_teen_value(word).is_some()
        || ordinal_teen_value(word).is_some()
        || cardinal_ten_value(word).is_some()
        || ordinal_ten_value(word).is_some()
        || matches!(
            word,
            "zero"
                | "o"
                | "nought"
                | "hundred"
                | "thousand"
                | "million"
                | "billion"
                | "and"
                | "point"
        )
}

fn parse_number_words(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    if let Some(digits) = parse_digit_word_sequence(words) {
        return Some(digits);
    }
    if let Some(ordinal) = parse_ordinal_words(words) {
        return Some(ordinal);
    }
    if words.iter().filter(|word| word.as_str() == "point").count() > 1 {
        return None;
    }
    if let Some(point) = words.iter().position(|word| word == "point") {
        if point == 0 || point + 1 == words.len() {
            return None;
        }
        let int = parse_cardinal(&words[..point])?;
        let mut decimals = String::new();
        for word in &words[point + 1..] {
            decimals.push(char::from(b'0' + decimal_digit(word)?));
        }
        return Some(format!("{int}.{decimals}"));
    }
    let cardinal = parse_cardinal(words)?;
    if scale_only_magnitude(words) {
        return None;
    }
    if words.len() > 1
        || cardinal
            .parse::<u128>()
            .ok()
            .is_some_and(|value| value >= 10)
    {
        Some(cardinal)
    } else {
        None
    }
}

fn parse_ordinal_words(words: &[String]) -> Option<String> {
    let (last, prefix) = words.split_last()?;
    let (ordinal_base, final_kind) = ordinal_word_value(last)?;
    let value = if prefix.is_empty() {
        if matches!(final_kind, OrdinalKind::Unit) {
            return None;
        }
        ordinal_base
    } else {
        if prefix.iter().any(|word| word == "point") {
            return None;
        }
        let prefix_value = parse_cardinal_value(prefix)?;
        match final_kind {
            OrdinalKind::Unit if prefix_value >= 20 && prefix_value % 10 == 0 => {
                prefix_value + ordinal_base
            }
            OrdinalKind::Unit if prefix_value >= 100 && prefix_value % 100 == 0 => {
                prefix_value + ordinal_base
            }
            OrdinalKind::TeenOrTen if prefix_value >= 100 && prefix_value % 100 == 0 => {
                prefix_value + ordinal_base
            }
            _ => return None,
        }
    };
    Some(format!("{value}{}", ordinal_suffix(value)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrdinalKind {
    Unit,
    TeenOrTen,
}

fn ordinal_word_value(word: &str) -> Option<(u128, OrdinalKind)> {
    ordinal_unit_value(word)
        .map(|value| (value, OrdinalKind::Unit))
        .or_else(|| ordinal_teen_value(word).map(|value| (value, OrdinalKind::TeenOrTen)))
        .or_else(|| ordinal_ten_value(word).map(|value| (value, OrdinalKind::TeenOrTen)))
}

fn ordinal_suffix(value: u128) -> &'static str {
    match value % 100 {
        11..=13 => "th",
        _ => match value % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    }
}

fn scale_only_magnitude(words: &[String]) -> bool {
    match words {
        [only] => matches!(
            only.as_str(),
            "hundred" | "thousand" | "million" | "billion"
        ),
        _ => words
            .last()
            .is_some_and(|word| matches!(word.as_str(), "thousand" | "million" | "billion")),
    }
}

fn parse_cardinal(words: &[String]) -> Option<String> {
    Some(parse_cardinal_value(words)?.to_string())
}

fn parse_cardinal_value(words: &[String]) -> Option<u128> {
    if let Some(digits) = parse_leading_zero_sequence(words) {
        return digits.parse().ok();
    }

    let mut total: u128 = 0;
    let mut group: u128 = 0;
    let mut saw = false;

    for (idx, word) in words.iter().enumerate() {
        let word = word.as_str();
        if word == "and" {
            let next = words.get(idx + 1).map(String::as_str);
            if !saw || next.is_none() || group < 100 || group % 100 != 0 {
                return None;
            }
            continue;
        }
        if matches!(word, "zero" | "o" | "nought") {
            return None;
        }
        if let Some(value) = cardinal_unit_value(word).or_else(|| cardinal_teen_value(word)) {
            if group == 0 {
                group = value;
            } else if (group >= 20 && group % 10 == 0 && value < 10)
                || (group >= 100 && group % 100 == 0 && value < 100)
            {
                group += value;
            } else {
                return None;
            }
            saw = true;
            continue;
        }
        if let Some(value) = cardinal_ten_value(word) {
            if group == 0 || (group >= 100 && group % 100 == 0) {
                group += value;
            } else {
                return None;
            }
            saw = true;
            continue;
        }
        if word == "hundred" {
            if group == 0 {
                group = 100;
            } else if group < 100 {
                group *= 100;
            } else {
                return None;
            }
            saw = true;
            continue;
        }
        if let Some(scale) = scale_value(word) {
            if group == 0 {
                group = 1;
            }
            total = total.checked_add(group.checked_mul(scale)?)?;
            group = 0;
            saw = true;
            continue;
        }
        return None;
    }

    if !saw {
        return None;
    }
    Some(total + group)
}

fn parse_leading_zero_sequence(words: &[String]) -> Option<String> {
    if words.len() < 2 || !matches!(words.first()?.as_str(), "zero" | "o" | "nought") {
        return None;
    }
    let mut digits = String::with_capacity(words.len());
    for word in words {
        digits.push(char::from(b'0' + decimal_digit(word)?));
    }
    Some(digits)
}

fn parse_digit_word_sequence(words: &[String]) -> Option<String> {
    if words.len() < 2 {
        return None;
    }
    let digits: Option<Vec<u8>> = words.iter().map(|word| decimal_digit(word)).collect();
    let digits = digits?;
    let mut out = String::with_capacity(digits.len());
    for digit in digits {
        out.push(char::from(b'0' + digit));
    }
    Some(out)
}

fn decimal_digit(word: &str) -> Option<u8> {
    match word {
        "zero" | "o" | "nought" => Some(0),
        _ => cardinal_unit_value(word).and_then(|value| u8::try_from(value).ok()),
    }
}

fn cardinal_unit_value(word: &str) -> Option<u128> {
    match word {
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        _ => None,
    }
}

fn ordinal_unit_value(word: &str) -> Option<u128> {
    match word {
        "first" | "oneth" => Some(1),
        "second" => Some(2),
        "third" => Some(3),
        "fourth" => Some(4),
        "fifth" => Some(5),
        "sixth" => Some(6),
        "seventh" => Some(7),
        "eighth" => Some(8),
        "ninth" => Some(9),
        _ => None,
    }
}

fn cardinal_teen_value(word: &str) -> Option<u128> {
    match word {
        "ten" => Some(10),
        "eleven" => Some(11),
        "twelve" => Some(12),
        "thirteen" => Some(13),
        "fourteen" => Some(14),
        "fifteen" => Some(15),
        "sixteen" => Some(16),
        "seventeen" => Some(17),
        "eighteen" => Some(18),
        "nineteen" => Some(19),
        _ => None,
    }
}

fn ordinal_teen_value(word: &str) -> Option<u128> {
    match word {
        "tenth" => Some(10),
        "eleventh" => Some(11),
        "twelfth" => Some(12),
        "thirteenth" => Some(13),
        "fourteenth" => Some(14),
        "fifteenth" => Some(15),
        "sixteenth" => Some(16),
        "seventeenth" => Some(17),
        "eighteenth" => Some(18),
        "nineteenth" => Some(19),
        _ => None,
    }
}

fn cardinal_ten_value(word: &str) -> Option<u128> {
    match word {
        "twenty" => Some(20),
        "thirty" => Some(30),
        "fourty" | "forty" => Some(40),
        "fifty" => Some(50),
        "sixty" => Some(60),
        "seventy" => Some(70),
        "eighty" => Some(80),
        "ninety" => Some(90),
        _ => None,
    }
}

fn ordinal_ten_value(word: &str) -> Option<u128> {
    match word {
        "twentieth" => Some(20),
        "thirtieth" => Some(30),
        "fourtieth" | "fortieth" => Some(40),
        "fiftieth" => Some(50),
        "sixtieth" | "sixteeth" => Some(60),
        "seventieth" => Some(70),
        "eightieth" => Some(80),
        "ninetieth" => Some(90),
        _ => None,
    }
}

fn scale_value(word: &str) -> Option<u128> {
    match word {
        "thousand" | "thousandth" => Some(1_000),
        "million" | "millionth" => Some(1_000_000),
        "billion" | "billionth" => Some(1_000_000_000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayTextOptions, PasteTextOptions, build_display_text, build_paste_text,
        collapse_consecutive_duplicates, generated::emoji_phrases::EMOJI_PHRASE_COUNT,
    };

    fn options<'a>(
        append_trailing_space: bool,
        removed_words: &'a [String],
        deduplicate_words: bool,
        convert_number_words: bool,
    ) -> PasteTextOptions<'a> {
        PasteTextOptions {
            append_trailing_space,
            removed_words,
            deduplicate_words,
            convert_number_words,
            convert_spoken_emoji: false,
            lowercase_except_uppercase_words: false,
        }
    }

    fn lowercase_options<'a>(
        append_trailing_space: bool,
        removed_words: &'a [String],
        deduplicate_words: bool,
        convert_number_words: bool,
    ) -> PasteTextOptions<'a> {
        PasteTextOptions {
            append_trailing_space,
            removed_words,
            deduplicate_words,
            convert_number_words,
            convert_spoken_emoji: false,
            lowercase_except_uppercase_words: true,
        }
    }

    fn emoji_options<'a>(
        append_trailing_space: bool,
        removed_words: &'a [String],
        deduplicate_words: bool,
        convert_number_words: bool,
    ) -> PasteTextOptions<'a> {
        PasteTextOptions {
            append_trailing_space,
            removed_words,
            deduplicate_words,
            convert_number_words,
            convert_spoken_emoji: true,
            lowercase_except_uppercase_words: false,
        }
    }

    fn display_options<'a>(
        removed_words: &'a [String],
        deduplicate_words: bool,
        convert_number_words: bool,
        lowercase_except_uppercase_words: bool,
    ) -> DisplayTextOptions<'a> {
        DisplayTextOptions {
            removed_words,
            deduplicate_words,
            convert_number_words,
            convert_spoken_emoji: false,
            lowercase_except_uppercase_words,
        }
    }

    fn emoji_display_options<'a>(
        removed_words: &'a [String],
        deduplicate_words: bool,
        convert_number_words: bool,
        lowercase_except_uppercase_words: bool,
    ) -> DisplayTextOptions<'a> {
        DisplayTextOptions {
            removed_words,
            deduplicate_words,
            convert_number_words,
            convert_spoken_emoji: true,
            lowercase_except_uppercase_words,
        }
    }

    #[test]
    fn build_paste_text_appends_trailing_space_when_enabled() {
        assert_eq!(
            build_paste_text("hello", options(true, &[], true, false)),
            "hello "
        );
        assert_eq!(
            build_paste_text("hello ", options(true, &[], true, false)),
            "hello "
        );
    }

    #[test]
    fn build_display_text_applies_transforms_without_trailing_space() {
        let words = vec!["um".to_string()];
        assert_eq!(
            build_display_text(
                "Um The the API has Twenty One tabs",
                display_options(&words, true, true, true)
            ),
            "the API has 21 tabs"
        );
    }

    #[test]
    fn build_display_text_preserves_untransformed_trailing_whitespace() {
        assert_eq!(
            build_display_text("hello ", display_options(&[], true, false, false)),
            "hello "
        );
    }

    #[test]
    fn build_paste_text_preserves_input_when_trailing_space_is_disabled() {
        assert_eq!(
            build_paste_text("hello", options(false, &[], true, false)),
            "hello"
        );
        assert_eq!(
            build_paste_text("hello ", options(false, &[], true, false)),
            "hello "
        );
    }

    #[test]
    fn build_paste_text_strips_single_word_terminal_period() {
        assert_eq!(
            build_paste_text("Hello.", options(false, &[], true, false)),
            "Hello"
        );
        assert_eq!(
            build_paste_text("Hello. ", options(false, &[], true, false)),
            "Hello "
        );
    }

    #[test]
    fn build_paste_text_strips_single_word_period_before_appending_space() {
        assert_eq!(
            build_paste_text("Hello.", options(true, &[], true, false)),
            "Hello "
        );
    }

    #[test]
    fn build_paste_text_strips_terminal_period_after_a_lone_emoji() {
        // "happy emoji" -> "😊"; a single emoji output must not carry a period.
        assert_eq!(
            build_paste_text("happy emoji.", emoji_options(false, &[], false, false)),
            "😊"
        );
        assert_eq!(
            build_paste_text("happy emoji.", emoji_options(true, &[], false, false)),
            "😊 "
        );
        // New aliases resolve, and a lone one drops the period too.
        assert_eq!(
            build_paste_text("checkbox emoji.", emoji_options(false, &[], false, false)),
            "✅"
        );
        assert_eq!(
            build_paste_text("red dot emoji.", emoji_options(false, &[], false, false)),
            "🔴"
        );
        // A period after real words still stays (two words, not a lone unit).
        assert_eq!(
            build_paste_text("ship it.", emoji_options(false, &[], false, false)),
            "ship it."
        );
    }

    #[test]
    fn build_paste_text_strips_period_after_single_identifier_transform() {
        assert_eq!(
            build_paste_text("S eight.", options(false, &[], true, true)),
            "S8"
        );
    }

    #[test]
    fn build_paste_text_keeps_terminal_period_for_multi_word_text() {
        assert_eq!(
            build_paste_text("Hello world.", options(false, &[], true, false)),
            "Hello world."
        );
    }

    #[test]
    fn build_paste_text_keeps_period_for_multi_token_abbreviation() {
        assert_eq!(
            build_paste_text("U.S.", options(false, &[], true, false)),
            "U.S."
        );
    }

    #[test]
    fn build_display_text_keeps_single_word_terminal_period() {
        assert_eq!(
            build_display_text("Hello.", display_options(&[], true, false, false)),
            "Hello."
        );
    }

    #[test]
    fn build_paste_text_strips_removed_words() {
        let words = vec!["um".to_string(), "ah".to_string()];
        assert_eq!(
            build_paste_text(
                "um I think ah this is right um",
                options(false, &words, true, false)
            ),
            "I think this is right"
        );
        assert_eq!(
            build_paste_text("Um hello Ah world", options(false, &words, true, false)),
            "hello world"
        );
    }

    #[test]
    fn build_paste_text_strips_removed_word_at_boundaries() {
        let words = vec!["um".to_string()];
        assert_eq!(
            build_paste_text("um", options(false, &words, true, false)),
            ""
        );
        assert_eq!(
            build_paste_text("um hello", options(false, &words, true, false)),
            "hello"
        );
        assert_eq!(
            build_paste_text("hello um", options(false, &words, true, false)),
            "hello"
        );
        assert_eq!(
            build_paste_text("yummy", options(false, &words, true, false)),
            "yummy"
        );
    }

    #[test]
    fn build_paste_text_strips_removed_words_with_punctuation() {
        let words = vec!["um".to_string(), "ah".to_string()];
        assert_eq!(
            build_paste_text(
                "Um, I think this is right.",
                options(false, &words, true, false)
            ),
            "I think this is right."
        );
        assert_eq!(
            build_paste_text("Ah. Hello world.", options(false, &words, true, false)),
            "Hello world."
        );
        assert_eq!(
            build_paste_text("um, ah, hello", options(false, &words, true, false)),
            "hello"
        );
    }

    #[test]
    fn build_paste_text_lowercases_without_removing_punctuation() {
        assert_eq!(
            build_paste_text(
                "Hello, World! This.Is A Test?",
                lowercase_options(false, &[], true, false)
            ),
            "hello, world! this.is a test?"
        );
    }

    #[test]
    fn build_paste_text_preserves_uppercase_words() {
        assert_eq!(
            build_paste_text(
                "I saw NASA launch an API-based HTTP2 test in the USA.",
                lowercase_options(false, &[], true, false)
            ),
            "i saw NASA launch an API-based HTTP2 test in the USA."
        );
    }

    #[test]
    fn build_paste_text_preserves_plural_uppercase_words() {
        assert_eq!(
            build_paste_text(
                "The APIs and OSes connect to CPUs, SDKs, and HTTP2 endpoints.",
                lowercase_options(false, &[], true, false)
            ),
            "the APIs and OSes connect to CPUs, SDKs, and HTTP2 endpoints."
        );
    }

    #[test]
    fn build_paste_text_preserves_plural_uppercase_words_with_possessives() {
        assert_eq!(
            build_paste_text(
                "The APIs' limits and API's docs mention SMSes.",
                lowercase_options(false, &[], true, false)
            ),
            "the APIs' limits and API's docs mention SMSes."
        );
    }

    #[test]
    fn build_paste_text_lowercases_non_plural_uppercase_suffixes() {
        assert_eq!(
            build_paste_text(
                "The APIing demo and URLish example are weird.",
                lowercase_options(false, &[], true, false)
            ),
            "the apiing demo and urlish example are weird."
        );
    }

    #[test]
    fn build_paste_text_can_lowercase_after_number_conversion() {
        assert_eq!(
            build_paste_text(
                "Ship Twenty One API calls.",
                lowercase_options(false, &[], true, true)
            ),
            "ship 21 API calls."
        );
    }

    #[test]
    fn build_paste_text_preserves_generated_identifiers_when_lowercasing() {
        assert_eq!(
            build_paste_text(
                "Use S eight and W three C with HTTP two.",
                lowercase_options(false, &[], true, true)
            ),
            "use S8 and W3C with HTTP2."
        );
    }

    #[test]
    fn generated_emoji_phrase_map_has_expected_coverage() {
        assert!(EMOJI_PHRASE_COUNT >= 3_800);
    }

    #[test]
    fn build_paste_text_converts_explicit_spoken_emoji_directives() {
        assert_eq!(
            build_paste_text(
                "Ship it happy emoji and thumbs up emoji.",
                emoji_options(false, &[], true, false)
            ),
            "Ship it 😊 and 👍."
        );
        assert_eq!(
            build_paste_text(
                "That was face with tears of joy emoji",
                emoji_options(false, &[], true, false)
            ),
            "That was 😂"
        );
    }

    #[test]
    fn build_paste_text_converts_curated_emotion_emoji_aliases() {
        assert_eq!(
            build_paste_text(
                "sad emoji angry emoji anxious emoji shocked emoji love emoji bored emoji meh emoji",
                emoji_options(false, &[], true, false)
            ),
            "😢 😠 😟 😱 ❤️ 🥱 😐"
        );
    }

    #[test]
    fn build_paste_text_converts_new_block_pause_and_common_aliases() {
        // {color} block mirrors {color} dot; single-word "pause"; and new common single-word aliases.
        assert_eq!(
            build_paste_text(
                "red block emoji green block emoji pause emoji joy emoji money emoji cake emoji",
                emoji_options(false, &[], true, false)
            ),
            "🟥 🟩 ⏸️ 😂 💰 🎂"
        );
    }

    #[test]
    fn build_paste_text_converts_plural_emoji_trigger() {
        assert_eq!(
            build_paste_text(
                "Approved check mark emojis",
                emoji_options(false, &[], true, false)
            ),
            "Approved ✅"
        );
    }

    #[test]
    fn build_paste_text_preserves_plain_words_without_emoji_trigger() {
        assert_eq!(
            build_paste_text(
                "I am happy about the fire drill",
                emoji_options(false, &[], true, false)
            ),
            "I am happy about the fire drill"
        );
    }

    #[test]
    fn build_paste_text_leaves_unknown_emoji_directives_unchanged() {
        assert_eq!(
            build_paste_text(
                "Use impossible widget emoji here",
                emoji_options(false, &[], true, false)
            ),
            "Use impossible widget emoji here"
        );
    }

    #[test]
    fn build_display_text_can_apply_emoji_with_other_transforms() {
        let words = vec!["um".to_string()];
        assert_eq!(
            build_display_text(
                "Um Ship Twenty One happy emoji API tests",
                emoji_display_options(&words, true, true, true)
            ),
            "ship 21 😊 API tests"
        );
    }

    #[test]
    fn collapse_dup_basic_two_in_a_row() {
        assert_eq!(collapse_consecutive_duplicates("the the cat"), "the cat");
    }

    #[test]
    fn collapse_dup_three_or_more_in_a_row() {
        assert_eq!(
            collapse_consecutive_duplicates("that that that idea"),
            "that idea"
        );
        assert_eq!(
            collapse_consecutive_duplicates("uh uh uh uh hello"),
            "uh hello"
        );
    }

    #[test]
    fn collapse_dup_period_acts_as_barrier() {
        assert_eq!(
            collapse_consecutive_duplicates("the. the cat"),
            "the. the cat"
        );
        assert_eq!(
            collapse_consecutive_duplicates("end. End of sentence."),
            "end. End of sentence."
        );
    }

    #[test]
    fn collapse_dup_comma_acts_as_barrier_for_letter_spelling() {
        assert_eq!(
            collapse_consecutive_duplicates("S, P, E, N, C, E, R"),
            "S, P, E, N, C, E, R"
        );
    }

    #[test]
    fn collapse_dup_single_letter_no_collapse() {
        assert_eq!(collapse_consecutive_duplicates("M M alpha"), "M M alpha");
        assert_eq!(collapse_consecutive_duplicates("A A B B"), "A A B B");
    }

    #[test]
    fn collapse_dup_digits_no_collapse() {
        assert_eq!(collapse_consecutive_duplicates("2288 2288"), "2288 2288");
        assert_eq!(
            collapse_consecutive_duplicates("2288. Eight, eight."),
            "2288. Eight, eight."
        );
    }

    #[test]
    fn collapse_dup_preserves_trailing_punct_on_survivor() {
        assert_eq!(collapse_consecutive_duplicates("the the. cat"), "the. cat");
        assert_eq!(collapse_consecutive_duplicates("uh uh, hello"), "uh, hello");
    }

    #[test]
    fn collapse_dup_case_insensitive() {
        assert_eq!(collapse_consecutive_duplicates("The the cat"), "the cat");
        assert_eq!(collapse_consecutive_duplicates("the The cat"), "The cat");
    }

    #[test]
    fn collapse_dup_known_false_positive_unpunctuated_number_words() {
        assert_eq!(
            collapse_consecutive_duplicates("two two eight eight"),
            "two eight"
        );
    }

    #[test]
    fn build_paste_text_runs_filler_then_dedup_in_order() {
        let words = vec!["um".to_string()];
        assert_eq!(
            build_paste_text("um the the cat", options(false, &words, true, false)),
            "the cat"
        );
    }

    #[test]
    fn build_paste_text_can_preserve_duplicate_words() {
        let words = vec!["um".to_string()];
        assert_eq!(
            build_paste_text("um no no", options(false, &words, false, false)),
            "no no"
        );
    }

    #[test]
    fn number_words_convert_basic_cardinals() {
        assert_eq!(
            build_paste_text(
                "twenty-five cows, twelve chickens and one hundred twenty five kg",
                options(false, &[], true, true),
            ),
            "25 cows, 12 chickens and 125 kg"
        );
    }

    #[test]
    fn number_words_convert_large_cardinals() {
        assert_eq!(
            build_paste_text(
                "one thousand two hundred and sixty six dollars",
                options(false, &[], true, true),
            ),
            "1266 dollars"
        );
        assert_eq!(
            build_paste_text(
                "fifty-three billion two hundred forty-three thousand seven hundred twenty-four",
                options(false, &[], true, true),
            ),
            "53000243724"
        );
    }

    #[test]
    fn number_words_convert_decimals() {
        assert_eq!(
            build_paste_text("twelve point nine nine", options(false, &[], true, true)),
            "12.99"
        );
        assert_eq!(
            build_paste_text(
                "one point two hundred thirty-six",
                options(false, &[], true, true)
            ),
            "1.2 136"
        );
    }

    #[test]
    fn number_words_convert_digit_sequences_without_dedup_loss() {
        assert_eq!(
            build_paste_text("two two eight eight", options(false, &[], true, true)),
            "2288"
        );
        assert_eq!(
            build_paste_text("zero zero five", options(false, &[], true, true)),
            "005"
        );
    }

    #[test]
    fn number_words_convert_digit_sequences_but_not_isolated_low_words() {
        assert_eq!(
            build_paste_text(
                "Please call me at one two three four five six seven eight nine zero",
                options(false, &[], true, true),
            ),
            "Please call me at 1234567890"
        );
        assert_eq!(
            build_paste_text(
                "This is the one I was waiting for",
                options(false, &[], true, true),
            ),
            "This is the one I was waiting for"
        );
        assert_eq!(
            build_paste_text("I have fifteen tabs", options(false, &[], true, true)),
            "I have 15 tabs"
        );
    }

    #[test]
    fn number_words_convert_spoken_section_identifiers() {
        assert_eq!(
            build_paste_text(
                "Open section S eight, R four, and V twelve.",
                options(false, &[], true, true),
            ),
            "Open section S8, R4, and V12."
        );
    }

    #[test]
    fn number_words_convert_spoken_acronym_identifiers() {
        assert_eq!(
            build_paste_text(
                "Check W three C, WS two, HTTP two, and API twenty one.",
                options(false, &[], true, true),
            ),
            "Check W3C, WS2, HTTP2, and API21."
        );
    }

    #[test]
    fn number_words_identifier_conversion_requires_uppercase_identifier_signal() {
        assert_eq!(
            build_paste_text(
                "we three C should stay prose",
                options(false, &[], true, true)
            ),
            "we three C should stay prose"
        );
        assert_eq!(
            build_paste_text(
                "A one time thing and I two guess",
                options(false, &[], true, true)
            ),
            "A one time thing and I two guess"
        );
    }

    #[test]
    fn number_words_match_agreed_replacement_table() {
        for (spoken, expected) in [
            ("three", "three"),
            ("four", "four"),
            ("twelve", "12"),
            ("twenty", "20"),
            ("twenty five", "25"),
            ("one hundred twenty five", "125"),
            ("one thousand two hundred sixty six", "1266"),
            ("twelve point nine nine", "12.99"),
            ("one two three four", "1234"),
            ("zero zero five", "005"),
            ("two two eight eight", "2288"),
            ("a hundred", "a hundred"),
            ("a thousand", "a thousand"),
            ("a million", "a million"),
            ("two million", "two million"),
            ("one billion", "one billion"),
            ("billions", "billions"),
            ("millions of rows", "millions of rows"),
            ("hundreds of users", "hundreds of users"),
            ("one and done", "one and done"),
            ("one hundred and three", "103"),
            ("twenty first", "21st"),
            ("first", "first"),
            ("second", "second"),
            ("third", "third"),
            ("S eight", "S8"),
            ("R four", "R4"),
            ("V twelve", "V12"),
            ("W three C", "W3C"),
            ("WS two", "WS2"),
            ("number three", "#3"),
            ("number twelve", "#12"),
            ("number twenty five", "#25"),
            ("number 3", "#3"),
        ] {
            assert_eq!(
                build_paste_text(spoken, options(false, &[], true, true)),
                expected,
                "spoken phrase: {spoken:?}"
            );
        }
    }

    #[test]
    fn number_words_conversion_is_optional() {
        assert_eq!(
            build_paste_text("twenty five", options(false, &[], true, false)),
            "twenty five"
        );
    }

    #[test]
    fn number_words_attach_percent_sign() {
        // digits already in the text
        assert_eq!(
            build_paste_text(
                "we grew 12 percent last quarter",
                options(false, &[], true, true)
            ),
            "we grew 12% last quarter"
        );
        // spoken number -> digits -> percent sign, end to end
        assert_eq!(
            build_paste_text(
                "raise it by twelve percent",
                options(false, &[], true, true)
            ),
            "raise it by 12%"
        );
        // decimals and grouped numbers keep their separators; the last digit run carries the sign
        assert_eq!(
            build_paste_text("about 3.5 percent", options(false, &[], true, true)),
            "about 3.5%"
        );
        assert_eq!(
            build_paste_text("up 1,000 percent", options(false, &[], true, true)),
            "up 1,000%"
        );
        // trailing punctuation is preserved
        assert_eq!(
            build_paste_text("up 100 percent.", options(false, &[], true, true)),
            "up 100%."
        );
        // capitalized word still matches
        assert_eq!(
            build_paste_text("50 Percent done", options(false, &[], true, true)),
            "50% done"
        );
    }

    #[test]
    fn number_words_percent_leaves_non_matches_alone() {
        // "percentage" is a different word
        assert_eq!(
            build_paste_text("12 percentage points", options(false, &[], true, true)),
            "12 percentage points"
        );
        // no preceding number
        assert_eq!(
            build_paste_text("a fair percent of them", options(false, &[], true, true)),
            "a fair percent of them"
        );
    }

    #[test]
    fn number_words_percent_requires_the_setting() {
        // gated by the number-words setting: off -> leave "12 percent" untouched
        assert_eq!(
            build_paste_text("12 percent", options(false, &[], true, false)),
            "12 percent"
        );
    }

    #[test]
    fn number_words_convert_spoken_hash_numbers() {
        assert_eq!(
            build_paste_text("see number three", options(false, &[], true, true)),
            "see #3"
        );
        assert_eq!(
            build_paste_text(
                "Number twenty five is fixed",
                options(false, &[], true, true)
            ),
            "#25 is fixed"
        );
        assert_eq!(
            build_paste_text(
                "closed number one two three and number 7.",
                options(false, &[], true, true)
            ),
            "closed #123 and #7."
        );
        // multi-word cardinals still land as a single #N
        assert_eq!(
            build_paste_text("track number one hundred", options(false, &[], true, true)),
            "track #100"
        );
        assert_eq!(
            build_paste_text(
                "see number one hundred and three",
                options(false, &[], true, true)
            ),
            "see #103"
        );
    }

    #[test]
    fn number_words_hash_leaves_non_matches_alone() {
        // bare "number" with no following quantity
        assert_eq!(
            build_paste_text("a number of bugs", options(false, &[], true, true)),
            "a number of bugs"
        );
        // ordinals / decimals are not issue-style numbers
        assert_eq!(
            build_paste_text("number first", options(false, &[], true, true)),
            "number first"
        );
        assert_eq!(
            build_paste_text("number twenty first", options(false, &[], true, true)),
            "number 21st"
        );
        assert_eq!(
            build_paste_text("number three point five", options(false, &[], true, true)),
            "number 3.5"
        );
        // isolated low number-words still stay prose without the "number" signal
        assert_eq!(
            build_paste_text("three bugs", options(false, &[], true, true)),
            "three bugs"
        );
    }

    #[test]
    fn number_words_hash_requires_the_setting() {
        assert_eq!(
            build_paste_text("number three", options(false, &[], true, false)),
            "number three"
        );
    }
}
