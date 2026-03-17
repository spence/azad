use std::path::PathBuf;

#[allow(dead_code)]
pub struct ModelPackDef {
  pub id: &'static str,
  pub display_name: &'static str,
  pub description: &'static str,
  pub total_size_bytes: u64,
  pub files: &'static [ModelFileDef],
}

pub struct ModelFileDef {
  pub rel_path: &'static str,
  pub url: &'static str,
  pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackStatus {
  NotDownloaded,
  Downloading { progress_pct: u8 },
  Ready,
  Incomplete,
}

pub static PARAKEET_V1: ModelPackDef = ModelPackDef {
  id: "parakeet-v1",
  display_name: "Parakeet v1",
  description: "Silero VAD + Parakeet streaming/finalization ASR",
  total_size_bytes: 3_117_007_360,
  files: &[
    ModelFileDef {
      rel_path: "vad/ggml-silero-v6.2.0.bin",
      url: "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin",
      size_bytes: 906_240,
    },
    ModelFileDef {
      rel_path: "eou/encoder.onnx",
      url: "https://huggingface.co/altunenes/parakeet-rs/resolve/main/realtime_eou_120m-v1-onnx/encoder.onnx",
      size_bytes: 326_265_132,
    },
    ModelFileDef {
      rel_path: "eou/decoder_joint.onnx",
      url: "https://huggingface.co/altunenes/parakeet-rs/resolve/main/realtime_eou_120m-v1-onnx/decoder_joint.onnx",
      size_bytes: 176_803_572,
    },
    ModelFileDef {
      rel_path: "eou/tokenizer.json",
      url: "https://huggingface.co/altunenes/parakeet-rs/resolve/main/realtime_eou_120m-v1-onnx/tokenizer.json",
      size_bytes: 2_966_086,
    },
    ModelFileDef {
      rel_path: "tdt/encoder-model.onnx",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx",
      size_bytes: 464_540,
    },
    ModelFileDef {
      rel_path: "tdt/encoder-model.onnx.data",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx.data",
      size_bytes: 2_449_474_560,
    },
    ModelFileDef {
      rel_path: "tdt/decoder_joint-model.onnx",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.onnx",
      size_bytes: 160_105_852,
    },
    ModelFileDef {
      rel_path: "tdt/vocab.txt",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt",
      size_bytes: 21_378,
    },
  ],
};

pub static ALL_PACKS: &[&ModelPackDef] = &[&PARAKEET_V1];

pub fn default_pack() -> &'static ModelPackDef {
  &PARAKEET_V1
}

pub fn pack_by_id(id: &str) -> Option<&'static ModelPackDef> {
  ALL_PACKS.iter().find(|p| p.id == id).copied()
}

pub fn models_base_dir() -> PathBuf {
  let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
  let mut path = PathBuf::from(home);
  path.push("Library");
  path.push("Application Support");
  path.push("Azad");
  path.push("models");
  path
}

pub fn pack_dir(pack_id: &str) -> PathBuf {
  models_base_dir().join(pack_id)
}

pub fn check_pack_status(pack: &ModelPackDef) -> PackStatus {
  let dir = pack_dir(pack.id);
  if !dir.exists() {
    return PackStatus::NotDownloaded;
  }

  let mut found = 0;
  for file in pack.files {
    if dir.join(file.rel_path).exists() {
      found += 1;
    }
  }

  if found == pack.files.len() {
    PackStatus::Ready
  } else if found == 0 {
    PackStatus::NotDownloaded
  } else {
    PackStatus::Incomplete
  }
}

/// Returns (vad_model_path, eou_dir, tdt_dir) if the pack is ready.
pub fn pipeline_paths(pack: &ModelPackDef) -> Option<(PathBuf, PathBuf, PathBuf)> {
  let dir = pack_dir(pack.id);
  let vad = dir.join("vad").join("ggml-silero-v6.2.0.bin");
  let eou = dir.join("eou");
  let tdt = dir.join("tdt");
  if vad.exists() && eou.exists() && tdt.exists() { Some((vad, eou, tdt)) } else { None }
}

pub fn format_size(bytes: u64) -> String {
  if bytes >= 1_000_000_000 {
    format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
  } else if bytes >= 1_000_000 {
    format!("{:.0} MB", bytes as f64 / 1_000_000.0)
  } else {
    format!("{:.0} KB", bytes as f64 / 1_000.0)
  }
}
