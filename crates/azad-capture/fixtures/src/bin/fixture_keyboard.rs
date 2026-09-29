//! A virtual source keyboard that plays a scripted report sequence.
//!
//! Usage: fixture-keyboard --script <steps.json> [--vendor N --product N]
//! Each step is `{"keys": [usages], "modifiers": bits, "hold_ms": n, "secure": "on"|"off"}`;
//! `secure` toggles Secure Input from this process before the step's report is posted.

use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use azad_capture::engine::ForwardState;
use azad_capture::sys::now_nanos;
use azad_capture::vhid::{self, Client};
use azad_capture_fixtures::*;

const FIXTURE_VENDOR_ID: u32 = 0xfeed;
const FIXTURE_PRODUCT_ID: u32 = 0x1790;

#[derive(Deserialize)]
struct Step {
  #[serde(default)]
  keys: Vec<u16>,
  #[serde(default)]
  modifiers: u8,
  #[serde(default = "default_hold")]
  hold_ms: u64,
  #[serde(default)]
  secure: Option<String>,
}

fn default_hold() -> u64 {
  120
}

fn main() {
  require_virtual_mac();
  let args: Vec<String> = std::env::args().collect();
  let script = arg(&args, "--script").expect("--script");
  let vendor = arg(&args, "--vendor").map_or(FIXTURE_VENDOR_ID, |v| v.parse().expect("vendor"));
  let product = arg(&args, "--product").map_or(FIXTURE_PRODUCT_ID, |v| v.parse().expect("product"));
  let settle_ms: u64 = arg(&args, "--settle-ms").map_or(3000, |v| v.parse().expect("settle"));
  let steps: Vec<Step> =
    serde_json::from_str(&std::fs::read_to_string(script).expect("script")).expect("steps");

  // An unidentified keyboard's keys never reach applications; identify the fixture as ANSI.
  let _ = std::process::Command::new("/usr/bin/defaults")
    .args([
      "write",
      "/Library/Preferences/com.apple.keyboardtype",
      "keyboardtype",
      "-dict-add",
      &format!("{product}-{vendor}-0"),
      "-int",
      "40",
    ])
    .status();
  let (ready_tx, ready_rx) = std::sync::mpsc::channel();
  let client = Client::connect_keyboard(
    vhid::SERVER_SOCKET_PATH,
    vendor,
    product,
    0,
    Box::new(move |state| {
      let _ = ready_tx.send(state.can_forward());
    }),
  )
  .expect("connect to virtual keyboard service");
  loop {
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
      Ok(true) => break,
      Ok(false) => continue,
      Err(_) => {
        eprintln!("source keyboard never became ready");
        std::process::exit(2);
      }
    }
  }
  println!("{}", json!({ "event": "source_ready", "wall_ms": wall_ms() }));
  // Lets the helper discover, observe and seize the new device before input starts.
  thread::sleep(Duration::from_millis(settle_ms));

  let mut previous = ForwardState::default();
  for (index, step) in steps.iter().enumerate() {
    match step.secure.as_deref() {
      // SAFETY: Carbon Secure Input calls have no preconditions.
      Some("on") => unsafe {
        EnableSecureEventInput();
      },
      // SAFETY: As above.
      Some("off") => unsafe {
        DisableSecureEventInput();
      },
      _ => {}
    }
    let next = ForwardState {
      modifiers: step.modifiers,
      keyboard: step.keys.clone(),
      ..ForwardState::default()
    };
    client.post_state(&previous, &next).expect("post report");
    previous = next;
    // SAFETY: No preconditions.
    let secure = unsafe { IsSecureEventInputEnabled() } != 0;
    println!(
      "{}",
      json!({ "event": "report", "index": index, "t_ns": now_nanos(), "secure": secure,
              "modifiers": step.modifiers, "keys": step.keys })
    );
    thread::sleep(Duration::from_millis(step.hold_ms));
  }
  client.release_all().ok();
  // SAFETY: No preconditions.
  unsafe { DisableSecureEventInput() };
  thread::sleep(Duration::from_millis(500));
  client.shutdown();
  println!("{}", json!({ "event": "source_done", "reports": steps.len() }));
}
