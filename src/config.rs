use std::path::{Path, PathBuf};

use toon::embed::SessionConfig;
use toon::pipeline::PipelineConfig;

#[derive(Debug, Clone)]
pub struct AzadConfig {
    pub show_overlay_on_vad_start: bool,
    pub final_pass_timeout_ms: u64,
    pub chunk_ms: u32,
    pub buffer_ms: u32,
    pub pipeline: PipelineConfig,
}

impl AzadConfig {
    pub fn to_session_config(&self, device_id: Option<String>) -> SessionConfig {
        SessionConfig {
            device_id,
            chunk_ms: self.chunk_ms,
            buffer_ms: self.buffer_ms,
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
                parakeet_tdt_dir: root.join("models").join("parakeet").join("tdt"),
                parakeet_eou_dir: root.join("models").join("parakeet").join("eou"),
            },
        }
    }
}

fn workspace_root() -> PathBuf {
    // <workspace>/azad/azad -> <workspace>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}
