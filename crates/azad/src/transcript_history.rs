#![cfg_attr(test, allow(dead_code))]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

const MAX_ENTRIES: usize = 1000;
const SCHEMA_DDL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS transcripts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts_ms INTEGER NOT NULL,
  turn_id INTEGER NOT NULL,
  draft_text TEXT NOT NULL,
  final_text TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS transcripts_ts ON transcripts(ts_ms DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
  final_text,
  content = 'transcripts',
  content_rowid = 'id',
  tokenize = "unicode61 remove_diacritics 2",
  prefix = '2 3'
);

CREATE TRIGGER IF NOT EXISTS transcripts_ai AFTER INSERT ON transcripts BEGIN
  INSERT INTO transcripts_fts(rowid, final_text)
  VALUES (new.id, new.final_text);
END;
CREATE TRIGGER IF NOT EXISTS transcripts_ad AFTER DELETE ON transcripts BEGIN
  INSERT INTO transcripts_fts(transcripts_fts, rowid, final_text)
  VALUES ('delete', old.id, old.final_text);
END;
CREATE TRIGGER IF NOT EXISTS transcripts_au AFTER UPDATE ON transcripts BEGIN
  INSERT INTO transcripts_fts(transcripts_fts, rowid, final_text)
  VALUES ('delete', old.id, old.final_text);
  INSERT INTO transcripts_fts(rowid, final_text)
  VALUES (new.id, new.final_text);
END;
"#;

/// Sentinel chars FTS5's `highlight()` wraps each match with. Chosen because
/// they never appear in transcribed speech text.
const HL_OPEN: char = '\u{0001}';
const HL_CLOSE: char = '\u{0002}';

#[derive(Debug, Serialize, Deserialize)]
struct TranscriptRecord {
  schema_version: u8,
  ts_ms: i64,
  turn_id: u64,
  draft_text: String,
  final_text: String,
}

#[derive(Debug, Clone)]
struct CachedEntry {
  #[allow(dead_code)] // present for symmetry with HistoryHit; not consumed today.
  id: i64,
  ts_ms: i64,
  final_text: String,
}

/// One row of a search result with the FTS-derived match positions inside
/// `final_text`. Empty `match_ranges` means "no highlight" — used for the
/// empty-query path that returns the full cache.
#[derive(Debug, Clone)]
pub struct HistoryHit {
  pub ts_ms: i64,
  pub final_text: String,
  pub match_ranges: Vec<(usize, usize)>,
}

pub struct TranscriptIndex {
  conn: Connection,
  /// Newest-first cache backing `entry_text` / `entry_ts_ms` / unfiltered
  /// renders. Capped at `MAX_ENTRIES`; the SQLite table holds the full
  /// history (the cap only affects what's held in RAM, never what's
  /// persisted).
  cache: Vec<CachedEntry>,
}

impl TranscriptIndex {
  #[cfg(test)]
  pub fn in_memory_for_tests() -> Self {
    let conn = Connection::open_in_memory().expect("open in-memory transcript history");
    conn.execute_batch(SCHEMA_DDL).expect("initialize in-memory transcript history");
    TranscriptIndex { conn, cache: Vec::with_capacity(MAX_ENTRIES) }
  }

