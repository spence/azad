//! Local Agent Gateway WebSocket client.
//!
//! A single dedicated worker thread owns a blocking `tungstenite` socket to
//! `ws://127.0.0.1:8787` and bridges it to the main-thread `AppController`: outbound
//! requests arrive as [`GatewayCommand`]s on an mpsc channel; inbound frames are mapped to
//! [`GatewayEvent`]s and posted with `crate::app::send_event` — the same channel the speech
//! engine uses, drained on the 50ms UI tick. The worker never touches the controller
//! directly, mirroring the `model_download` background pattern.

use std::env;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Error, Message, WebSocket, connect};

use crate::app::{AppEvent, send_event};

pub const GATEWAY_AGENT: &str = "claude";
pub const GATEWAY_MODEL_ID: &str = "opus-4.8";
pub const GATEWAY_MODEL_EFFORT: &str = "high";

const DEFAULT_PORT: u16 = 8787;
const READ_TIMEOUT: Duration = Duration::from_millis(100);

static GATEWAY_TX: OnceLock<Mutex<Option<Sender<GatewayCommand>>>> = OnceLock::new();
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Display state of the conversation, shared with the overlay renderer. Owned here because
/// the gateway is the source of truth for run progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvStatus {
  Thinking,
  Streaming,
  Done,
  Error,
}

/// Non-answer activity phase from `run.activity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvPhase {
  Thinking,
  Researching,
  Responding,
  Idle,
}

/// Outbound request the main thread asks the worker to send. `req_id` is generated on the
/// main thread so the controller can correlate the `runs.create` response that carries the
/// new `thread_id`.
#[derive(Debug, Clone)]
pub enum GatewayCommand {
  SendNewThread { req_id: String, query: String },
  SendFollowup { req_id: String, thread_id: String, query: String },
  Close { req_id: String, thread_id: String },
  Shutdown,
}

/// Inbound result mapped from a daemon frame, delivered to the controller as
/// `AppEvent::Gateway`.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
  Connected,
  Disconnected { reason: String },
  RunAccepted { thread_id: String, run_id: String },
  Delta { thread_id: String, content: Option<String>, delta: Option<String>, replace: bool },
  Completed { thread_id: String, content: String },
  Activity { phase: ConvPhase, label: Option<String> },
  Failed { error: String },
  RequestError { error: String },
}

/// A monotonically increasing request id, e.g. `azad-42`. Deterministic (no wall clock).
pub fn make_request_id() -> String {
  let n = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
  format!("azad-{n}")
}

/// Spawn the worker thread (and its command channel) if one is not already alive. The
/// worker connects immediately; a failed connect posts `Disconnected` and exits, so a
/// later call re-spawns and retries.
pub fn ensure_worker() {
  let slot = GATEWAY_TX.get_or_init(|| Mutex::new(None));
  let mut guard = slot.lock().unwrap();
  if guard.is_some() {
    return;
  }
  let (tx, rx) = mpsc::channel::<GatewayCommand>();
  *guard = Some(tx);
  drop(guard);
  let _ = thread::Builder::new()
    .name("azad-gateway".to_string())
    .spawn(move || worker_main(rx));
}

/// Queue a command for the worker. Returns false when no worker is alive.
pub fn send_command(cmd: GatewayCommand) -> bool {
  let Some(slot) = GATEWAY_TX.get() else {
    return false;
  };
  let guard = slot.lock().unwrap();
  match guard.as_ref() {
    Some(tx) => tx.send(cmd).is_ok(),
    None => false,
  }
}

/// Apply a streaming delta to an accumulated reply buffer. The full `content` wins when
/// present (robust against duplicate provider renders); otherwise `delta` is appended, or
/// replaces the buffer when `replace` is set.
pub fn apply_delta(buffer: &mut String, content: Option<&str>, delta: Option<&str>, replace: bool) {
  if let Some(c) = content {
    buffer.clear();
    buffer.push_str(c);
  } else if let Some(d) = delta {
    if replace {
      buffer.clear();
    }
    buffer.push_str(d);
  }
}

fn clear_worker() {
  if let Some(slot) = GATEWAY_TX.get() {
    *slot.lock().unwrap() = None;
  }
}

