//! Input + lifecycle logging for after-the-fact bug repros.
//!
//! Appends one JSON line per user input or engine lifecycle event to
//! `~/Library/Logs/Azad/input.log`. Input entries include a small state snapshot;
//! platform lifecycle entries omit app state because they can originate outside
//! the controller thread.
//!
//! Privacy: text content is NEVER logged — only character counts and turn ids.
//! For the actual drafts, cross-reference the debug-recordings sidecar JSONs.
//!
//! Rotation: when the file exceeds [`MAX_LOG_BYTES`], the existing log is moved
//! to `input.log.1` (overwriting any prior rotation) and a fresh log is
//! started. One rotation slot is enough — the goal is "the last few minutes
//! before the bug," not unlimited history.
//!
//! See `crates/azad/src/app.rs` callsites for the events that get emitted.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

const INPUT_LOG_SCHEMA_VERSION: u8 = 1;
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
static INPUT_LOG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize)]
pub struct InputLogEntry {
  pub schema_version: u8,
  pub ts_ms: i64,
  #[serde(flatten)]
  pub event: InputLogEvent,
  pub state: StateSnapshot,
}

/// Minimum-viable AppController state for each entry. Refresh from the live
/// controller at the moment the event is logged.
#[derive(Debug, Clone, Serialize)]
pub struct StateSnapshot {
  pub finalizing_turn_id: Option<u64>,
  pub current_turn_id: Option<u64>,
  pub latest_seen_turn_id: u64,
  pub finalizing_draft_chars: usize,
  pub latest_draft_chars: usize,
  pub engine_state: &'static str,
  pub manual_hold_active: bool,
  pub overlay_visible: bool,
  pub saw_vad_start_during_finalizing: bool,
  pub history_browsing: bool,
  pub last_pasted_turn_id: Option<u64>,
  pub raw_handled_turn_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum InputLogEvent {
  /// Opt+Space pressed (start of dictation hold).
  HotkeyPressed,
  /// Opt+Space released.
  HotkeyReleased { raw_requested: bool },
  /// Enter (or Opt+Enter) pressed in the overlay context.
  FinalizeHotkeyPressed { raw_requested: bool },
  /// Esc pressed (overlay cancel).
  OverlayCancel,
  /// Up / Down pressed.
  ArrowNavigate { direction: i32 },
  /// Right arrow expanded a history entry to full text.
  HistoryExpand,
  /// Left arrow collapsed an expanded history entry.
  HistoryCollapse,
  /// History search bar typed/edited.
  HistorySearchEdit { kind: &'static str, chars_appended: Option<usize> },
  /// Engine reported speech started (VAD start).
  EngineSpeechStart,
  /// Engine reported entering finalize state for a turn.
  EngineSpeechFinalizing { turn_id: u64, draft_chars: usize },
  /// Engine cancelled a tentative finalize.
  EngineFinalizingCancelled { turn_id: u64 },
  /// Engine emitted a final transcript for a turn.
  EngineFinalText { turn_id: u64, text_chars: usize },
  /// App dropped a final transcript because the turn never became user-visible.
  FinalTextSuppressed { turn_id: u64, text_chars: usize, reason: &'static str },
  /// Engine reported the audio session ended.
  EngineSessionEnded,
  /// App-side paste attempt (normal or raw).
  PasteAttempt { turn_id: u64, mode: &'static str, source: &'static str, paste_ok: bool },
  /// Raw fallback fired from `handle_finalize_hotkey_pressed` because the engine
  /// looked stuck (`engine_state == Idle && finalizing_turn_id.is_some() &&
  /// !finalizing_draft.empty() && last_pasted_turn_id != finalizing_turn_id`).
  RawFallbackFired { turn_id: u64 },
}

#[derive(Serialize)]
struct HotkeyTapLogEntry<'a> {
  schema_version: u8,
  ts_ms: i64,
  event: &'static str,
  action: &'a str,
  reason: &'a str,
  generation: u64,
  #[serde(skip_serializing_if = "Option::is_none")]
  avg_latency_us: Option<f32>,
}

/// Append `entry` to the input log. Best-effort — failures are silent because
/// logging must never crash the app or block the audio thread. No-op under
/// `cfg(test)` so unit tests that exercise input handlers don't pollute the
/// real `~/Library/Logs/Azad/input.log`.
pub fn append(entry: &InputLogEntry) {
  append_json(entry);
}

pub fn append_hotkey_tap(action: &str, reason: &str, generation: u64, avg_latency_us: Option<f32>) {
  append_json(&HotkeyTapLogEntry {
    schema_version: schema_version(),
    ts_ms: now_epoch_ms(),
    event: "hotkey_tap_lifecycle",
    action,
    reason,
    generation,
    avg_latency_us,
  });
}

fn append_json(entry: &impl Serialize) {
  if cfg!(test) {
    let _ = entry;
    return;
  }
  let Ok(_guard) = INPUT_LOG_LOCK.lock() else { return };
  let path = input_log_path();
  let parent = path.parent().map(PathBuf::from);
  if let Some(parent) = parent {
    let _ = fs::create_dir_all(&parent);
  }
  rotate_if_needed(&path);
  let Ok(mut line) = serde_json::to_string(entry) else {
    return;
  };
  line.push('\n');
  if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
    let _ = file.write_all(line.as_bytes());
  }
}

pub fn schema_version() -> u8 {
  INPUT_LOG_SCHEMA_VERSION
}

pub fn now_epoch_ms() -> i64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .ok()
    .and_then(|d| i64::try_from(d.as_millis()).ok())
    .unwrap_or(0)
}

