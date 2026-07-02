use std::path::PathBuf;

use asr::embed::SessionConfig;
use asr::pipeline::{PipelineConfig, StreamingModelConfig};

use crate::models;

#[derive(Debug, Clone)]
pub struct AzadConfig {
  pub show_overlay_on_vad_start: bool,
  pub final_pass_timeout_ms: u64,
  pub chunk_ms: u32,
  pub buffer_ms: u32,
  pub paste_delay_ms: u64,
  pub native_engine_logs_enabled: bool,
  pub pipeline: PipelineConfig,
}

impl AzadConfig {
  pub fn to_session_config(
    &self,
    device_id: Option<String>,
    auto_vad_enabled: bool,
    capture_enabled: bool,
    debug_stats_enabled: bool,
  ) -> SessionConfig {
    SessionConfig {
      device_id,
      chunk_ms: self.chunk_ms,
      buffer_ms: self.buffer_ms,
      auto_vad_enabled,
      capture_enabled,
      debug_stats_enabled,
      native_engine_logs_enabled: self.native_engine_logs_enabled,
      pipeline: self.pipeline.clone(),
    }
  }

  pub fn rebuild_pipeline_paths(&mut self, pack: &models::ModelPackDef) {
    if let Some(paths) = resolve_pipeline_paths(pack) {
      apply_pipeline_paths(&mut self.pipeline, paths);
    }
  }
}

impl Default for AzadConfig {
  fn default() -> Self {
    let default_pack = models::default_pack();
    let fallback_dir = models::pack_dir(default_pack.id)
      .unwrap_or_else(|| PathBuf::from("/nonexistent/azad/models"));
    let vad_path = fallback_dir.join("vad").join("silero_vad.mlmodelc");
    let fallback_streaming_model = StreamingModelConfig::MlxNemotron {
      model_dir: fallback_dir.join("mlx"),
      language: "en-US".to_string(),
      streaming_chunk_ms: 80,
      final_chunk_ms: 560,
      helper_path: None,
    };

    let mut cfg = Self {
      show_overlay_on_vad_start: true,
      final_pass_timeout_ms: 3_000,
      chunk_ms: 20,
      buffer_ms: 120_000,
      // Was 120 ms — originally a "let the focused app see the new pasteboard" buffer, but
      // NSPasteboard::writeObjects: is synchronous (the write is visible before the call
      // returns) and Screen Sharing clients are handled separately by
      // `nudge_screen_sharing_clipboard_sync`, so on local paste ~20 ms is plenty.
      paste_delay_ms: 20,
      native_engine_logs_enabled: env_flag_enabled("AZAD_NATIVE_ENGINE_LOGS"),
      pipeline: PipelineConfig {
        vad_model_path: vad_path,
        vad_helper_path: None,
        streaming_model: fallback_streaming_model,
        // Lowered from 0.45 → 0.30 to detect softer speech-starts faster.
        // Trade-off: more permissive turn-start means a quiet false-positive
        // (typing, breath, lip-smack) can spawn a turn — but the engine
        // immediately hands such turns to the empty-draft path which paste-
        // suppresses on `cleaned.is_empty()`, so the user-visible cost is
        // an overlay flicker rather than a phantom paste. Companion changes:
        // pre_roll_ms widened to 1500 below so the first slow-attack syllable
        // isn't lost when start-detection fires late.
        vad_thold: 0.30,
        vad_start_chunks: 1,
        // Widened from 800 → 1500 ms. With the lower vad_thold = 0.30 above
        // we accept slightly slower first-detection, but the larger pre-roll
        // means even when detection lags by a full second the engine still
        // recovers the user's first 1.5 s of audio. Memory cost is trivial
        // (1.5 s @ 16 kHz mono f32 ≈ 96 kB / circular buffer).
        pre_roll_ms: 1500,
        // Was 240 ms. 240 ms of VAD-classified silence is easily reached during natural
        // connected speech — micro-pauses at word boundaries, consonant-heavy segments, or
        // brief Silero probability dips. Combined with EOU latching on an intermediate
        // clause boundary, users hit "cut off mid-word" false finalizations. 350 ms adds
        // ~110 ms of finalize latency when you actually stop talking (invisible under the
        // paste spinner) but meaningfully widens the window against false VAD-silence cuts.
        eou_min_silence_ms: 350,
        eou_max_silence_ms: 1_000,
        // VAD probability floor while a turn is in progress. Sub-floor chunks
        // accumulate against `eou_max_silence_ms`. Was implicitly 0.30 (derived
        // from `vad_thold - 0.15`) for years. Production turn 252 (2026-05-01)
        // showed sustained vad_prob 0.01-0.24 during continuous user speech —
        // sub-0.30 — so the engine misread soft continuation as silence and
        // force-ended mid-clause. 0.10 keeps any non-trivial voice activity
        // above the floor while staying above typical mic / room noise floor
        // (< 0.05 in tests). Starting a turn still requires the higher
        // `vad_thold` confidence; only the in-speech floor is permissive.
        vad_in_speech_thold: 0.10,
        // Tentative-finalize: after EOU latches and `eou_min_silence_ms` is met,
        // wait this long before actually committing. If VAD picks up speech AND
        // EOU produces meaningful text inside the window, the latch is undone
        // and the turn continues. Targets the "Silero-misclassified-soft-attack
        // cuts the user off mid-word" failure mode — see turn-000100 in the
        // debug-recording buffer for the canonical example.
        //
        // Was 500 ms. In live use the recovery branch effectively never fires
        // (Silero is reliable enough on continuation speech that the strong-
        // recovery path almost always handles it). Pulling the window to 250 ms
        // halves the paste-latency cost on every turn while still catching
        // soft-attack resumes that land within ~half the window.
        recovery_window_ms: 250,
        // Lower than the turn-start `vad_thold`. False-positive recovery
        // only costs latency; false-negative cuts the user off. Keep generous.
        recovery_vad_thold: 0.30,
        stable_k: 3,
        stable_h: 5,
        finalizing_pulse_enabled: true,
        incremental_finalization_enabled: true,
        incremental_slice_ms: 6_000,
        incremental_overlap_ms: 3_000,
        incremental_left_context_ms: 10_000,
        incremental_min_new_audio_ms: 1_200,
        incremental_wait_tail_result_ms: 220,
      },
    };
    if let Some(paths) = resolve_pipeline_paths(default_pack) {
      apply_pipeline_paths(&mut cfg.pipeline, paths);
    }
    cfg
  }
}