fn worker_main(rx: Receiver<GatewayCommand>) {
  let port = env::var("AGENT_GATEWAY_PORT")
    .ok()
    .and_then(|v| v.parse::<u16>().ok())
    .unwrap_or(DEFAULT_PORT);
  let url = format!("ws://127.0.0.1:{port}");

  let mut socket = match connect(url.as_str()) {
    Ok((socket, _resp)) => {
      eprintln!("AZAD_GATEWAY worker connect url={url} result=connected");
      socket
    }
    Err(e) => {
      eprintln!("AZAD_GATEWAY worker connect url={url} result=error reason={e}");
      send_event(AppEvent::Gateway(GatewayEvent::Disconnected { reason: e.to_string() }));
      clear_worker();
      return;
    }
  };

  // Read timeout lets the single thread interleave reads with outbound command draining.
  if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
  }

  send_event(AppEvent::Gateway(GatewayEvent::Connected));
  run_loop(&mut socket, &rx);
  let _ = socket.close(None);
  clear_worker();
}

fn run_loop(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, rx: &Receiver<GatewayCommand>) {
  loop {
    loop {
      match rx.try_recv() {
        Ok(GatewayCommand::Shutdown) => return,
        Ok(cmd) => {
          if let Some(text) = command_to_json(&cmd) {
            eprintln!("AZAD_GATEWAY worker send {text}");
            if socket.send(Message::text(text)).is_err() {
              post_disconnect("send failed");
              return;
            }
          }
        }
        Err(TryRecvError::Empty) => break,
        Err(TryRecvError::Disconnected) => return,
      }
    }

    match socket.read() {
      Ok(Message::Text(t)) => {
        eprintln!("AZAD_GATEWAY worker recv {}", truncate_frame(t.as_str()));
        handle_inbound(t.as_str());
      }
      Ok(Message::Close(_)) => {
        post_disconnect("server closed");
        return;
      }
      Ok(_) => {}
      Err(Error::Io(e)) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
      }
      Err(Error::ConnectionClosed) | Err(Error::AlreadyClosed) => {
        post_disconnect("connection closed");
        return;
      }
      Err(e) => {
        post_disconnect(&e.to_string());
        return;
      }
    }
  }
}

fn post_disconnect(reason: &str) {
  eprintln!("AZAD_GATEWAY worker disconnect reason={reason}");
  send_event(AppEvent::Gateway(GatewayEvent::Disconnected { reason: reason.to_string() }));
}

/// Cap a logged frame so a long streamed reply doesn't flood the log; keeps the head, which
/// carries the type/name and enough payload to diagnose shape mismatches.
fn truncate_frame(text: &str) -> String {
  const MAX: usize = 600;
  if text.chars().count() <= MAX {
    return text.to_string();
  }
  let head: String = text.chars().take(MAX).collect();
  format!("{head}… (+{} chars)", text.chars().count() - MAX)
}

fn command_to_json(cmd: &GatewayCommand) -> Option<String> {
  let value = match cmd {
    GatewayCommand::SendNewThread { req_id, query } => json!({
      "type": "request",
      "id": req_id,
      "method": "runs.create",
      "params": {
        "agent": GATEWAY_AGENT,
        "model": { "id": GATEWAY_MODEL_ID, "effort": GATEWAY_MODEL_EFFORT },
        "input": { "text": query, "attachments": [] },
        "response": { "format": "text" },
        "close": { "policy": "application_managed" },
        "user_approved": true,
      },
    }),
    GatewayCommand::SendFollowup { req_id, thread_id, query } => json!({
      "type": "request",
      "id": req_id,
      "method": "runs.create",
      "params": {
        "thread_id": thread_id,
        "input": { "text": query, "attachments": [] },
        "response": { "format": "text" },
        "user_approved": true,
      },
    }),
    GatewayCommand::Close { req_id, thread_id } => json!({
      "type": "request",
      "id": req_id,
      "method": "threads.close",
      "params": { "thread_id": thread_id },
    }),
    GatewayCommand::Shutdown => return None,
  };
  Some(value.to_string())
}

fn handle_inbound(text: &str) {
  let Ok(frame) = serde_json::from_str::<Value>(text) else {
    return;
  };
  match frame.get("type").and_then(Value::as_str).unwrap_or("") {
    "response" => {
      if !frame.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        let error = frame
          .get("error")
          .and_then(Value::as_str)
          .unwrap_or("unknown_error")
          .to_string();
        send_event(AppEvent::Gateway(GatewayEvent::RequestError { error }));
        return;
      }
      let result = frame.get("result");
      let thread_id = result.and_then(|r| r.get("thread_id")).and_then(Value::as_str);
      let run_id = result.and_then(|r| r.get("run_id")).and_then(Value::as_str);
      if let (Some(thread_id), Some(run_id)) = (thread_id, run_id) {
        send_event(AppEvent::Gateway(GatewayEvent::RunAccepted {
          thread_id: thread_id.to_string(),
          run_id: run_id.to_string(),
        }));
      }
    }
    "event" => {
      let name = frame.get("name").and_then(Value::as_str).unwrap_or("");
      let data = frame.get("data").cloned().unwrap_or(Value::Null);
      handle_event_frame(name, &data);
    }
    _ => {}
  }
}

