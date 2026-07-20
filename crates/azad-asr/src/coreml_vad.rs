use crate::mlx;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Debug, Clone)]
pub struct CoreMlVadConfig {
  pub model_path: PathBuf,
  pub helper_path: Option<PathBuf>,
}

pub struct CoreMlVadProcessor {
  child: Child,
  stdin: ChildStdin,
  stdout: BufReader<ChildStdout>,
}

impl CoreMlVadProcessor {
  pub fn load(cfg: &CoreMlVadConfig) -> Result<Self> {
    let helper = mlx::resolve_helper_path(cfg.helper_path.as_deref())?;
    let mut child = Command::new(&helper)
      .arg("--vad-model")
      .arg(&cfg.model_path)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::inherit())
      .spawn()
      .with_context(|| format!("failed to spawn CoreML VAD helper at {}", helper.display()))?;

    let stdin = child
      .stdin
      .take()
      .ok_or_else(|| anyhow!("CoreML VAD helper stdin unavailable"))?;
    let stdout = child
      .stdout
      .take()
      .ok_or_else(|| anyhow!("CoreML VAD helper stdout unavailable"))?;
    let mut client = Self { child, stdin, stdout: BufReader::new(stdout) };
    let ready = client.read_response().context("CoreML VAD helper did not report ready")?;
    if ready.get("type").and_then(Value::as_str) != Some("ready") {
      return Err(anyhow!("CoreML VAD helper sent unexpected startup frame: {ready}"));
    }
    response_ok(&ready)?;
    Ok(client)
  }

  pub fn probabilities(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
    let response = self.command(json!({
      "type": "vad",
      "samples": samples,
    }))?;
    let probs = response
      .get("probs")
      .and_then(Value::as_array)
      .ok_or_else(|| anyhow!("CoreML VAD helper response missing probs"))?;
    probs
      .iter()
      .map(|v| {
        v.as_f64()
          .map(|f| f as f32)
          .ok_or_else(|| anyhow!("invalid CoreML VAD probability"))
      })
      .collect()
  }

  pub fn reset(&mut self) -> Result<()> {
    self.command(json!({ "type": "reset" }))?;
    Ok(())
  }

  fn command(&mut self, payload: Value) -> Result<Value> {
    serde_json::to_writer(&mut self.stdin, &payload)
      .context("failed to encode CoreML VAD command")?;
    self.stdin.write_all(b"\n").context("failed to write CoreML VAD command")?;
    self.stdin.flush().context("failed to flush CoreML VAD command")?;

    let response = self.read_response()?;
    response_ok(&response)?;
    Ok(response)
  }

  fn read_response(&mut self) -> Result<Value> {
    let mut line = String::new();
    let n = self
      .stdout
      .read_line(&mut line)
      .context("failed to read CoreML VAD helper response")?;
    if n == 0 {
      return Err(anyhow!("CoreML VAD helper exited"));
    }
    serde_json::from_str(line.trim_end()).context("failed to parse CoreML VAD helper response")
  }
}

impl Drop for CoreMlVadProcessor {
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
    .unwrap_or("unknown CoreML VAD helper error");
  Err(anyhow!("{message}"))
}
