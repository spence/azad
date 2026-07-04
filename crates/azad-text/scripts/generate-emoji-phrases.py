#!/usr/bin/env python3
import json
import re
import sys
import tomllib
import urllib.request
from collections import defaultdict
from pathlib import Path

UNICODE_EMOJI_VERSION = "17.0"
UNICODE_EMOJI_TEST_URL = "https://unicode.org/Public/17.0.0/emoji/emoji-test.txt"
CLDR_JSON_VERSION = "48.2.0"
CLDR_ANNOTATIONS_URL = (
    "https://raw.githubusercontent.com/unicode-org/cldr-json/48.2.0/"
    "cldr-json/cldr-annotations-full/annotations/en/annotations.json"
)
CLDR_DERIVED_ANNOTATIONS_URL = (
    "https://raw.githubusercontent.com/unicode-org/cldr-json/48.2.0/"
    "cldr-json/cldr-annotations-derived-full/annotationsDerived/en/annotations.json"
)

ROOT = Path(__file__).resolve().parents[1]
ALIASES_PATH = ROOT / "data" / "emoji_aliases.toml"
OUTPUT_PATH = ROOT / "src" / "generated" / "emoji_phrases.rs"


def fetch_text(url: str) -> str:
    with urllib.request.urlopen(url) as response:
        return response.read().decode("utf-8")


def fetch_json(url: str) -> dict:
    return json.loads(fetch_text(url))


def normalize_phrase(phrase: str) -> str:
    phrase = phrase.lower()
    phrase = phrase.replace("&", " and ")
    phrase = phrase.replace("+", " plus ")
    phrase = re.sub(r"[^a-z0-9]+", " ", phrase)
    return re.sub(r"\s+", " ", phrase).strip()


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def parse_emoji_test(text: str) -> dict[str, str]:
    emoji_names: dict[str, str] = {}
    line_re = re.compile(r"^([0-9A-F ]+)\s*;\s*([^#]+)\s*#\s*(\S+)\s+E[0-9.]+\s+(.+)$")
    for line in text.splitlines():
        match = line_re.match(line)
        if not match:
            continue
        codepoints, status, emoji, name = match.groups()
        if status.strip() != "fully-qualified":
            continue
        points = [int(part, 16) for part in codepoints.split()]
        if any(0x1F3FB <= point <= 0x1F3FF for point in points):
            continue
        emoji_names[emoji] = name
    return emoji_names


def load_cldr_annotations() -> dict[str, dict]:
    annotations: dict[str, dict] = {}
    full = fetch_json(CLDR_ANNOTATIONS_URL)["annotations"]["annotations"]
    derived = fetch_json(CLDR_DERIVED_ANNOTATIONS_URL)["annotationsDerived"]["annotations"]
    annotations.update(full)
    annotations.update(derived)
    return annotations


def add_unique_source(
    phrase_to_values: dict[str, set[str]],
    phrase: str,
    emoji: str,
) -> None:
    normalized = normalize_phrase(phrase)
    if normalized:
        phrase_to_values[f"{normalized} emoji"].add(emoji)


def unique_entries(phrase_to_values: dict[str, set[str]]) -> dict[str, str]:
    return {
        phrase: next(iter(values))
        for phrase, values in phrase_to_values.items()
        if len(values) == 1
    }


def load_aliases() -> dict[str, str]:
    aliases = tomllib.loads(ALIASES_PATH.read_text(encoding="utf-8")).get("aliases", {})
    out = {}
    for phrase, emoji in aliases.items():
        normalized = normalize_phrase(phrase)
        if not normalized.endswith(" emoji"):
            raise SystemExit(f"alias must end with 'emoji': {phrase}")
        out[normalized] = emoji
    return out


def build_map() -> tuple[dict[str, str], dict[str, int]]:
    emoji_names = parse_emoji_test(fetch_text(UNICODE_EMOJI_TEST_URL))
    annotations = load_cldr_annotations()
    allowed = set(emoji_names)

    canonical: dict[str, set[str]] = defaultdict(set)
    keywords: dict[str, set[str]] = defaultdict(set)

    for emoji, name in emoji_names.items():
        add_unique_source(canonical, name, emoji)
        annotation = annotations.get(emoji)
        if not annotation:
            continue
        for phrase in annotation.get("tts", []):
            add_unique_source(canonical, phrase, emoji)
        for phrase in annotation.get("default", []):
            add_unique_source(keywords, phrase, emoji)

    entries: dict[str, str] = {}
    for phrase, emoji in unique_entries(canonical).items():
        entries[phrase] = emoji

    keyword_conflicts = 0
    for phrase, emoji in unique_entries(keywords).items():
        existing = entries.get(phrase)
        if existing is not None and existing != emoji:
            keyword_conflicts += 1
            continue
        entries[phrase] = emoji

    aliases = load_aliases()
    unknown_aliases = sorted({emoji for emoji in aliases.values() if emoji not in allowed})
    if unknown_aliases:
        raise SystemExit(f"aliases reference emoji outside the generated set: {unknown_aliases}")
    entries.update(aliases)

    stats = {
        "emoji_count": len(emoji_names),
        "canonical_phrase_count": len(canonical),
        "canonical_collision_count": sum(1 for values in canonical.values() if len(values) > 1),
        "keyword_phrase_count": len(keywords),
        "unique_keyword_count": sum(1 for values in keywords.values() if len(values) == 1),
        "ambiguous_keyword_count": sum(1 for values in keywords.values() if len(values) > 1),
        "keyword_conflict_count": keyword_conflicts,
        "alias_count": len(aliases),
        "entry_count": len(entries),
        "max_phrase_words": max(len(phrase.split()) for phrase in entries),
    }
    return entries, stats


def write_generated(entries: dict[str, str], stats: dict[str, int]) -> None:
    OUTPUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        "// @generated by crates/azad-text/scripts/generate-emoji-phrases.py",
        "// Do not edit by hand.",
        "//",
        f"// Unicode Emoji data: {UNICODE_EMOJI_TEST_URL}",
        f"// Unicode Emoji version: {UNICODE_EMOJI_VERSION}",
        f"// CLDR JSON tag: {CLDR_JSON_VERSION}",
        f"// CLDR annotations: {CLDR_ANNOTATIONS_URL}",
        f"// CLDR derived annotations: {CLDR_DERIVED_ANNOTATIONS_URL}",
        "// Unicode data is available under the Unicode Terms of Use:",
        "// https://www.unicode.org/terms_of_use.html",
        "//",
        f"pub(crate) const MAX_EMOJI_PHRASE_WORDS: usize = {stats['max_phrase_words']};",
        f"#[cfg(test)]",
        f"pub(crate) const EMOJI_PHRASE_COUNT: usize = {stats['entry_count']};",
        "",
        "pub(crate) static EMOJI_PHRASES: phf::Map<&'static str, &'static str> = phf::phf_map! {",
    ]
    for phrase, emoji in sorted(entries.items()):
        lines.append(f"    {rust_string(phrase)} => {rust_string(emoji)},")
    lines.extend(["};", ""])
    OUTPUT_PATH.write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    entries, stats = build_map()
    write_generated(entries, stats)
    print(
        "generated {entry_count} phrases from {emoji_count} emoji "
        "({canonical_collision_count} canonical collisions, "
        "{ambiguous_keyword_count} ambiguous keywords skipped, "
        "{keyword_conflict_count} keyword conflicts skipped, "
        "{alias_count} aliases)".format(**stats)
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
