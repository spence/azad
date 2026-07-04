use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub struct ModelPackDef {
  pub id: &'static str,
  pub display_name: &'static str,
  pub description: &'static str,
  pub page_url: &'static str,
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
  Resumable { bytes_done: u64, progress_pct: u8 },
  Incomplete,
}

pub static NEMOTRON_35_MLX_BF16_V1: ModelPackDef = ModelPackDef {
  id: "nemotron-3.5-mlx-bf16-v1",
  display_name: "Nemotron 3.5 MLX bf16",
  description: "CoreML Silero VAD + MLX Nemotron 3.5 streaming/final ASR",
  page_url: "https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b",
  total_size_bytes: 1_277_344_990,
  files: &[
    ModelFileDef {
      rel_path: "vad/config.json",
      url: "https://huggingface.co/aufklarer/Silero-VAD-v6.2.1-CoreML/resolve/523876545a57961474fee9df913e833e130560b8/config.json",
      size_bytes: 888,
      sha256: "459e764d58cdc13f3db6878adfdf8a29b5fd467ad1f4ef2161137cc115339c81",
    },
    ModelFileDef {
      rel_path: "vad/silero_vad.mlmodelc/analytics/coremldata.bin",
      url: "https://huggingface.co/aufklarer/Silero-VAD-v6.2.1-CoreML/resolve/523876545a57961474fee9df913e833e130560b8/silero_vad.mlmodelc/analytics/coremldata.bin",
      size_bytes: 243,
      sha256: "b777c3751d72b7430eac7f8544769a3d918faf77c15db184fec30e44c56007a3",
    },
    ModelFileDef {
      rel_path: "vad/silero_vad.mlmodelc/coremldata.bin",
      url: "https://huggingface.co/aufklarer/Silero-VAD-v6.2.1-CoreML/resolve/523876545a57961474fee9df913e833e130560b8/silero_vad.mlmodelc/coremldata.bin",
      size_bytes: 399,
      sha256: "f6fcd92c3132c9c718e5f54e0e770a8c8075beaa50a5b212a6287273b4ddae67",
    },
    ModelFileDef {
      rel_path: "vad/silero_vad.mlmodelc/metadata.json",
      url: "https://huggingface.co/aufklarer/Silero-VAD-v6.2.1-CoreML/resolve/523876545a57961474fee9df913e833e130560b8/silero_vad.mlmodelc/metadata.json",
      size_bytes: 3_005,
      sha256: "1b953eb3818e7092deedd96e976c05354f77beb2ddc2976fe416af17e47f62d2",
    },
    ModelFileDef {
      rel_path: "vad/silero_vad.mlmodelc/model.mil",
      url: "https://huggingface.co/aufklarer/Silero-VAD-v6.2.1-CoreML/resolve/523876545a57961474fee9df913e833e130560b8/silero_vad.mlmodelc/model.mil",
      size_bytes: 18_203,
      sha256: "b0a1384c4a664697989d9eb9cfb166b4b85f151206aeefd1bfa391ef9e5ad08f",
    },
    ModelFileDef {
      rel_path: "vad/silero_vad.mlmodelc/weights/weight.bin",
      url: "https://huggingface.co/aufklarer/Silero-VAD-v6.2.1-CoreML/resolve/523876545a57961474fee9df913e833e130560b8/silero_vad.mlmodelc/weights/weight.bin",
      size_bytes: 619_136,
      sha256: "83210545de90c65195e8d6db1b349b7e5c31f989f48d0a908a8dc0e2f586e5f9",
    },
    ModelFileDef {
      rel_path: "mlx/config.json",
      url: "https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b/resolve/e550040c0478027ed679b2b6b0d055502c103663/config.json",
      size_bytes: 159_432,
      sha256: "97fe51f0970514e6cac928bcaebac4dbb1dba554f980642542ffac451a0dca56",
    },
    ModelFileDef {
      rel_path: "mlx/model.safetensors",
      url: "https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b/resolve/e550040c0478027ed679b2b6b0d055502c103663/model.safetensors",
      size_bytes: 1_276_058_836,
      sha256: "1b78e4551371b1438daba0e8c9e1673bb18606994c1bcc493d85c5454d428ee5",
    },
    ModelFileDef {
      rel_path: "mlx/tokenizer.model",
      url: "https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b/resolve/e550040c0478027ed679b2b6b0d055502c103663/tokenizer.model",
      size_bytes: 406_554,
      sha256: "ce3895e40806f02a26c3a225161b96ef682d6c0054bae32a245dec4258d7d291",
    },
    ModelFileDef {
      rel_path: "mlx/vocab.txt",
      url: "https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b/resolve/e550040c0478027ed679b2b6b0d055502c103663/vocab.txt",
      size_bytes: 78_294,
      sha256: "d74b60edd1cad792cfce25dcb7e1048d78d717cf4f29acaae2854262d5189f4f",
    },
  ],
};