  pub fn load() -> Option<Self> {
    let db_path = transcript_db_path()?;
    if let Some(parent) = db_path.parent() {
      let _ = fs::create_dir_all(parent);
    }

    // Try the persistent file. If anything goes wrong, fall back to an
    // in-memory DB so the app keeps working for the session — the user
    // sees an empty history and we log a warning, but nothing crashes.
    let conn = match Connection::open_with_flags(
      &db_path,
      OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    ) {
      Ok(conn) => conn,
      Err(err) => {
        eprintln!("AZAD_HISTORY: failed to open {}: {}", db_path.display(), err);
        Connection::open_in_memory().ok()?
      }
    };

    if let Err(err) = conn.execute_batch(SCHEMA_DDL) {
      eprintln!("AZAD_HISTORY: schema DDL failed: {err}");
      return None;
    }

    let mut index = TranscriptIndex { conn, cache: Vec::with_capacity(MAX_ENTRIES) };

    // One-shot migration from the legacy JSONL — runs only when the
    // SQLite table is empty (so re-launches with an existing DB are
    // a no-op).
    if index.is_empty_table() {
      if let Some(jsonl) = transcript_jsonl_path() {
        if jsonl.exists() {
          match index.migrate_from_jsonl(&jsonl) {
            Ok(count) => {
              eprintln!("AZAD_HISTORY: migrated {count} entries from {}", jsonl.display());
              let bak = jsonl.with_extension("jsonl.bak");
              if let Err(err) = fs::rename(&jsonl, &bak) {
                eprintln!("AZAD_HISTORY: rename to .bak failed: {err}");
              }
            }
            Err(err) => {
              eprintln!("AZAD_HISTORY: migration failed; leaving JSONL in place: {err}");
            }
          }
        }
      }
    }

    index.refill_cache();
    Some(index)
  }

  pub fn append(&mut self, turn_id: u64, draft_text: &str, final_text: &str) {
    let final_text = final_text.trim();
    if final_text.is_empty() {
      return;
    }
    let ts_ms = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .map(|d| d.as_millis() as i64)
      .unwrap_or(0);

    let insert = self.conn.execute(
      "INSERT INTO transcripts(ts_ms, turn_id, draft_text, final_text) VALUES (?, ?, ?, ?)",
      params![ts_ms, turn_id as i64, draft_text, final_text],
    );
    let id = match insert {
      Ok(_) => self.conn.last_insert_rowid(),
      Err(err) => {
        eprintln!("AZAD_HISTORY: append failed: {err}");
        return;
      }
    };

    // Front-of-cache; truncate any stale tail.
    self
      .cache
      .insert(0, CachedEntry { id, ts_ms, final_text: final_text.to_string() });
    if self.cache.len() > MAX_ENTRIES {
      self.cache.truncate(MAX_ENTRIES);
    }

    // SQLite holds the full transcript history forever; the `MAX_ENTRIES`
    // cap only bounds the in-memory `cache` (which is what the overlay's
    // index-by-position rendering walks). Disk size at ~200 bytes/entry
    // stays trivial even past 100k entries; if a user ever hits that
    // scale we'd revisit with a vacuum job.
  }

  pub fn entry_count(&self) -> usize {
    self.cache.len()
  }

  #[allow(dead_code)] // Public API; the renderer now consumes search() results
  // (which carry the text inline) so direct cache indexing isn't called
  // from app code, but the accessor stays available.
  pub fn entry_text(&self, index: usize) -> Option<&str> {
    self.cache.get(index).map(|e| e.final_text.as_str())
  }

  #[allow(dead_code)] // Used by the renderer via the public path; some callers might not.
  pub fn entry_ts_ms(&self, index: usize) -> Option<i64> {
    self.cache.get(index).map(|e| e.ts_ms)
  }