fn rotate_if_needed(path: &PathBuf) {
  let Ok(meta) = fs::metadata(path) else { return };
  if meta.len() <= MAX_LOG_BYTES {
    return;
  }
  let mut rotated = path.clone();
  rotated.set_extension("log.1");
  let _ = fs::rename(path, rotated);
  let _ = File::create(path);
}

fn input_log_path() -> PathBuf {
  if let Some(home) = std::env::var_os("HOME") {
    return PathBuf::from(home).join("Library").join("Logs").join("Azad").join("input.log");
  }
  PathBuf::from("input.log")
}

#[cfg(test)]
mod tests {
  use super::{HotkeyTapLogEntry, InputLogEntry, InputLogEvent, StateSnapshot, schema_version};

  fn empty_state() -> StateSnapshot {
    StateSnapshot {
      finalizing_turn_id: None,
      current_turn_id: None,
      latest_seen_turn_id: 0,
      finalizing_draft_chars: 0,
      latest_draft_chars: 0,
      engine_state: "idle",
      manual_hold_active: false,
      overlay_visible: false,
      saw_vad_start_during_finalizing: false,
      history_browsing: false,
      last_pasted_turn_id: None,
      raw_handled_turn_id: None,
    }
  }

  #[test]
  fn entry_serialises_as_a_single_jsonl_line() {
    let entry = InputLogEntry {
      schema_version: schema_version(),
      ts_ms: 1_700_000_000_000,
      event: InputLogEvent::HotkeyPressed,
      state: empty_state(),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.starts_with('{'));
    assert!(line.ends_with('}'));
    assert!(!line.contains('\n'), "entry must serialise to a single line");
    assert!(line.contains("\"event\":\"hotkey_pressed\""));
    assert!(line.contains("\"engine_state\":\"idle\""));
  }

  #[test]
  fn finalize_event_records_raw_flag() {
    let entry = InputLogEntry {
      schema_version: 1,
      ts_ms: 0,
      event: InputLogEvent::FinalizeHotkeyPressed { raw_requested: true },
      state: empty_state(),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"event\":\"finalize_hotkey_pressed\""));
    assert!(line.contains("\"raw_requested\":true"));
  }

  #[test]
  fn hotkey_release_event_records_raw_flag() {
    let entry = InputLogEntry {
      schema_version: 1,
      ts_ms: 0,
      event: InputLogEvent::HotkeyReleased { raw_requested: true },
      state: empty_state(),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"event\":\"hotkey_released\""));
    assert!(line.contains("\"raw_requested\":true"));
  }

  #[test]
  fn raw_fallback_event_records_target_turn() {
    let entry = InputLogEntry {
      schema_version: 1,
      ts_ms: 0,
      event: InputLogEvent::RawFallbackFired { turn_id: 42 },
      state: empty_state(),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"event\":\"raw_fallback_fired\""));
    assert!(line.contains("\"turn_id\":42"));
  }

  #[test]
  fn suppressed_final_text_event_records_reason() {
    let entry = InputLogEntry {
      schema_version: 1,
      ts_ms: 0,
      event: InputLogEvent::FinalTextSuppressed {
        turn_id: 42,
        text_chars: 4,
        reason: "hidden_without_visible_draft",
      },
      state: empty_state(),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"event\":\"final_text_suppressed\""));
    assert!(line.contains("\"turn_id\":42"));
    assert!(line.contains("\"text_chars\":4"));
    assert!(line.contains("\"reason\":\"hidden_without_visible_draft\""));
  }

  #[test]
  fn hotkey_tap_lifecycle_entry_omits_app_state() {
    let entry = HotkeyTapLogEntry {
      schema_version: 1,
      ts_ms: 0,
      event: "hotkey_tap_lifecycle",
      action: "recreate",
      reason: "stale_latency",
      generation: 2,
      avg_latency_us: Some(2_491_336_700.0),
    };
    let line = serde_json::to_string(&entry).unwrap();
    assert!(line.contains("\"event\":\"hotkey_tap_lifecycle\""));
    assert!(line.contains("\"action\":\"recreate\""));
    assert!(line.contains("\"reason\":\"stale_latency\""));
    assert!(!line.contains("\"state\""));
  }
}
