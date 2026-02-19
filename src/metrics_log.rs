use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const METRICS_LOG_SCHEMA_VERSION: u8 = 1;
const LAST_24_HOURS_MS: i64 = 24 * 60 * 60 * 1000;

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
        Self {
            schema_version: METRICS_LOG_SCHEMA_VERSION,
            ts_ms: now_epoch_ms(),
            event,
        }
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
    PartialFinalizeOutcome {
        turn_id: u64,
        outcome: String,
        reason: String,
    },
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
    PartialAuditError {
        turn_id: u64,
        emitted_kind: String,
        partial_count: usize,
        message: String,
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
    pub fallback_attempts: usize,
    pub fallback_count: usize,
    pub fallback_rate_pct: f64,
    pub quality_samples: usize,
    pub quality_exact_rate_pct: f64,
    pub quality_avg_edit_distance: f64,
    pub quality_avg_wer_like: f64,
    pub quality_avg_lcp_pct: f64,
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
    lines.push(format!(
        "Transcriptions: total={} raw={} normal={}",
        summary.total_transcriptions, summary.raw_transcriptions, summary.normal_transcriptions
    ));
    lines.push(format!(
        "Transcription latency (ms, speech->paste): all {} | raw {} | normal {}",
        format_duration_stats(&summary.transcription_all),
        format_duration_stats(&summary.transcription_raw),
        format_duration_stats(&summary.transcription_normal)
    ));
    lines.push(format!(
        "Paste latency (ms): {}",
        format_duration_stats(&summary.paste)
    ));
    lines.push(format!(
        "Finalize fallback: {} / {} ({:.1}%)",
        summary.fallback_count, summary.fallback_attempts, summary.fallback_rate_pct
    ));
    if summary.quality_samples == 0 {
        lines.push("Quality: no partial-audit samples in last 24h".to_string());
    } else {
        lines.push(format!(
            "Quality: samples={} exact={:.1}% avg_edit={:.2} avg_wer_like={:.3} avg_lcp={:.1}%",
            summary.quality_samples,
            summary.quality_exact_rate_pct,
            summary.quality_avg_edit_distance,
            summary.quality_avg_wer_like,
            summary.quality_avg_lcp_pct
        ));
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
    let mut turns: HashMap<u64, (i64, TranscriptMode, u64)> = HashMap::new();
    let mut pastes: HashMap<u64, (i64, TranscriptMode, u64)> = HashMap::new();
    let mut outcomes: HashMap<u64, (i64, String, String)> = HashMap::new();
    let mut audits: HashMap<u64, (i64, bool, usize, f64, f64)> = HashMap::new();

    for record in records {
        match &record.event {
            MetricsLogEvent::TurnCompleted {
                turn_id,
                mode,
                transcription_duration_ms,
            } => {
                upsert_latest(
                    &mut turns,
                    *turn_id,
                    (record.ts_ms, *mode, *transcription_duration_ms),
                );
            }
            MetricsLogEvent::PasteCompleted {
                turn_id,
                mode,
                paste_duration_ms,
                ..
            } => {
                upsert_latest(
                    &mut pastes,
                    *turn_id,
                    (record.ts_ms, *mode, *paste_duration_ms),
                );
            }
            MetricsLogEvent::PartialFinalizeOutcome {
                turn_id,
                outcome,
                reason,
            } => {
                upsert_latest(
                    &mut outcomes,
                    *turn_id,
                    (record.ts_ms, outcome.clone(), reason.clone()),
                );
            }
            MetricsLogEvent::PartialAuditResult {
                turn_id,
                exact,
                edit_distance,
                wer_like,
                lcp_pct,
                ..
            } => {
                upsert_latest(
                    &mut audits,
                    *turn_id,
                    (record.ts_ms, *exact, *edit_distance, *wer_like, *lcp_pct),
                );
            }
            MetricsLogEvent::PartialAuditError { .. } => {}
        }
    }

    let mut summary = MetricsSummary::default();

    let mut trans_all = Vec::new();
    let mut trans_raw = Vec::new();
    let mut trans_normal = Vec::new();
    for (_, mode, duration) in turns.values().copied() {
        summary.total_transcriptions += 1;
        trans_all.push(duration);
        match mode {
            TranscriptMode::Raw => {
                summary.raw_transcriptions += 1;
                trans_raw.push(duration);
            }
            TranscriptMode::Normal => {
                summary.normal_transcriptions += 1;
                trans_normal.push(duration);
            }
        }
    }
    summary.transcription_all = duration_stats(&trans_all);
    summary.transcription_raw = duration_stats(&trans_raw);
    summary.transcription_normal = duration_stats(&trans_normal);

    let mut paste_values = Vec::new();
    for (_, _, duration) in pastes.values().copied() {
        paste_values.push(duration);
    }
    summary.paste = duration_stats(&paste_values);

    for (_, outcome, _) in outcomes.values() {
        summary.fallback_attempts += 1;
        if outcome == "full_pass_bailout" {
            summary.fallback_count += 1;
        }
    }
    summary.fallback_rate_pct = if summary.fallback_attempts == 0 {
        0.0
    } else {
        summary.fallback_count as f64 * 100.0 / summary.fallback_attempts as f64
    };

    let mut exact_count = 0usize;
    let mut sum_edit = 0usize;
    let mut sum_wer = 0.0f64;
    let mut sum_lcp = 0.0f64;
    for (_, exact, edit_distance, wer_like, lcp_pct) in audits.values().copied() {
        summary.quality_samples += 1;
        if exact {
            exact_count += 1;
        }
        sum_edit += edit_distance;
        sum_wer += wer_like;
        sum_lcp += lcp_pct;
    }

    if summary.quality_samples > 0 {
        let denom = summary.quality_samples as f64;
        summary.quality_exact_rate_pct = exact_count as f64 * 100.0 / denom;
        summary.quality_avg_edit_distance = sum_edit as f64 / denom;
        summary.quality_avg_wer_like = sum_wer / denom;
        summary.quality_avg_lcp_pct = sum_lcp / denom;
    }

    summary
}

fn upsert_latest<V>(map: &mut HashMap<u64, V>, key: u64, value: V)
where
    V: Clone + HasTsMs,
{
    let should_replace = map
        .get(&key)
        .map(|existing| value.ts_ms() >= existing.ts_ms())
        .unwrap_or(true);
    if should_replace {
        map.insert(key, value);
    }
}

trait HasTsMs {
    fn ts_ms(&self) -> i64;
}

impl HasTsMs for (i64, TranscriptMode, u64) {
    fn ts_ms(&self) -> i64 {
        self.0
    }
}

impl HasTsMs for (i64, String, String) {
    fn ts_ms(&self) -> i64 {
        self.0
    }
}

impl HasTsMs for (i64, bool, usize, f64, f64) {
    fn ts_ms(&self) -> i64 {
        self.0
    }
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

fn format_duration_stats(stats: &DurationStats) -> String {
    if stats.count == 0 {
        return "n=0".to_string();
    }
    format!(
        "n={} avg={:.1} p50={} p95={} max={}",
        stats.count, stats.avg_ms, stats.p50_ms, stats.p95_ms, stats.max_ms
    )
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
        MetricsLogEvent, MetricsLogRecord, TranscriptMode, duration_stats, percentile, summarize,
    };

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
                ts_ms: 4,
                event: MetricsLogEvent::PartialFinalizeOutcome {
                    turn_id: 1,
                    outcome: "assembled".to_string(),
                    reason: "na".to_string(),
                },
            },
            MetricsLogRecord {
                schema_version: 1,
                ts_ms: 5,
                event: MetricsLogEvent::PartialFinalizeOutcome {
                    turn_id: 2,
                    outcome: "full_pass_bailout".to_string(),
                    reason: "tail_timeout".to_string(),
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
        ];

        let summary = summarize(&records);
        assert_eq!(summary.total_transcriptions, 2);
        assert_eq!(summary.raw_transcriptions, 1);
        assert_eq!(summary.normal_transcriptions, 1);
        assert_eq!(summary.fallback_attempts, 2);
        assert_eq!(summary.fallback_count, 1);
        assert_eq!(summary.quality_samples, 1);
        assert_eq!(summary.quality_exact_rate_pct, 100.0);
    }
}