  /// Ranked FTS5 search. Empty / pure-whitespace query short-circuits to
  /// the cache in newest-first order (with empty `match_ranges`), matching
  /// the no-filter behaviour. Tokens are split on whitespace; non-alnum
  /// punctuation is stripped; each token is a prefix-phrase
  /// (`"<token>"*`). Tokens are joined by space (FTS5 implicit AND) so all
  /// must appear. Results sorted by BM25 (best first) with recency as the
  /// tiebreak.
  pub fn search(&self, query: &str, limit: usize) -> Vec<HistoryHit> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
      return self
        .cache
        .iter()
        .take(limit)
        .map(|e| HistoryHit {
          ts_ms: e.ts_ms,
          final_text: e.final_text.clone(),
          match_ranges: Vec::new(),
        })
        .collect();
    }

    let Some(fts_query) = build_fts_query(trimmed) else {
      // Query was non-empty but contained nothing tokenisable — fall back
      // to cache so the user sees something rather than an inscrutable
      // blank.
      return self
        .cache
        .iter()
        .take(limit)
        .map(|e| HistoryHit {
          ts_ms: e.ts_ms,
          final_text: e.final_text.clone(),
          match_ranges: Vec::new(),
        })
        .collect();
    };

    let mut stmt = match self.conn.prepare(
      "SELECT t.ts_ms, \
              highlight(transcripts_fts, 0, char(1), char(2)) AS hl \
       FROM transcripts_fts \
       JOIN transcripts t ON t.id = transcripts_fts.rowid \
       WHERE transcripts_fts MATCH ? \
       ORDER BY bm25(transcripts_fts) ASC, t.ts_ms DESC \
       LIMIT ?",
    ) {
      Ok(s) => s,
      Err(err) => {
        eprintln!("AZAD_HISTORY: prepare search failed: {err}");
        return Vec::new();
      }
    };

    let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
      let ts_ms: i64 = row.get(0)?;
      let highlighted: String = row.get(1)?;
      let (clean, ranges) = extract_highlight_ranges(&highlighted);
      Ok(HistoryHit { ts_ms, final_text: clean, match_ranges: ranges })
    });

    match rows {
      Ok(iter) => iter.filter_map(Result::ok).collect(),
      Err(err) => {
        eprintln!("AZAD_HISTORY: search failed: {err}");
        Vec::new()
      }
    }
  }

  fn is_empty_table(&self) -> bool {
    self
      .conn
      .query_row("SELECT COUNT(*) FROM transcripts", [], |row| row.get::<_, i64>(0))
      .map(|n| n == 0)
      .unwrap_or(true)
  }

  fn refill_cache(&mut self) {
    self.cache.clear();
    let mut stmt = match self
      .conn
      .prepare("SELECT id, ts_ms, final_text FROM transcripts ORDER BY ts_ms DESC LIMIT ?")
    {
      Ok(s) => s,
      Err(err) => {
        eprintln!("AZAD_HISTORY: prepare refill failed: {err}");
        return;
      }
    };
    let rows = stmt.query_map(params![MAX_ENTRIES as i64], |row| {
      Ok(CachedEntry { id: row.get(0)?, ts_ms: row.get(1)?, final_text: row.get(2)? })
    });
    if let Ok(iter) = rows {
      for entry in iter.flatten() {
        self.cache.push(entry);
      }
    }
  }

  fn migrate_from_jsonl(&mut self, path: &PathBuf) -> rusqlite::Result<usize> {
    let file = match fs::File::open(path) {
      Ok(f) => f,
      Err(err) => {
        eprintln!("AZAD_HISTORY: migration open failed: {err}");
        return Ok(0);
      }
    };
    let reader = BufReader::new(file);
    let mut count = 0usize;
    let tx = self.conn.unchecked_transaction()?;
    {
      let mut stmt = tx.prepare(
        "INSERT INTO transcripts(ts_ms, turn_id, draft_text, final_text) VALUES (?, ?, ?, ?)",
      )?;
      for line in reader.lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
          continue;
        }
        let Ok(record) = serde_json::from_str::<TranscriptRecord>(line) else {
          continue;
        };
        if record.final_text.trim().is_empty() {
          continue;
        }
        stmt.execute(params![
          record.ts_ms,
          record.turn_id as i64,
          record.draft_text,
          record.final_text,
        ])?;
        count += 1;
      }
    }
    tx.commit()?;
    Ok(count)
  }
}

/// Tokenise the user's free-form query into an FTS5 MATCH expression of
/// prefix-phrases ANDed together. Returns `None` if no usable tokens
/// survive the stripping pass — caller should treat that as "no filter".
fn build_fts_query(query: &str) -> Option<String> {
  let mut parts = Vec::new();
  for raw in query.split_whitespace() {
    let cleaned: String = raw.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    if cleaned.is_empty() {
      continue;
    }
    parts.push(format!("\"{cleaned}\"*"));
  }
  if parts.is_empty() { None } else { Some(parts.join(" ")) }
}