pub static ALL_PACKS: &[&ModelPackDef] = &[&NEMOTRON_35_MLX_BF16_V1];

pub fn default_pack() -> &'static ModelPackDef {
  &NEMOTRON_35_MLX_BF16_V1
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
  let progress = pack_download_progress(pack);
  if progress.ready_files == pack.files.len() {
    PackStatus::Ready
  } else if progress.bytes_done > 0 {
    PackStatus::Resumable {
      bytes_done: progress.bytes_done,
      progress_pct: progress.progress_pct(pack.total_size_bytes),
    }
  } else if progress.has_install_artifacts {
    PackStatus::Incomplete
  } else {
    PackStatus::NotDownloaded
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackDownloadProgress {
  pub bytes_done: u64,
  pub ready_files: usize,
  pub has_install_artifacts: bool,
}

impl PackDownloadProgress {
  pub fn progress_pct(self, total_size_bytes: u64) -> u8 {
    if total_size_bytes == 0 {
      0
    } else {
      ((self.bytes_done as f64 / total_size_bytes as f64) * 100.0).clamp(0.0, 100.0) as u8
    }
  }
}

pub fn pack_download_progress(pack: &ModelPackDef) -> PackDownloadProgress {
  let dir = match pack_dir(pack.id) {
    Some(d) => d,
    None => {
      return PackDownloadProgress { bytes_done: 0, ready_files: 0, has_install_artifacts: false };
    }
  };
  if !dir.exists() {
    return PackDownloadProgress { bytes_done: 0, ready_files: 0, has_install_artifacts: false };
  }

  pack_download_progress_in_dir(pack, &dir)
}

fn pack_download_progress_in_dir(pack: &ModelPackDef, dir: &Path) -> PackDownloadProgress {
  let mut ready_files = 0;
  let mut bytes_done = 0;
  let mut has_install_artifacts = false;
  for file in pack.files {
    let path = dir.join(file.rel_path);
    match path.metadata() {
      Ok(meta) if meta.len() == file.size_bytes => {
        ready_files += 1;
        bytes_done += file.size_bytes;
        has_install_artifacts = true;
        continue;
      }
      Ok(_) => {
        has_install_artifacts = true;
      }
      Err(_) => {}
    }

    let part_path = PathBuf::from(format!("{}.part", path.display()));
    match part_path.metadata() {
      Ok(meta) if meta.len() > 0 && meta.len() <= file.size_bytes => {
        bytes_done += meta.len();
        has_install_artifacts = true;
      }
      Ok(_) => {
        has_install_artifacts = true;
      }
      Err(_) => {}
    }
  }

  PackDownloadProgress { bytes_done, ready_files, has_install_artifacts }
}

pub struct ResolvedPipelinePaths {
  pub vad_model_path: PathBuf,
  pub backend: ResolvedModelBackend,
}

pub enum ResolvedModelBackend {
  MlxNemotron { model_dir: PathBuf },
}

/// Returns pipeline model paths if the pack has the required runtime directories.
pub fn pipeline_paths(pack: &ModelPackDef) -> Option<ResolvedPipelinePaths> {
  let dir = pack_dir(pack.id)?;
  let vad = dir.join("vad").join("silero_vad.mlmodelc");
  let model_dir = dir.join("mlx");
  if coreml_vad_model_ready(&vad) && mlx_nemotron_model_dir_ready(&model_dir) {
    Some(ResolvedPipelinePaths {
      vad_model_path: vad,
      backend: ResolvedModelBackend::MlxNemotron { model_dir },
    })
  } else {
    None
  }
}

pub fn mlx_nemotron_model_dir_ready(model_dir: &std::path::Path) -> bool {
  ["config.json", "model.safetensors", "tokenizer.model", "vocab.txt"]
    .iter()
    .all(|file| model_dir.join(file).is_file())
}

pub fn coreml_vad_model_ready(model_dir: &std::path::Path) -> bool {
  ["analytics/coremldata.bin", "coremldata.bin", "metadata.json", "model.mil", "weights/weight.bin"]
    .iter()
    .all(|file| model_dir.join(file).is_file())
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

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;
  use std::time::{SystemTime, UNIX_EPOCH};

  #[test]
  fn default_pack_is_bf16_mlx_nemotron() {
    let pack = default_pack();
    assert_eq!(pack.id, "nemotron-3.5-mlx-bf16-v1");
    assert!(pack.files.iter().any(|f| f.rel_path == "mlx/model.safetensors"));
    assert!(pack.files.iter().any(|f| f.rel_path == "vad/silero_vad.mlmodelc/model.mil"));
    assert!(!pack.files.iter().any(|f| f.rel_path.ends_with(".onnx")));
  }

  #[test]
  fn app_download_packs_only_include_mlx_nemotron() {
    assert_eq!(ALL_PACKS.len(), 1);
    assert_eq!(ALL_PACKS[0].id, default_pack().id);
    assert!(pack_by_id("legacy-pack").is_none());
  }

  #[test]
  fn mlx_nemotron_model_dir_requires_all_runtime_files() {
    let dir = std::env::temp_dir().join(format!(
      "azad-mlx-model-test-{}",
      SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();

    for file in ["config.json", "model.safetensors", "tokenizer.model"] {
      fs::write(dir.join(file), b"x").unwrap();
    }
    assert!(!mlx_nemotron_model_dir_ready(&dir));

    fs::write(dir.join("vocab.txt"), b"x").unwrap();
    assert!(mlx_nemotron_model_dir_ready(&dir));

    let _ = fs::remove_dir_all(dir);
  }

  #[test]
  fn coreml_vad_model_dir_requires_compiled_model_files() {
    let dir = std::env::temp_dir().join(format!(
      "azad-coreml-vad-model-test-{}",
      SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir_all(dir.join("analytics")).unwrap();
    fs::create_dir_all(dir.join("weights")).unwrap();

    for file in ["analytics/coremldata.bin", "coremldata.bin", "metadata.json", "model.mil"] {
      fs::write(dir.join(file), b"x").unwrap();
    }
    assert!(!coreml_vad_model_ready(&dir));

    fs::write(dir.join("weights").join("weight.bin"), b"x").unwrap();
    assert!(coreml_vad_model_ready(&dir));

    let _ = fs::remove_dir_all(dir);
  }

  #[test]
  fn pack_progress_counts_resumable_part_file() {
    let dir = temp_model_dir("azad-pack-progress-part");
    let file = default_pack()
      .files
      .iter()
      .find(|file| file.rel_path == "mlx/model.safetensors")
      .unwrap();
    let target = dir.join(file.rel_path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(PathBuf::from(format!("{}.part", target.display())), vec![7u8; 4096]).unwrap();

    let progress = pack_download_progress_in_dir(default_pack(), &dir);

    assert_eq!(progress.bytes_done, 4096);
    assert_eq!(progress.ready_files, 0);
    assert!(progress.has_install_artifacts);

    let _ = fs::remove_dir_all(dir);
  }

  #[test]
  fn pack_status_reports_resumable_progress() {
    let dir = temp_model_dir("azad-pack-progress-complete");
    let pack = default_pack();
    let file = &pack.files[0];
    let target = dir.join(file.rel_path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, vec![1u8; file.size_bytes as usize]).unwrap();

    let progress = pack_download_progress_in_dir(pack, &dir);
    let status = if progress.ready_files == pack.files.len() {
      PackStatus::Ready
    } else if progress.bytes_done > 0 {
      PackStatus::Resumable {
        bytes_done: progress.bytes_done,
        progress_pct: progress.progress_pct(pack.total_size_bytes),
      }
    } else if progress.has_install_artifacts {
      PackStatus::Incomplete
    } else {
      PackStatus::NotDownloaded
    };

    assert!(
      matches!(status, PackStatus::Resumable { bytes_done, .. } if bytes_done == file.size_bytes)
    );

    let _ = fs::remove_dir_all(dir);
  }

  fn temp_model_dir(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
      "{}-{}",
      prefix,
      SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ))
  }
}