/// In debug builds, check workspace-root paths first for dev convenience.
/// In release builds (or when workspace paths don't exist), use user-local storage.
fn resolve_pipeline_paths(pack: &models::ModelPackDef) -> Option<models::ResolvedPipelinePaths> {
  #[cfg(debug_assertions)]
  {
    let root = workspace_root();
    let dev_vad = default_vad_model_path(&root);
    let dev_model_dir = root.join("models").join("nemotron-mlx");
    if models::coreml_vad_model_ready(&dev_vad)
      && models::mlx_nemotron_model_dir_ready(&dev_model_dir)
    {
      return Some(models::ResolvedPipelinePaths {
        vad_model_path: dev_vad,
        backend: models::ResolvedModelBackend::MlxNemotron { model_dir: dev_model_dir },
      });
    }
  }

  models::pipeline_paths(pack)
}

fn apply_pipeline_paths(pipeline: &mut PipelineConfig, paths: models::ResolvedPipelinePaths) {
  pipeline.vad_model_path = paths.vad_model_path;
  match paths.backend {
    models::ResolvedModelBackend::MlxNemotron { model_dir } => {
      pipeline.streaming_model = StreamingModelConfig::MlxNemotron {
        model_dir,
        language: "en-US".to_string(),
        streaming_chunk_ms: 80,
        final_chunk_ms: 560,
        helper_path: None,
      };
      pipeline.incremental_finalization_enabled = true;
    }
  }
}

fn env_flag_enabled(key: &str) -> bool {
  std::env::var(key)
    .ok()
    .map(|raw| raw.trim().to_ascii_lowercase())
    .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

#[cfg(debug_assertions)]
fn workspace_root() -> PathBuf {
  std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .parent()
    .and_then(std::path::Path::parent)
    .unwrap_or_else(|| std::path::Path::new("."))
    .to_path_buf()
}

#[cfg(debug_assertions)]
fn default_vad_model_path(root: &std::path::Path) -> PathBuf {
  root.join("models").join("vad").join("silero_vad.mlmodelc")
}
