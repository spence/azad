use std::path::PathBuf;

use asr::embed::SessionConfig;
use asr::pipeline::PipelineConfig;

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
    if let Some((vad, eou, tdt)) = resolve_pipeline_paths(pack) {
      self.pipeline.vad_model_path = vad;
      self.pipeline.parakeet_eou_dir = eou;
      self.pipeline.parakeet_tdt_dir = tdt;
    }
  }
}

impl Default for AzadConfig {
  fn default() -> Self {
    let fallback_dir = models::pack_dir(models::default_pack().id)
      .unwrap_or_else(|| PathBuf::from("/nonexistent/azad/models"));
    let (vad_path, eou_dir, tdt_dir) = resolve_pipeline_paths(models::default_pack())
      .unwrap_or_else(|| {
        (
          fallback_dir.join("vad").join("ggml-silero-v6.2.0.bin"),
          fallback_dir.join("eou"),
          fallback_dir.join("tdt"),
        )
      });

    Self {
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
        vad_thold: 0.45,
        vad_start_chunks: 1,
        pre_roll_ms: 800,
        // Was 240 ms. 240 ms of VAD-classified silence is easily reached during natural
        // connected speech — micro-pauses at word boundaries, consonant-heavy segments, or
        // brief Silero probability dips. Combined with EOU latching on an intermediate
        // clause boundary, users hit "cut off mid-word" false finalizations. 350 ms adds
        // ~110 ms of finalize latency when you actually stop talking (invisible under the
        // paste spinner) but meaningfully widens the window against false VAD-silence cuts.
        eou_min_silence_ms: 350,
        eou_max_silence_ms: 1_000,
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
        // Lower than the turn-start `vad_thold` (0.45). False-positive recovery
        // only costs latency; false-negative cuts the user off. Keep generous.
        recovery_vad_thold: 0.30,
        stable_k: 3,
        stable_h: 5,
        enable_tdt_final_pass: true,
        incremental_finalization_enabled: true,
        incremental_slice_ms: 6_000,
        incremental_overlap_ms: 3_000,
        incremental_left_context_ms: 10_000,
        incremental_min_new_audio_ms: 1_200,
        incremental_wait_tail_result_ms: 220,
        parakeet_tdt_dir: tdt_dir,
        parakeet_eou_dir: eou_dir,
      },
    }
  }
}

/// In debug builds, check workspace-root paths first for dev convenience.
/// In release builds (or when workspace paths don't exist), use user-local storage.
fn resolve_pipeline_paths(pack: &models::ModelPackDef) -> Option<(PathBuf, PathBuf, PathBuf)> {
  #[cfg(debug_assertions)]
  {
    let root = workspace_root();
    let dev_vad = default_vad_model_path(&root);
    let dev_eou = root.join("models").join("parakeet").join("eou");
    let dev_tdt = root.join("models").join("parakeet").join("tdt");
    if dev_vad.exists() && dev_eou.exists() && dev_tdt.exists() {
      return Some((dev_vad, dev_eou, dev_tdt));
    }
  }

  models::pipeline_paths(pack)
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
  let model_dir = root.join("crates").join("whisper.cpp").join("models");
  let primary = model_dir.join("ggml-silero-v6.2.0.bin");
  if primary.exists() { primary } else { model_dir.join("for-tests-silero-v6.2.0-ggml.bin") }
}
