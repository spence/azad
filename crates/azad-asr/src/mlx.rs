use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Debug, Clone)]
pub struct MlxNemotronConfig {
  pub model_dir: PathBuf,
  pub language: String,
  pub streaming_chunk_ms: u32,
  pub final_chunk_ms: u32,
  pub helper_path: Option<PathBuf>,
}

pub struct MlxNemotronAsr {
  child: Child,
  stdin: ChildStdin,
  stdout: BufReader<ChildStdout>,
}

impl MlxNemotronAsr {
  pub fn load(cfg: &MlxNemotronConfig) -> Result<Self> {
    let helper = resolve_helper_path(cfg.helper_path.as_deref())?;
    let mut child = Command::new(&helper)
      .arg("--model-dir")
      .arg(&cfg.model_dir)
      .arg("--language")
      .arg(&cfg.language)
      .arg("--streaming-chunk-ms")
      .arg(cfg.streaming_chunk_ms.to_string())
      .arg("--final-chunk-ms")
      .arg(cfg.final_chunk_ms.to_string())
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::inherit())
      .spawn()
      .with_context(|| format!("failed to spawn MLX ASR helper at {}", helper.display()))?;

    let stdin = child.stdin.take().ok_or_else(|| anyhow!("MLX ASR helper stdin unavailable"))?;
    let stdout = child
      .stdout
      .take()
      .ok_or_else(|| anyhow!("MLX ASR helper stdout unavailable"))?;
    let mut client = Self { child, stdin, stdout: BufReader::new(stdout) };
    let ready = client.read_response().context("MLX ASR helper did not report ready")?;
    if ready.get("type").and_then(Value::as_str) != Some("ready") {
      return Err(anyhow!("MLX ASR helper sent unexpected startup frame: {ready}"));
    }
    response_ok(&ready)?;
    Ok(client)
  }

  pub fn transcribe_chunk(&mut self, samples: &[f32]) -> Result<String> {
    let response = self.command(json!({
      "type": "chunk",
      "samples": samples,
    }))?;
    Ok(response.get("delta").and_then(Value::as_str).unwrap_or_default().to_string())
  }

  pub fn reset_turn(&mut self) -> Result<()> {
    let response = self.command(json!({ "type": "reset" }))?;
    response_ok(&response)
  }

  pub fn final_transcript(&mut self) -> Result<Option<String>> {
    let response = self.command(json!({ "type": "finish" }))?;
    let text = response
      .get("text")
      .and_then(Value::as_str)
      .unwrap_or_default()
      .trim()
      .to_string();
    Ok((!text.is_empty()).then_some(text))
  }

  fn command(&mut self, payload: Value) -> Result<Value> {
    serde_json::to_writer(&mut self.stdin, &payload)
      .context("failed to encode MLX helper command")?;
    self.stdin.write_all(b"\n").context("failed to write MLX helper command")?;
    self.stdin.flush().context("failed to flush MLX helper command")?;

    let response = self.read_response()?;
    response_ok(&response)?;
    Ok(response)
  }

  fn read_response(&mut self) -> Result<Value> {
    let mut line = String::new();
    let n = self.stdout.read_line(&mut line).context("failed to read MLX helper response")?;
    if n == 0 {
      return Err(anyhow!("MLX ASR helper exited"));
    }
    serde_json::from_str(line.trim_end()).context("failed to parse MLX helper response")
  }
}

impl Drop for MlxNemotronAsr {
  fn drop(&mut self) {
    let _ = serde_json::to_writer(&mut self.stdin, &json!({ "type": "shutdown" }));
    let _ = self.stdin.write_all(b"\n");
    let _ = self.stdin.flush();
    let _ = self.child.try_wait();
  }
}

fn response_ok(response: &Value) -> Result<()> {
  if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
    return Ok(());
  }
  let message = response
    .get("error")
    .and_then(Value::as_str)
    .unwrap_or("unknown MLX ASR helper error");
  Err(anyhow!("{message}"))
}

fn resolve_helper_path(configured: Option<&Path>) -> Result<PathBuf> {
  let candidates = helper_path_candidates(configured, std::env::current_exe().ok());
  candidates
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| anyhow!("MLX ASR helper not found; run `just install` to build and bundle it"))
}

fn helper_path_candidates(configured: Option<&Path>, current_exe: Option<PathBuf>) -> Vec<PathBuf> {
  let mut candidates = Vec::new();
  if let Some(path) = configured {
    candidates.push(path.to_path_buf());
  }
  if let Some(path) = std::env::var_os("AZAD_MLX_ASR_HELPER") {
    candidates.push(PathBuf::from(path));
  }
  if let Some(exe) = current_exe {
    if let Some(dir) = exe.parent() {
      candidates.push(dir.join("azad-mlx-asr"));
    }
  }
  candidates.push(PathBuf::from("target/swift/azad-mlx-asr/release/azad-mlx-asr"));
  candidates
}

#[cfg(test)]
mod tests {
  use super::helper_path_candidates;
  use std::path::{Path, PathBuf};

  #[test]
  fn helper_candidates_prefer_configured_path() {
    let configured = Path::new("/tmp/custom-azad-mlx-asr");
    let candidates = helper_path_candidates(
      Some(configured),
      Some(PathBuf::from("/Applications/Azad.app/Contents/MacOS/azad")),
    );
    assert_eq!(candidates.first().unwrap(), configured);
  }

  #[test]
  fn helper_candidates_include_bundled_helper_next_to_current_exe() {
    let candidates = helper_path_candidates(
      None,
      Some(PathBuf::from("/Applications/Azad.app/Contents/MacOS/azad")),
    );
    assert!(candidates.iter().any(|p| p.ends_with("Contents/MacOS/azad-mlx-asr")));
  }
}