fn handle_event_frame(name: &str, data: &Value) {
  let thread_id = data.get("thread_id").and_then(Value::as_str).unwrap_or("").to_string();
  match name {
    "message.delta" => {
      let content = data.get("content").and_then(Value::as_str).map(str::to_string);
      let delta = data.get("delta").and_then(Value::as_str).map(str::to_string);
      let replace = data.get("replace").and_then(Value::as_bool).unwrap_or(false);
      send_event(AppEvent::Gateway(GatewayEvent::Delta { thread_id, content, delta, replace }));
    }
    "message.completed" => {
      let content = data.get("content").and_then(Value::as_str).unwrap_or("").to_string();
      send_event(AppEvent::Gateway(GatewayEvent::Completed { thread_id, content }));
    }
    "run.activity" => {
      let phase = match data.get("phase").and_then(Value::as_str).unwrap_or("") {
        "thinking" => ConvPhase::Thinking,
        "researching" => ConvPhase::Researching,
        "responding" => ConvPhase::Responding,
        _ => ConvPhase::Idle,
      };
      let label = data.get("label").and_then(Value::as_str).map(str::to_string);
      send_event(AppEvent::Gateway(GatewayEvent::Activity { phase, label }));
    }
    "run.failed" => {
      let error = data
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| data.get("label").and_then(Value::as_str))
        .unwrap_or("run_failed")
        .to_string();
      send_event(AppEvent::Gateway(GatewayEvent::Failed { error }));
    }
    _ => {}
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn new_thread_envelope_has_agent_model_and_approval() {
    let json = command_to_json(&GatewayCommand::SendNewThread {
      req_id: "r1".into(),
      query: "hello".into(),
    })
    .unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["type"], "request");
    assert_eq!(v["method"], "runs.create");
    assert_eq!(v["params"]["agent"], "claude");
    assert_eq!(v["params"]["model"]["id"], "opus-4.8");
    assert_eq!(v["params"]["model"]["effort"], "high");
    assert_eq!(v["params"]["input"]["text"], "hello");
    assert_eq!(v["params"]["response"]["format"], "text");
    assert_eq!(v["params"]["user_approved"], true);
    assert!(v["params"].get("thread_id").is_none());
  }

  #[test]
  fn followup_envelope_carries_thread_id_and_omits_agent() {
    let json = command_to_json(&GatewayCommand::SendFollowup {
      req_id: "r2".into(),
      thread_id: "t1".into(),
      query: "more".into(),
    })
    .unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["method"], "runs.create");
    assert_eq!(v["params"]["thread_id"], "t1");
    assert_eq!(v["params"]["input"]["text"], "more");
    assert_eq!(v["params"]["user_approved"], true);
    assert!(v["params"].get("agent").is_none());
    assert!(v["params"].get("model").is_none());
  }

  #[test]
  fn close_envelope_is_ungated() {
    let json =
      command_to_json(&GatewayCommand::Close { req_id: "r3".into(), thread_id: "t1".into() })
        .unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["method"], "threads.close");
    assert_eq!(v["params"]["thread_id"], "t1");
    assert!(v["params"].get("user_approved").is_none());
  }

  #[test]
  fn shutdown_has_no_envelope() {
    assert!(command_to_json(&GatewayCommand::Shutdown).is_none());
  }

  #[test]
  fn delta_content_wins() {
    let mut buf = "old".to_string();
    apply_delta(&mut buf, Some("Hello"), Some("ignored"), false);
    assert_eq!(buf, "Hello");
  }

  #[test]
  fn delta_appends_without_content() {
    let mut buf = "Hel".to_string();
    apply_delta(&mut buf, None, Some("lo"), false);
    assert_eq!(buf, "Hello");
  }

  #[test]
  fn delta_replace_overwrites() {
    let mut buf = "stale".to_string();
    apply_delta(&mut buf, None, Some("fresh"), true);
    assert_eq!(buf, "fresh");
  }
}
