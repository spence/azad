//! Stands in for the Azad app: connects to the helper, publishes a key context, and records
//! every helper message with its local receipt time.
//!
//! Usage: fixture-app --context <json> --seconds N [--silent-after-ms N]
//! Heartbeats every 250 ms like the app's main thread; `--silent-after-ms` stops them while
//! staying connected, modelling a hung app.
//! Must be signed with identifier `ai.azad` by the helper's team to be accepted.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use azad_capture::ipc::{AppMessage, HEARTBEAT_INTERVAL, PROTOCOL_VERSION, SOCKET_PATH};
use azad_capture::policy::KeyContext;
use azad_capture::sys::now_nanos;
use azad_capture_fixtures::*;

fn main() {
  require_virtual_mac();
  let args: Vec<String> = std::env::args().collect();
  let context: KeyContext =
    serde_json::from_str(&arg(&args, "--context").unwrap_or_else(|| "{}".into())).expect("context");
  let seconds: u64 = arg(&args, "--seconds").map_or(10, |v| v.parse().expect("seconds"));
  let mut stream = match UnixStream::connect(SOCKET_PATH) {
    Ok(stream) => stream,
    Err(error) => {
      println!("{}", json!({ "event": "connect_failed", "error": error.to_string() }));
      std::process::exit(3);
    }
  };
  for message in [AppMessage::Hello { protocol: PROTOCOL_VERSION }, AppMessage::Context { context }]
  {
    let mut line = serde_json::to_string(&message).expect("message");
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() {
      println!("{}", json!({ "event": "write_failed" }));
      std::process::exit(4);
    }
  }
  println!("{}", json!({ "event": "app_connected", "wall_ms": wall_ms() }));
  let silent_after = arg(&args, "--silent-after-ms").map(|v| v.parse::<u64>().expect("silent"));
  let mut heartbeat = stream.try_clone().expect("clone stream");
  std::thread::spawn(move || {
    let started = Instant::now();
    let mut line = serde_json::to_string(&AppMessage::Heartbeat).expect("heartbeat");
    line.push('\n');
    loop {
      if silent_after.is_some_and(|ms| started.elapsed() >= Duration::from_millis(ms)) {
        println!("{}", json!({ "event": "heartbeat_stopped", "wall_ms": wall_ms() }));
        return;
      }
      if heartbeat.write_all(line.as_bytes()).is_err() {
        return;
      }
      std::thread::sleep(HEARTBEAT_INTERVAL);
    }
  });
  let deadline = Instant::now() + Duration::from_secs(seconds);
  stream.set_read_timeout(Some(Duration::from_millis(200))).ok();
  let mut reader = BufReader::new(stream);
  let mut line = String::new();
  while Instant::now() < deadline {
    line.clear();
    match reader.read_line(&mut line) {
      Ok(0) => {
        println!("{}", json!({ "event": "helper_closed", "wall_ms": wall_ms() }));
        break;
      }
      Ok(_) => {
        let message: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
        println!(
          "{}",
          json!({ "event": "received", "received_ns": now_nanos(), "message": message })
        );
      }
      Err(error)
        if matches!(
          error.kind(),
          std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) => {}
      Err(error) => {
        println!("{}", json!({ "event": "read_failed", "error": error.to_string() }));
        break;
      }
    }
  }
  println!("{}", json!({ "event": "app_done", "wall_ms": wall_ms() }));
}