/// Walk a string returned by FTS5 `highlight()` (with sentinels `\u{0001}`
/// / `\u{0002}` wrapping each match) and recover the original text plus
/// `(start_byte, end_byte)` ranges in that *clean* text. The renderer's
/// existing UTF-16 conversion handles AppKit-side rendering.
fn extract_highlight_ranges(highlighted: &str) -> (String, Vec<(usize, usize)>) {
  let mut clean = String::with_capacity(highlighted.len());
  let mut ranges = Vec::new();
  let mut start: Option<usize> = None;
  for ch in highlighted.chars() {
    if ch == HL_OPEN {
      start = Some(clean.len());
    } else if ch == HL_CLOSE {
      if let Some(s) = start.take() {
        ranges.push((s, clean.len()));
      }
    } else {
      clean.push(ch);
    }
  }
  (clean, ranges)
}

/// Compact char-count label rendered to the right of each history row, ABOVE the
/// time-ago label. Mirrors [`format_timestamp_compact`]'s "tiny number + single-letter
/// suffix" style: a `c` suffix on every output disambiguates "characters" from "k =
/// kilometres / thousands / etc." at a glance. Plain integer up to 999, then `N.Mkc`
/// (truncated, not rounded) up to 9.9k, then `Nkc` for 10k+, clamped at `999kc` for
/// pathologically long transcripts. Truncation keeps `1999` reading as `1.9kc` — honest
/// about magnitude — instead of pretending it's already 2.0kc. Worst case display is
/// `999kc` (5 chars) which fits the 26 pt right-meta column at 8 pt font.
pub fn format_char_count_compact(n: usize) -> String {
  if n < 1_000 {
    format!("{n}c")
  } else if n < 10_000 {
    format!("{}.{}kc", n / 1000, (n % 1000) / 100)
  } else if n < 1_000_000 {
    format!("{}kc", n / 1000)
  } else {
    "999kc".to_string()
  }
}

/// Compact "time-ago" label rendered to the right of each history row.
/// Always 1-3 chars: e.g. "5s", "12m", "1h", "2d".
pub fn format_timestamp_compact(ts_ms: i64) -> String {
  let now_ms = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis() as i64)
    .unwrap_or(0);
  let delta = (now_ms - ts_ms).max(0);
  let secs = delta / 1000;
  if secs < 60 {
    return format!("{secs}s");
  }
  let mins = secs / 60;
  if mins < 60 {
    return format!("{mins}m");
  }
  let hours = mins / 60;
  if hours < 24 {
    return format!("{hours}h");
  }
  let days = hours / 24;
  format!("{days}d")
}

#[allow(dead_code)] // Reserved for the timestamp footer; kept public so adding it back is a
// one-line UI change rather than a re-wire.
pub fn format_timestamp_relative(ts_ms: i64) -> String {
  let now_ms = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis() as i64)
    .unwrap_or(0);

  let delta_ms = now_ms - ts_ms;
  if delta_ms < 0 {
    return "just now".to_string();
  }

  let delta_secs = delta_ms / 1000;
  if delta_secs < 60 {
    return "just now".to_string();
  }

  let delta_mins = delta_secs / 60;
  if delta_mins < 60 {
    return format!("{delta_mins}m ago");
  }

  let delta_hours = delta_mins / 60;
  if delta_hours < 24 {
    return format!("{delta_hours}h ago");
  }

  let days_ago = delta_hours / 24;
  format!("{days_ago}d ago")
}

fn transcript_db_path() -> Option<PathBuf> {
  let home = std::env::var("HOME").ok()?;
  Some(
    PathBuf::from(home)
      .join("Library")
      .join("Application Support")
      .join("Azad")
      .join("transcripts.db"),
  )
}

