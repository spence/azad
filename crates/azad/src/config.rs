use std::path::{Path, PathBuf};

use asr::embed::SessionConfig;
use asr::pipeline::PipelineConfig;

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
}

impl Default for AzadConfig {
    fn default() -> Self {
        let root = workspace_root();
        Self {
            show_overlay_on_vad_start: true,
            final_pass_timeout_ms: 3_000,
            chunk_ms: 20,
            buffer_ms: 120_000,
            paste_delay_ms: 120,
            native_engine_logs_enabled: env_flag_enabled("AZAD_NATIVE_ENGINE_LOGS"),
            pipeline: PipelineConfig {
                vad_model_path: root
                    .join("whisper.cpp")
                    .join("models")
                    .join("ggml-silero-v6.2.0.bin"),
                vad_thold: 0.45,
                vad_start_chunks: 1,
                pre_roll_ms: 800,
                eou_min_silence_ms: 240,
                eou_max_silence_ms: 1_000,
                stable_k: 3,
                stable_h: 5,
                enable_tdt_final_pass: true,
                incremental_finalization_enabled: true,
                incremental_slice_ms: 6_000,
                incremental_overlap_ms: 3_000,
                incremental_left_context_ms: 10_000,
                incremental_min_new_audio_ms: 1_200,
                incremental_wait_tail_result_ms: 220,
                parakeet_tdt_dir: root.join("models").join("parakeet").join("tdt"),
                parakeet_eou_dir: root.join("models").join("parakeet").join("eou"),
            },
        }
    }
}

fn env_flag_enabled(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|raw| raw.trim().to_ascii_lowercase())
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

fn workspace_root() -> PathBuf {
    // <workspace>/azad/azad -> <workspace>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}
