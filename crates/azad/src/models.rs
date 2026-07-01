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
  pub sha256: &'static str,
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
  total_size_bytes: 3_031_399_977,
  files: &[
    ModelFileDef {
      rel_path: "vad/ggml-silero-v6.2.0.bin",
      url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin",
      size_bytes: 885_098,
      sha256: "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987",
    },
    ModelFileDef {
      rel_path: "eou/encoder.onnx",
      url: "https://huggingface.co/altunenes/parakeet-rs/resolve/a61d2818df4659c956b9661a9447f46e98c15126/realtime_eou_120m-v1-onnx/encoder.onnx",
      size_bytes: 459_341_289,
      sha256: "d472887cc38a784a5bfc21c2dbe247639edc3b3f9992388d8ceceaec07256b5b",
    },
    ModelFileDef {
      rel_path: "eou/decoder_joint.onnx",
      url: "https://huggingface.co/altunenes/parakeet-rs/resolve/a61d2818df4659c956b9661a9447f46e98c15126/realtime_eou_120m-v1-onnx/decoder_joint.onnx",
      size_bytes: 21_347_639,
      sha256: "9d2553ac043c2fc5f69e970769b0fb8ab9103fbfdeb7d26a1ea9729d4bd2dddd",
    },
    ModelFileDef {
      rel_path: "eou/tokenizer.json",
      url: "https://huggingface.co/altunenes/parakeet-rs/resolve/a61d2818df4659c956b9661a9447f46e98c15126/realtime_eou_120m-v1-onnx/tokenizer.json",
      size_bytes: 20_053,
      sha256: "f6b0ad8690559351fa478116fe0985a203b76f7c040f3a9381f485c99c0325f8",
    },
    ModelFileDef {
      rel_path: "tdt/encoder-model.onnx",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/encoder-model.onnx",
      size_bytes: 41_770_866,
      sha256: "98a74b21b4cc0017c1e7030319a4a96f4a9506e50f0708f3a516d02a77c96bb1",
    },
    ModelFileDef {
      rel_path: "tdt/encoder-model.onnx.data",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/encoder-model.onnx.data",
      size_bytes: 2_435_420_160,
      sha256: "9a22d372c51455c34f13405da2520baefb7125bd16981397561423ed32d24f36",
    },
    ModelFileDef {
      rel_path: "tdt/decoder_joint-model.onnx",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/decoder_joint-model.onnx",
      size_bytes: 72_520_893,
      sha256: "e978ddf6688527182c10fde2eb4b83068421648985ef23f7a86be732be8706c1",
    },
    ModelFileDef {
      rel_path: "tdt/vocab.txt",
      url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce/vocab.txt",
      size_bytes: 93_939,
      sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
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

pub fn models_base_dir() -> Option<PathBuf> {
  let home = std::env::var_os("HOME")?;
  let mut path = PathBuf::from(home);
  path.push("Library");
  path.push("Application Support");
  path.push("Azad");
  path.push("models");
  Some(path)
}

pub fn pack_dir(pack_id: &str) -> Option<PathBuf> {
  Some(models_base_dir()?.join(pack_id))
}

pub fn check_pack_status(pack: &ModelPackDef) -> PackStatus {
  let dir = match pack_dir(pack.id) {
    Some(d) => d,
    None => return PackStatus::NotDownloaded,
  };
  if !dir.exists() {
    return PackStatus::NotDownloaded;
  }

  let mut found = 0;
  for file in pack.files {
    let path = dir.join(file.rel_path);
    let ok = path.metadata().map(|m| m.len() == file.size_bytes).unwrap_or(false);
    if ok {
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
  let dir = pack_dir(pack.id)?;
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
