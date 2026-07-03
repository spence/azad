// English number-word parsing is adapted from allo-media/text2num-rs' English
// language rules, licensed MIT:
// https://github.com/allo-media/text2num-rs/blob/master/src/lang/en/mod.rs
// Copyright (c) 2021-2024 Groupe Allo-Media.

pub struct PasteTextOptions<'a> {
    pub append_trailing_space: bool,
    pub removed_words: &'a [String],
    pub deduplicate_words: bool,
    pub convert_number_words: bool,
}

pub fn build_paste_text(text: &str, options: PasteTextOptions<'_>) -> String {
    let mut paste_text = if options.removed_words.is_empty() {
        text.to_string()
    } else {
        strip_removed_words(text, options.removed_words)
    };
    if options.convert_number_words {
        paste_text = replace_english_number_words(&paste_text);
    }
    if options.deduplicate_words {
        paste_text = collapse_consecutive_duplicates(&paste_text);
    }
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
        .map(|part| lemmatize(part).to_ascii_lowercase())
        .collect();
    if parts.is_empty() || parts.iter().any(|part| !is_number_part(part)) {
        return None;
    }
    Some(parts)
}

fn lemmatize(word: &str) -> &str {
    if word.ends_with('s') && word != "seconds" {
        word.trim_end_matches('s')
    } else {
        word
    }
}

fn is_number_part(word: &str) -> bool {
    unit_value(word).is_some()
        || teen_value(word).is_some()
        || ten_value(word).is_some()
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

fn parse_cardinal(words: &[String]) -> Option<String> {
    if let Some(digits) = parse_leading_zero_sequence(words) {
        return Some(digits);
    }

    let mut total: u128 = 0;
    let mut group: u128 = 0;
    let mut saw = false;

    for word in words {
        let word = word.as_str();
        if word == "and" {
            if !saw {
                return None;
            }
            continue;
        }
        if matches!(word, "zero" | "o" | "nought") {
            return None;
        }
        if let Some(value) = unit_value(word).or_else(|| teen_value(word)) {
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
        if let Some(value) = ten_value(word) {
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
    Some((total + group).to_string())
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
    if matches!(words.first()?.as_str(), "zero" | "o" | "nought") {
        let mut out = String::with_capacity(digits.len());
        for digit in digits {
            out.push(char::from(b'0' + digit));
        }
        return Some(out);
    }
    Some(
        digits
            .into_iter()
            .map(|digit| char::from(b'0' + digit).to_string())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn decimal_digit(word: &str) -> Option<u8> {
    match word {
        "zero" | "o" | "nought" => Some(0),
        _ => unit_value(word).and_then(|value| u8::try_from(value).ok()),
    }
}

fn unit_value(word: &str) -> Option<u128> {
    match word {
        "one" | "first" | "oneth" => Some(1),
        "two" | "second" => Some(2),
        "three" | "third" => Some(3),
        "four" | "fourth" => Some(4),
        "five" | "fifth" => Some(5),
        "six" | "sixth" => Some(6),
        "seven" | "seventh" => Some(7),
        "eight" | "eighth" => Some(8),
        "nine" | "ninth" => Some(9),
        _ => None,
    }
}

fn teen_value(word: &str) -> Option<u128> {
    match word {
        "ten" | "tenth" => Some(10),
        "eleven" | "eleventh" => Some(11),
        "twelve" | "twelfth" => Some(12),
        "thirteen" | "thirteenth" => Some(13),
        "fourteen" | "fourteenth" => Some(14),
        "fifteen" | "fifteenth" => Some(15),
        "sixteen" | "sixteenth" => Some(16),
        "seventeen" | "seventeenth" => Some(17),
        "eighteen" | "eighteenth" => Some(18),
        "nineteen" | "nineteenth" => Some(19),
        _ => None,
    }
}

fn ten_value(word: &str) -> Option<u128> {
    match word {
        "twenty" | "twentieth" => Some(20),
        "thirty" | "thirtieth" => Some(30),
        "fourty" | "forty" | "fourtieth" | "fortieth" => Some(40),
        "fifty" | "fiftieth" => Some(50),
        "sixty" | "sixtieth" | "sixteeth" => Some(60),
        "seventy" | "seventieth" => Some(70),
        "eighty" | "eightieth" => Some(80),
        "ninety" | "ninetieth" => Some(90),
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
    use super::{PasteTextOptions, build_paste_text, collapse_consecutive_duplicates};

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
            "2 2 8 8"
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
            "Please call me at 1 2 3 4 5 6 7 8 9 0"
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
    fn number_words_conversion_is_optional() {
        assert_eq!(
            build_paste_text("twenty five", options(false, &[], true, false)),
            "twenty five"
        );
    }
}
