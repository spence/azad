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

struct TranscriptEntry {
  ts_ms: i64,
  final_text: String,
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
              });
            }
          }
        }

        if entries.len() > MAX_ENTRIES {
          entries.drain(..entries.len() - MAX_ENTRIES);
        }

        if line_count > COMPACT_THRESHOLD {
          compact_file(&path, &entries);
        }
      }
    }

    // Newest first
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

    self
      .entries
      .insert(0, TranscriptEntry { ts_ms, final_text: final_text.to_string() });

    if self.entries.len() > MAX_ENTRIES {
      self.entries.truncate(MAX_ENTRIES);
    }
  }

  pub fn entry_count(&self) -> usize {
    self.entries.len()
  }

  pub fn entry_text(&self, index: usize) -> Option<&str> {
    self.entries.get(index).map(|e| e.final_text.as_str())
  }

  #[allow(dead_code)] // Public API; current list overlay omits per-entry timestamps but a
  // future footer / inline label can pick this back up without churn.
  pub fn entry_ts_ms(&self, index: usize) -> Option<i64> {
    self.entries.get(index).map(|e| e.ts_ms)
  }
}

/// Compact "time-ago" label rendered to the left of each history row.
/// Always 1-3 chars: e.g. "5s", "12m", "1h", "2d". Picks the largest unit
/// whose count is ≥ 1 so the label stays terse — there's a fixed-width
/// column reserved for it in the overlay so neighbouring rows align.
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
  fn timestamp_formatting() {
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;

    assert_eq!(format_timestamp_relative(now_ms), "just now");
    assert_eq!(format_timestamp_relative(now_ms - 30_000), "just now");
    assert_eq!(format_timestamp_relative(now_ms - 120_000), "2m ago");
    assert_eq!(format_timestamp_relative(now_ms - 7_200_000), "2h ago");
    assert_eq!(format_timestamp_relative(now_ms - 172_800_000), "2d ago");
  }
}
