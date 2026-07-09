use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const METRICS_LOG_SCHEMA_VERSION: u8 = 1;
const LAST_24_HOURS_MS: i64 = 24 * 60 * 60 * 1000;
const RECENT_TRANSCRIPTS_LIMIT: usize = 10;
const RECENT_EVENT_ASSOCIATION_MAX_GAP_MS: u64 = 5 * 60 * 1000;
const SUMMARY_TRAILING_BLANK_LINES: usize = 2;

/// Recent-transcriptions table: `raw` (`x` if you held Option to raw-paste, else blank), `fixes`
/// (token edits the refined pass made to the live draft, or `-`), `preview`, `dur (s)` (turn
/// seconds). Shared by both render call sites so the widths can't drift apart.
const RECENT_TABLE_HEADERS: &[&str] = &["raw", "fixes", "preview", "dur (s)"];
const RECENT_TABLE_WIDTHS: &[usize] = &[3, 5, 40, 7];
/// The debug overlay wraps any row wider than this; the table must stay within it so each turn
/// occupies exactly one line. See `recent_transcriptions_row_fits_on_one_line`.
const RECENT_TABLE_MAX_WIDTH: usize = 69;

// Compile-time no-wrap guard: a rendered row is `sum(widths) + 3 per column gap`. If a future
// column change pushes it past the overlay budget, the build fails here — not the user's eyes.
const _: () = assert!(
  RECENT_TABLE_WIDTHS[0]
    + RECENT_TABLE_WIDTHS[1]
    + RECENT_TABLE_WIDTHS[2]
    + RECENT_TABLE_WIDTHS[3]
    + 3 * (RECENT_TABLE_WIDTHS.len() - 1)
    <= RECENT_TABLE_MAX_WIDTH
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMode {
  Raw,
  Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsLogRecord {
  pub schema_version: u8,
  pub ts_ms: i64,
  #[serde(flatten)]
  pub event: MetricsLogEvent,
}

impl MetricsLogRecord {
  pub fn new(event: MetricsLogEvent) -> Self {
    Self { schema_version: METRICS_LOG_SCHEMA_VERSION, ts_ms: now_epoch_ms(), event }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum MetricsLogEvent {
  TurnCompleted {
    turn_id: u64,
    mode: TranscriptMode,
    transcription_duration_ms: u64,
  },
  PasteCompleted {
    turn_id: u64,
    mode: TranscriptMode,
    paste_duration_ms: u64,
    result: String,
  },
  /// Quality signal. Under dual-stream this is the draft->refined-final token divergence (how much
  /// the refined stream corrected the live draft at finalize). Legacy records used the same shape
  /// for emitted-vs-whole-turn-re-decode divergence, so both eras parse identically.
  PartialAuditResult {
    turn_id: u64,
    emitted_kind: String,
    exact: bool,
    partial_count: usize,
    emitted_tokens: usize,
    full_tokens: usize,
    edit_distance: usize,
    wer_like: f64,
    lcp_tokens: usize,
    lcp_pct: f64,
  },
  TurnSnapshot {
    turn_id: u64,
    mode: TranscriptMode,
    transcription_duration_ms: u64,
    text_preview: String,
  },
}

#[derive(Debug, Clone, Default)]
pub struct DurationStats {
  pub count: usize,
  pub avg_ms: f64,
  pub p50_ms: u64,
  pub p95_ms: u64,
  pub max_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MetricsSummary {
  pub total_transcriptions: usize,
  pub raw_transcriptions: usize,
  pub normal_transcriptions: usize,
  pub transcription_all: DurationStats,
  pub transcription_raw: DurationStats,
  pub transcription_normal: DurationStats,
  pub paste: DurationStats,
  pub quality_samples: usize,
  pub quality_exact_rate_pct: f64,
  pub quality_avg_edit_distance: f64,
  pub quality_avg_wer_like: f64,
  pub quality_avg_lcp_pct: f64,
  pub recent_transcripts: Vec<RecentTranscriptSummary>,
}

#[derive(Debug, Clone)]
pub struct RecentTranscriptSummary {
  pub turn_id: u64,
  pub mode: TranscriptMode,
  pub transcription_duration_ms: u64,
  /// Fixes the refined pass applied to the live draft at finalize (token edit distance — words or
  /// punctuation). Small nonzero == subtle in-place sharpening; not an error rate (no ground truth).
  pub refined_fixes: Option<usize>,
  pub text_preview: String,
}

pub fn append_record(record: &MetricsLogRecord) -> std::io::Result<()> {
  let path = metrics_log_path();
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)?;
  }

  let mut file = OpenOptions::new().create(true).append(true).open(path)?;
  serde_json::to_writer(&mut file, record)?;
  file.write_all(b"\n")?;
  Ok(())
}

pub fn read_records_since(since_epoch_ms: i64) -> std::io::Result<Vec<MetricsLogRecord>> {
  let path = metrics_log_path();
  if !path.exists() {
    return Ok(Vec::new());
  }

  let file = File::open(path)?;
  let reader = BufReader::new(file);
  let mut records = Vec::new();
  for line in reader.lines() {
    let line = match line {
      Ok(v) => v,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    let record = match serde_json::from_str::<MetricsLogRecord>(&line) {
      Ok(v) => v,
      Err(_) => continue,
    };
    if record.schema_version != METRICS_LOG_SCHEMA_VERSION {
      continue;
    }
    if record.ts_ms < since_epoch_ms {
      continue;
    }
    records.push(record);
  }
  Ok(records)
}

pub fn summarize_last_24h() -> std::io::Result<MetricsSummary> {
  let since = now_epoch_ms().saturating_sub(LAST_24_HOURS_MS);
  let records = read_records_since(since)?;
  Ok(summarize(&records))
}

pub fn render_summary(summary: &MetricsSummary) -> String {
  let mut lines = Vec::new();
  lines.push("Debug statistics (last 24h)".to_string());
  lines.push(String::new());
  lines.push(format!("Transcriptions total: {}", summary.total_transcriptions));
  lines.push(format!("Transcriptions raw: {}", summary.raw_transcriptions));
  lines.push(format!("Transcriptions normal: {}", summary.normal_transcriptions));
  lines.push(String::new());
  lines.push("Latency (ms)".to_string());
  lines.extend(render_table(
    &["scope", "n", "avg", "p50", "p95", "max"],
    &[8, 5, 8, 6, 6, 6],
    &[
      duration_row("all", &summary.transcription_all),
      duration_row("raw", &summary.transcription_raw),
      duration_row("normal", &summary.transcription_normal),
      duration_row("paste", &summary.paste),
    ],
  ));
  lines.push(String::new());
  // Draft->refined-final divergence: how much the refined stream corrected the live draft at
  // finalize (token edit distance; small nonzero == subtle in-place sharpenings, goal #3).
  lines.push(format!("Draft->final divergence samples: {}", summary.quality_samples));
  if summary.quality_samples == 0 {
    lines.push("Draft->final exact_pct: n/a".to_string());
    lines.push("Draft->final avg_edit: n/a".to_string());
    lines.push("Draft->final avg_wer: n/a".to_string());
    lines.push("Draft->final avg_lcp_pct: n/a".to_string());
  } else {
    lines.push(format!("Draft->final exact_pct: {:.1}", summary.quality_exact_rate_pct));
    lines.push(format!("Draft->final avg_edit: {:.2}", summary.quality_avg_edit_distance));
    lines.push(format!("Draft->final avg_wer: {:.3}", summary.quality_avg_wer_like));
    lines.push(format!("Draft->final avg_lcp_pct: {:.1}", summary.quality_avg_lcp_pct));
  }
  lines.push(String::new());
  lines.push(format!("Recent transcriptions (latest {})", RECENT_TRANSCRIPTS_LIMIT));
  if summary.recent_transcripts.is_empty() {
    lines.extend(render_table(
      RECENT_TABLE_HEADERS,
      RECENT_TABLE_WIDTHS,
      &[vec!["-".to_string(), "-".to_string(), "-".to_string(), "-".to_string()]],
    ));
  } else {
    let mut rows = Vec::new();
    for sample in &summary.recent_transcripts {
      rows.push(vec![
        recent_raw_label(sample).to_string(),
        recent_fixes_label(sample),
        sample.text_preview.clone(),
        format_duration_secs(sample.transcription_duration_ms),
      ]);
    }
    lines.extend(render_table(RECENT_TABLE_HEADERS, RECENT_TABLE_WIDTHS, &rows));
  }
  for _ in 0..SUMMARY_TRAILING_BLANK_LINES {
    lines.push(String::new());
  }
  lines.join("\n")
}

pub fn now_epoch_ms() -> i64 {
  use std::time::{SystemTime, UNIX_EPOCH};

  match SystemTime::now().duration_since(UNIX_EPOCH) {
    Ok(dur) => i64::try_from(dur.as_millis()).unwrap_or(i64::MAX),
    Err(_) => 0,
  }
}

fn summarize(records: &[MetricsLogRecord]) -> MetricsSummary {
  type AuditSample = (i64, bool, usize, f64, f64);

  let mut audits_by_turn: HashMap<u64, Vec<AuditSample>> = HashMap::new();
  let mut recent_snapshots: Vec<(i64, RecentTranscriptSummary)> = Vec::new();
  let mut summary = MetricsSummary::default();
  let mut trans_all = Vec::new();
  let mut trans_raw = Vec::new();
  let mut trans_normal = Vec::new();
  let mut paste_values = Vec::new();
  let mut exact_count = 0usize;
  let mut sum_edit = 0usize;
  let mut sum_wer = 0.0f64;
  let mut sum_lcp = 0.0f64;

  for record in records {
    match &record.event {
      MetricsLogEvent::TurnCompleted { mode, transcription_duration_ms, .. } => {
        summary.total_transcriptions += 1;
        trans_all.push(*transcription_duration_ms);
        match mode {
          TranscriptMode::Raw => {
            summary.raw_transcriptions += 1;
            trans_raw.push(*transcription_duration_ms);
          }
          TranscriptMode::Normal => {
            summary.normal_transcriptions += 1;
            trans_normal.push(*transcription_duration_ms);
          }
        }
      }
      MetricsLogEvent::PasteCompleted { paste_duration_ms, .. } => {
        paste_values.push(*paste_duration_ms);
      }
      MetricsLogEvent::PartialAuditResult {
        turn_id,
        exact,
        edit_distance,
        wer_like,
        lcp_pct,
        ..
      } => {
        summary.quality_samples += 1;
        if *exact {
          exact_count += 1;
        }
        sum_edit += *edit_distance;
        sum_wer += *wer_like;
        sum_lcp += *lcp_pct;
        audits_by_turn.entry(*turn_id).or_default().push((
          record.ts_ms,
          *exact,
          *edit_distance,
          *wer_like,
          *lcp_pct,
        ));
      }
      MetricsLogEvent::TurnSnapshot { turn_id, mode, transcription_duration_ms, text_preview } => {
        recent_snapshots.push((
          record.ts_ms,
          RecentTranscriptSummary {
            turn_id: *turn_id,
            mode: *mode,
            transcription_duration_ms: *transcription_duration_ms,
            refined_fixes: None,
            text_preview: text_preview.clone(),
          },
        ));
      }
    }
  }

  summary.transcription_all = duration_stats(&trans_all);
  summary.transcription_raw = duration_stats(&trans_raw);
  summary.transcription_normal = duration_stats(&trans_normal);
  summary.paste = duration_stats(&paste_values);

  if summary.quality_samples > 0 {
    let denom = summary.quality_samples as f64;
    summary.quality_exact_rate_pct = exact_count as f64 * 100.0 / denom;
    summary.quality_avg_edit_distance = sum_edit as f64 / denom;
    summary.quality_avg_wer_like = sum_wer / denom;
    summary.quality_avg_lcp_pct = sum_lcp / denom;
  }

  recent_snapshots.sort_by_key(|(ts_ms, _)| std::cmp::Reverse(*ts_ms));
  let mut recent_samples: Vec<(i64, RecentTranscriptSummary)> =
    recent_snapshots.into_iter().take(RECENT_TRANSCRIPTS_LIMIT).collect();

  for (snapshot_ts, sample) in &mut recent_samples {
    if let Some(audits) = audits_by_turn.get(&sample.turn_id) {
      if let Some((_, _, edit_distance, _, _)) = nearest_by_ts(audits, *snapshot_ts, |item| item.0)
      {
        sample.refined_fixes = Some(*edit_distance);
      }
    }
  }

  summary.recent_transcripts = recent_samples.into_iter().map(|(_, sample)| sample).collect();

  summary
}

fn nearest_by_ts<T, F>(items: &[T], target_ts: i64, ts_of: F) -> Option<&T>
where
  F: Fn(&T) -> i64,
{
  items
    .iter()
    .filter_map(|item| {
      let delta = abs_diff_ms(ts_of(item), target_ts);
      (delta <= RECENT_EVENT_ASSOCIATION_MAX_GAP_MS).then_some((delta, item))
    })
    .min_by_key(|(delta, _)| *delta)
    .map(|(_, item)| item)
}

fn abs_diff_ms(lhs: i64, rhs: i64) -> u64 {
  if lhs >= rhs { lhs.saturating_sub(rhs) as u64 } else { rhs.saturating_sub(lhs) as u64 }
}

fn duration_stats(values: &[u64]) -> DurationStats {
  if values.is_empty() {
    return DurationStats::default();
  }

  let mut sorted = values.to_vec();
  sorted.sort_unstable();
  let total = sorted.iter().sum::<u64>();
  DurationStats {
    count: sorted.len(),
    avg_ms: total as f64 / sorted.len() as f64,
    p50_ms: percentile(&sorted, 50.0),
    p95_ms: percentile(&sorted, 95.0),
    max_ms: sorted.last().copied().unwrap_or(0),
  }
}

fn percentile(sorted_values: &[u64], pct: f64) -> u64 {
  if sorted_values.is_empty() {
    return 0;
  }
  if sorted_values.len() == 1 {
    return sorted_values[0];
  }

  let rank = (pct / 100.0).clamp(0.0, 1.0) * (sorted_values.len() - 1) as f64;
  let lo = rank.floor() as usize;
  let hi = rank.ceil() as usize;
  if lo == hi {
    return sorted_values[lo];
  }
  let weight = rank - lo as f64;
  let lo_v = sorted_values[lo] as f64;
  let hi_v = sorted_values[hi] as f64;
  ((lo_v * (1.0 - weight)) + (hi_v * weight)).round() as u64
}

fn duration_row(scope: &str, stats: &DurationStats) -> Vec<String> {
  vec![
    scope.to_string(),
    stats.count.to_string(),
    format!("{:.1}", stats.avg_ms),
    stats.p50_ms.to_string(),
    stats.p95_ms.to_string(),
    stats.max_ms.to_string(),
  ]
}

fn render_table(headers: &[&str], widths: &[usize], rows: &[Vec<String>]) -> Vec<String> {
  let mut out = Vec::new();
  let header_cells = headers.iter().map(|s| s.to_string()).collect::<Vec<_>>();
  out.push(table_row(&header_cells, widths));
  out.push(table_divider(widths));
  for row in rows {
    out.push(table_row(row, widths));
  }
  out
}

fn table_divider(widths: &[usize]) -> String {
  widths.iter().map(|width| "-".repeat(*width)).collect::<Vec<_>>().join("-+-")
}

fn table_row(cells: &[String], widths: &[usize]) -> String {
  widths
    .iter()
    .copied()
    .enumerate()
    .map(|(idx, width)| {
      let cell = cells.get(idx).cloned().unwrap_or_default();
      let clipped = truncate_cell(&cell, width);
      format!("{:<width$}", clipped, width = width)
    })
    .collect::<Vec<_>>()
    .join(" | ")
}

fn truncate_cell(text: &str, width: usize) -> String {
  if width == 0 {
    return String::new();
  }
  let count = text.chars().count();
  if count <= width {
    return text.to_string();
  }
  if width <= 3 {
    return ".".repeat(width);
  }
  let keep = width - 3;
  let mut out = String::new();
  for ch in text.chars().take(keep) {
    out.push(ch);
  }
  out.push_str("...");
  out
}

fn recent_raw_label(sample: &RecentTranscriptSummary) -> &'static str {
  // `x` marks a raw paste (Option held at finalize); blank == the normal cleaned paste.
  match sample.mode {
    TranscriptMode::Raw => "x",
    TranscriptMode::Normal => "",
  }
}

fn recent_fixes_label(sample: &RecentTranscriptSummary) -> String {
  // Token edits (words or punctuation) the refined pass made to the live draft. 0 == the paste
  // matched what you saw; small nonzero == subtle in-place sharpening. Not an error rate — dual
  // has no ground-truth reference. `-` == no refined pass ran for this turn (e.g. a raw paste).
  match sample.refined_fixes {
    Some(fixes) => fixes.to_string(),
    None => "-".to_string(),
  }
}

/// Compact turn duration in seconds (the column header `dur (s)` carries the unit) for the
/// recent-transcriptions table. Raw ms reached six digits on long turns ("118378"), overflowing
/// the overlay width and wrapping the row. Tenths under 100 s, whole seconds above.
fn format_duration_secs(ms: u64) -> String {
  let secs = ms as f64 / 1000.0;
  if secs < 99.95 { format!("{secs:.1}") } else { format!("{secs:.0}") }
}

fn metrics_log_path() -> PathBuf {
  if let Some(home) = std::env::var_os("HOME") {
    return PathBuf::from(home)
      .join("Library")
      .join("Logs")
      .join("Azad")
      .join("metrics.log");
  }
  PathBuf::from("metrics.log")
}

#[cfg(test)]
mod tests {
  use super::{
    MetricsLogEvent, MetricsLogRecord, RECENT_TABLE_HEADERS, RECENT_TABLE_MAX_WIDTH,
    RECENT_TABLE_WIDTHS, TranscriptMode, duration_stats, format_duration_secs, percentile,
    render_table, summarize,
  };

  #[test]
  fn recent_transcriptions_row_fits_on_one_line() {
    // render_table pads/truncates every cell to its width, so a row can only wrap if the widths
    // total exceeds the overlay budget. Worst case: widest state label, an overlong preview, a
    // multi-minute duration. Guards the column widths against future growth.
    let rows = vec![vec![
      "x".to_string(),
      "999".to_string(),
      "x".repeat(200),
      format_duration_secs(3_599_000),
    ]];
    for line in render_table(RECENT_TABLE_HEADERS, RECENT_TABLE_WIDTHS, &rows) {
      assert!(
        line.chars().count() <= RECENT_TABLE_MAX_WIDTH,
        "recent-transcriptions line is {} cols (> {RECENT_TABLE_MAX_WIDTH}); it will wrap: {line:?}",
        line.chars().count()
      );
    }
  }

  #[test]
  fn format_duration_secs_stays_within_dur_column() {
    for ms in [0, 900, 3_741, 12_739, 99_949, 99_999, 118_378, 3_599_000] {
      let rendered = format_duration_secs(ms);
      assert!(rendered.len() <= 5, "`{rendered}` is too wide for the dur (s) column");
    }
    assert_eq!(format_duration_secs(3_741), "3.7");
    assert_eq!(format_duration_secs(118_378), "118");
  }

  #[test]
  fn percentile_interpolates_correctly() {
    let values = vec![10, 20, 30, 40];
    assert_eq!(percentile(&values, 50.0), 25);
    assert_eq!(percentile(&values, 95.0), 39);
  }

  #[test]
  fn duration_stats_handles_empty_input() {
    let stats = duration_stats(&[]);
    assert_eq!(stats.count, 0);
    assert_eq!(stats.max_ms, 0);
  }

  #[test]
  fn summarize_rolls_up_expected_metrics() {
    let records = vec![
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 1,
        event: MetricsLogEvent::TurnCompleted {
          turn_id: 1,
          mode: TranscriptMode::Normal,
          transcription_duration_ms: 120,
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 2,
        event: MetricsLogEvent::TurnCompleted {
          turn_id: 2,
          mode: TranscriptMode::Raw,
          transcription_duration_ms: 80,
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 3,
        event: MetricsLogEvent::PasteCompleted {
          turn_id: 1,
          mode: TranscriptMode::Normal,
          paste_duration_ms: 14,
          result: "pasted".to_string(),
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 6,
        event: MetricsLogEvent::PartialAuditResult {
          turn_id: 1,
          emitted_kind: "assembled".to_string(),
          exact: true,
          partial_count: 2,
          emitted_tokens: 5,
          full_tokens: 5,
          edit_distance: 0,
          wer_like: 0.0,
          lcp_tokens: 5,
          lcp_pct: 100.0,
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 7,
        event: MetricsLogEvent::TurnSnapshot {
          turn_id: 1,
          mode: TranscriptMode::Normal,
          transcription_duration_ms: 120,
          text_preview: "hello world".to_string(),
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 8,
        event: MetricsLogEvent::TurnSnapshot {
          turn_id: 2,
          mode: TranscriptMode::Raw,
          transcription_duration_ms: 80,
          text_preview: "raw paste example".to_string(),
        },
      },
    ];

    let summary = summarize(&records);
    assert_eq!(summary.total_transcriptions, 2);
    assert_eq!(summary.raw_transcriptions, 1);
    assert_eq!(summary.normal_transcriptions, 1);
    assert_eq!(summary.quality_samples, 1);
    assert_eq!(summary.quality_exact_rate_pct, 100.0);
    assert_eq!(summary.recent_transcripts.len(), 2);
    assert_eq!(summary.recent_transcripts[0].turn_id, 2);
    assert_eq!(summary.recent_transcripts[0].transcription_duration_ms, 80);
    assert_eq!(summary.recent_transcripts[1].turn_id, 1);
    assert_eq!(summary.recent_transcripts[1].refined_fixes, Some(0));
  }

  #[test]
  fn summarize_counts_reused_turn_ids_across_sessions() {
    let records = vec![
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 10,
        event: MetricsLogEvent::TurnCompleted {
          turn_id: 3,
          mode: TranscriptMode::Normal,
          transcription_duration_ms: 120,
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 20,
        event: MetricsLogEvent::TurnCompleted {
          turn_id: 3,
          mode: TranscriptMode::Raw,
          transcription_duration_ms: 80,
        },
      },
    ];

    let summary = summarize(&records);
    assert_eq!(summary.total_transcriptions, 2);
    assert_eq!(summary.normal_transcriptions, 1);
    assert_eq!(summary.raw_transcriptions, 1);
  }

  #[test]
  fn summarize_recent_quality_uses_nearest_turn_instance() {
    let records = vec![
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 10,
        event: MetricsLogEvent::TurnSnapshot {
          turn_id: 18,
          mode: TranscriptMode::Normal,
          transcription_duration_ms: 900,
          text_preview: "older turn".to_string(),
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 12,
        event: MetricsLogEvent::PartialAuditResult {
          turn_id: 18,
          emitted_kind: "assembled".to_string(),
          exact: true,
          partial_count: 1,
          emitted_tokens: 5,
          full_tokens: 5,
          edit_distance: 0,
          wer_like: 0.0,
          lcp_tokens: 5,
          lcp_pct: 100.0,
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 1000,
        event: MetricsLogEvent::TurnSnapshot {
          turn_id: 18,
          mode: TranscriptMode::Normal,
          transcription_duration_ms: 1500,
          text_preview: "newer turn".to_string(),
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 1025,
        event: MetricsLogEvent::PartialAuditResult {
          turn_id: 18,
          emitted_kind: "assembled".to_string(),
          exact: false,
          partial_count: 4,
          emitted_tokens: 12,
          full_tokens: 16,
          edit_distance: 4,
          wer_like: 0.25,
          lcp_tokens: 10,
          lcp_pct: 62.5,
        },
      },
    ];

    let summary = summarize(&records);
    assert_eq!(summary.recent_transcripts.len(), 2);
    assert_eq!(summary.recent_transcripts[0].text_preview, "newer turn");
    assert_eq!(summary.recent_transcripts[0].refined_fixes, Some(4));
    assert_eq!(summary.recent_transcripts[1].text_preview, "older turn");
    assert_eq!(summary.recent_transcripts[1].refined_fixes, Some(0));
  }

  #[test]
  fn summarize_ignores_stale_quality_from_far_away_turn_instance() {
    let records = vec![
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 1,
        event: MetricsLogEvent::PartialAuditResult {
          turn_id: 44,
          emitted_kind: "assembled".to_string(),
          exact: true,
          partial_count: 2,
          emitted_tokens: 8,
          full_tokens: 8,
          edit_distance: 0,
          wer_like: 0.0,
          lcp_tokens: 8,
          lcp_pct: 100.0,
        },
      },
      MetricsLogRecord {
        schema_version: 1,
        ts_ms: 1_000_000,
        event: MetricsLogEvent::TurnSnapshot {
          turn_id: 44,
          mode: TranscriptMode::Normal,
          transcription_duration_ms: 1300,
          text_preview: "current session".to_string(),
        },
      },
    ];

    let summary = summarize(&records);
    assert_eq!(summary.recent_transcripts.len(), 1);
    assert_eq!(summary.recent_transcripts[0].refined_fixes, None);
  }
}