fn transcript_jsonl_path() -> Option<PathBuf> {
  let home = std::env::var("HOME").ok()?;
  Some(
    PathBuf::from(home)
      .join("Library")
      .join("Application Support")
      .join("Azad")
      .join("transcripts.jsonl"),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  fn open_in_memory() -> TranscriptIndex {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA_DDL).unwrap();
    TranscriptIndex { conn, cache: Vec::new() }
  }

  #[test]
  fn timestamp_formatting() {
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;

    assert_eq!(format_timestamp_relative(now_ms), "just now");
    assert_eq!(format_timestamp_relative(now_ms - 30_000), "just now");
    assert_eq!(format_timestamp_relative(now_ms - 120_000), "2m ago");
    assert_eq!(format_timestamp_relative(now_ms - 7_200_000), "2h ago");
    assert_eq!(format_timestamp_relative(now_ms - 172_800_000), "2d ago");
  }

  #[test]
  fn compact_timestamp_picks_largest_unit() {
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;

    assert_eq!(format_timestamp_compact(now_ms), "0s");
    assert_eq!(format_timestamp_compact(now_ms - 5_000), "5s");
    assert_eq!(format_timestamp_compact(now_ms - 90_000), "1m");
    assert_eq!(format_timestamp_compact(now_ms - 3_600_000), "1h");
    assert_eq!(format_timestamp_compact(now_ms - 172_800_000), "2d");
  }

  #[test]
  fn format_char_count_compact_handles_each_range() {
    // < 1000: plain integer + "c" suffix.
    assert_eq!(format_char_count_compact(0), "0c");
    assert_eq!(format_char_count_compact(1), "1c");
    assert_eq!(format_char_count_compact(47), "47c");
    assert_eq!(format_char_count_compact(100), "100c");
    assert_eq!(format_char_count_compact(500), "500c");
    assert_eq!(format_char_count_compact(999), "999c");
    // 1_000..10_000: N.Mkc (truncate, not round).
    assert_eq!(format_char_count_compact(1_000), "1.0kc");
    assert_eq!(format_char_count_compact(1_063), "1.0kc");
    assert_eq!(format_char_count_compact(1_100), "1.1kc");
    assert_eq!(format_char_count_compact(1_234), "1.2kc");
    assert_eq!(format_char_count_compact(1_999), "1.9kc");
    assert_eq!(format_char_count_compact(9_000), "9.0kc");
    assert_eq!(format_char_count_compact(9_999), "9.9kc");
    // 10_000..1_000_000: Nkc (no decimal).
    assert_eq!(format_char_count_compact(10_000), "10kc");
    assert_eq!(format_char_count_compact(12_345), "12kc");
    assert_eq!(format_char_count_compact(100_000), "100kc");
    assert_eq!(format_char_count_compact(999_999), "999kc");
    // 1_000_000+: clamp to "999kc".
    assert_eq!(format_char_count_compact(1_000_000), "999kc");
    assert_eq!(format_char_count_compact(usize::MAX), "999kc");
  }

  #[test]
  fn append_then_read_back() {
    let mut idx = open_in_memory();
    idx.append(1, "draft a", "hello world");
    idx.append(2, "draft b", "fixed the overlay padding bug");
    assert_eq!(idx.entry_count(), 2);
    assert_eq!(idx.entry_text(0), Some("fixed the overlay padding bug"));
    assert_eq!(idx.entry_text(1), Some("hello world"));
  }

  #[test]
  fn append_skips_empty_final_text() {
    let mut idx = open_in_memory();
    idx.append(1, "draft", "   ");
    assert_eq!(idx.entry_count(), 0);
  }

  #[test]
  fn cap_keeps_newest_in_cache() {
    let mut idx = open_in_memory();
    for i in 0..(MAX_ENTRIES + 50) {
      idx.append(i as u64, "", &format!("entry {i}"));
    }
    assert_eq!(idx.entry_count(), MAX_ENTRIES);
    // Most recent at index 0.
    assert!(idx.entry_text(0).unwrap().starts_with("entry "));
    let last_entry = format!("entry {}", MAX_ENTRIES + 49);
    assert_eq!(idx.entry_text(0).unwrap(), last_entry);
  }

  #[test]
  fn disk_unbounded_only_cache_is_capped() {
    let mut idx = open_in_memory();
    let total_appended = MAX_ENTRIES + 50;
    for i in 0..total_appended {
      idx.append(i as u64, "", &format!("entry {i}"));
    }
    // Cache is capped at MAX_ENTRIES.
    assert_eq!(idx.entry_count(), MAX_ENTRIES);
    // SQLite kept every record — the prune-on-append used to delete
    // anything past 1000, which silently lost migrated history.
    let total: i64 = idx
      .conn
      .query_row("SELECT COUNT(*) FROM transcripts", [], |r| r.get(0))
      .unwrap();
    assert_eq!(total as usize, total_appended);
  }

  #[test]
  fn empty_query_returns_full_cache() {
    let mut idx = open_in_memory();
    idx.append(1, "", "alpha");
    idx.append(2, "", "beta");
    let hits = idx.search("", 50);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].final_text, "beta");
    assert_eq!(hits[1].final_text, "alpha");
    assert!(hits[0].match_ranges.is_empty());
  }

  #[test]
  fn whitespace_only_query_falls_back_to_cache() {
    let mut idx = open_in_memory();
    idx.append(1, "", "alpha");
    let hits = idx.search("   \t  ", 50);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].match_ranges.is_empty());
  }

  #[test]
  fn punctuation_only_query_falls_back_to_cache() {
    let mut idx = open_in_memory();
    idx.append(1, "", "alpha");
    let hits = idx.search("!@#$%^&*()", 50);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].match_ranges.is_empty());
  }

  #[test]
  fn token_and_search_any_order() {
    let mut idx = open_in_memory();
    idx.append(1, "", "fixed the overlay padding bug"); // both tokens
    idx.append(2, "", "the overlay was buggy and I want to fix it"); // both tokens
    idx.append(3, "", "the overlay needs more padding"); // missing 'fix'
    idx.append(4, "", "fix the search query"); // missing 'overlay'
    let hits = idx.search("fix overlay", 10);
    let texts: Vec<_> = hits.iter().map(|h| h.final_text.as_str()).collect();
    assert!(texts.contains(&"fixed the overlay padding bug"));
    assert!(texts.contains(&"the overlay was buggy and I want to fix it"));
    assert_eq!(hits.len(), 2);
  }

  #[test]
  fn token_prefix_match() {
    let mut idx = open_in_memory();
    idx.append(1, "", "transcription pipeline");
    idx.append(2, "", "transcripts overview");
    idx.append(3, "", "the unrelated entry");
    let hits = idx.search("trans", 10);
    assert_eq!(hits.len(), 2);
  }

  #[test]
  fn match_ranges_align_with_clean_text() {
    let mut idx = open_in_memory();
    idx.append(1, "", "fixed the overlay bug");
    let hits = idx.search("overlay", 1);
    assert_eq!(hits.len(), 1);
    let h = &hits[0];
    assert_eq!(h.final_text, "fixed the overlay bug");
    // Single matched token => single range covering "overlay".
    assert_eq!(h.match_ranges.len(), 1);
    let (s, e) = h.match_ranges[0];
    assert_eq!(&h.final_text[s..e].to_lowercase(), "overlay");
  }

  #[test]
  fn build_fts_query_strips_punctuation() {
    assert_eq!(build_fts_query("hello, world!").as_deref(), Some("\"hello\"* \"world\"*"));
    assert_eq!(build_fts_query("  fix\toverlay  ").as_deref(), Some("\"fix\"* \"overlay\"*"));
  }

  #[test]
  fn build_fts_query_returns_none_for_empty_or_punct_only() {
    assert!(build_fts_query("").is_none());
    assert!(build_fts_query("   ").is_none());
    assert!(build_fts_query("!@#$").is_none());
  }

  #[test]
  fn extract_highlight_ranges_recovers_original_and_positions() {
    let highlighted = format!(
      "fixed the {open}overlay{close} bug and {open}overlay{close} pad",
      open = HL_OPEN,
      close = HL_CLOSE
    );
    let (clean, ranges) = extract_highlight_ranges(&highlighted);
    assert_eq!(clean, "fixed the overlay bug and overlay pad");
    assert_eq!(ranges.len(), 2);
    let (s, e) = ranges[0];
    assert_eq!(&clean[s..e], "overlay");
    let (s, e) = ranges[1];
    assert_eq!(&clean[s..e], "overlay");
  }
}
