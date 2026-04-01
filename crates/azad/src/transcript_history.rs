use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MAX_ENTRIES: usize = 1000;
const COMPACT_THRESHOLD: usize = 2000;

#[derive(Debug, Serialize, Deserialize)]
struct TranscriptRecord {
  schema_version: u8,
  ts_ms: i64,
  turn_id: u64,
  draft_text: String,
  final_text: String,
}

#[derive(Debug, Clone)]
pub struct AutocompleteMatch {
  pub final_text: String,
  #[allow(dead_code)]
  pub ts_ms: i64,
}

struct TranscriptEntry {
  ts_ms: i64,
  final_text: String,
  words_lower: Vec<String>,
}

pub struct TranscriptIndex {
  entries: Vec<TranscriptEntry>,
  file_path: PathBuf,
}

impl TranscriptIndex {
  pub fn load() -> Option<Self> {
    let path = transcript_file_path()?;
    if let Some(parent) = path.parent() {
      let _ = fs::create_dir_all(parent);
    }

    let mut entries = Vec::new();
    if path.exists() {
      if let Ok(file) = File::open(&path) {
        let reader = BufReader::new(file);
        let mut line_count = 0usize;
        for line in reader.lines() {
          let Ok(line) = line else { continue };
          let line = line.trim();
          if line.is_empty() {
            continue;
          }
          line_count += 1;
          if let Ok(record) = serde_json::from_str::<TranscriptRecord>(line) {
            if !record.final_text.trim().is_empty() {
              entries.push(TranscriptEntry {
                ts_ms: record.ts_ms,
                final_text: record.final_text.clone(),
                words_lower: split_words_lower(&record.final_text),
              });
            }
          }
        }

        // Keep only the most recent MAX_ENTRIES
        if entries.len() > MAX_ENTRIES {
          entries.drain(..entries.len() - MAX_ENTRIES);
        }

        // Compact file if it grew too large
        if line_count > COMPACT_THRESHOLD {
          compact_file(&path, &entries);
        }
      }
    }

    // Reverse so newest is first
    entries.reverse();

    Some(TranscriptIndex { entries, file_path: path })
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

    let record = TranscriptRecord {
      schema_version: 1,
      ts_ms,
      turn_id,
      draft_text: draft_text.to_string(),
      final_text: final_text.to_string(),
    };

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.file_path) {
      if let Ok(json) = serde_json::to_string(&record) {
        let _ = writeln!(file, "{json}");
      }
    }

    self.entries.insert(0, TranscriptEntry {
      ts_ms,
      final_text: final_text.to_string(),
      words_lower: split_words_lower(final_text),
    });

    if self.entries.len() > MAX_ENTRIES {
      self.entries.truncate(MAX_ENTRIES);
    }
  }

  pub fn search_prefix(&self, draft: &str, limit: usize) -> Vec<AutocompleteMatch> {
    let draft = draft.trim();
    if draft.is_empty() {
      return Vec::new();
    }
    let query_words = split_words_lower(draft);
    if query_words.is_empty() {
      return Vec::new();
    }

    let mut matches = Vec::new();
    let mut seen_texts = Vec::new();

    for entry in &self.entries {
      if entry.words_lower.len() < query_words.len() {
        continue;
      }
      // Check if query words form a prefix of entry words
      let is_prefix = query_words.iter().enumerate().all(|(i, qw)| {
        if i < query_words.len() - 1 {
          // All words except the last must match exactly
          entry.words_lower[i] == *qw
        } else {
          // Last query word can be a prefix
          entry.words_lower[i].starts_with(qw.as_str())
        }
      });

      if !is_prefix {
        continue;
      }

      // Skip exact matches (user is saying exactly this, no need to autocomplete)
      if entry.words_lower.len() == query_words.len()
        && entry.words_lower.last() == query_words.last()
      {
        continue;
      }

      // Deduplicate by text (keep most recent, which comes first)
      let lower = entry.final_text.to_ascii_lowercase();
      if seen_texts.contains(&lower) {
        continue;
      }
      seen_texts.push(lower);

      matches.push(AutocompleteMatch { final_text: entry.final_text.clone(), ts_ms: entry.ts_ms });

      if matches.len() >= limit {
        break;
      }
    }

    matches
  }
}

#[allow(dead_code)]
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

  // Format as "Mon DD" using rough calculation
  let days_ago = delta_hours / 24;
  format!("{days_ago}d ago")
}

fn split_words_lower(text: &str) -> Vec<String> {
  text.split_whitespace().map(|w| w.to_ascii_lowercase()).collect()
}

fn transcript_file_path() -> Option<PathBuf> {
  let home = std::env::var("HOME").ok()?;
  Some(
    PathBuf::from(home)
      .join("Library")
      .join("Application Support")
      .join("Azad")
      .join("transcripts.jsonl"),
  )
}

fn compact_file(path: &PathBuf, entries: &[TranscriptEntry]) {
  let tmp_path = path.with_extension("jsonl.tmp");
  let Ok(mut file) = File::create(&tmp_path) else { return };

  // Write entries oldest-first (entries are stored newest-first in memory)
  for entry in entries.iter().rev() {
    let record = TranscriptRecord {
      schema_version: 1,
      ts_ms: entry.ts_ms,
      turn_id: 0,
      draft_text: String::new(),
      final_text: entry.final_text.clone(),
    };
    if let Ok(json) = serde_json::to_string(&record) {
      let _ = writeln!(file, "{json}");
    }
  }

  let _ = fs::rename(&tmp_path, path);
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn prefix_search_basic() {
    let entries = vec![
      TranscriptEntry {
        ts_ms: 1000,
        final_text: "hello world".to_string(),
        words_lower: split_words_lower("hello world"),
      },
      TranscriptEntry {
        ts_ms: 2000,
        final_text: "hello there friend".to_string(),
        words_lower: split_words_lower("hello there friend"),
      },
      TranscriptEntry {
        ts_ms: 3000,
        final_text: "goodbye world".to_string(),
        words_lower: split_words_lower("goodbye world"),
      },
    ];

    let index = TranscriptIndex { entries, file_path: PathBuf::from("/tmp/test.jsonl") };

    let matches = index.search_prefix("hello", 5);
    assert_eq!(matches.len(), 2);

    let matches = index.search_prefix("hello wor", 5);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].final_text, "hello world");

    let matches = index.search_prefix("good", 5);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].final_text, "goodbye world");

    let matches = index.search_prefix("xyz", 5);
    assert_eq!(matches.len(), 0);
  }

  #[test]
  fn prefix_search_deduplicates() {
    let entries = vec![
      TranscriptEntry {
        ts_ms: 3000,
        final_text: "hello world".to_string(),
        words_lower: split_words_lower("hello world"),
      },
      TranscriptEntry {
        ts_ms: 1000,
        final_text: "Hello World".to_string(),
        words_lower: split_words_lower("Hello World"),
      },
    ];

    let index = TranscriptIndex { entries, file_path: PathBuf::from("/tmp/test.jsonl") };

    let matches = index.search_prefix("hel", 5);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].ts_ms, 3000);
  }

  #[test]
  fn prefix_search_skips_exact_match() {
    let entries = vec![TranscriptEntry {
      ts_ms: 1000,
      final_text: "hello".to_string(),
      words_lower: split_words_lower("hello"),
    }];

    let index = TranscriptIndex { entries, file_path: PathBuf::from("/tmp/test.jsonl") };

    let matches = index.search_prefix("hello", 5);
    assert_eq!(matches.len(), 0);
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
}
