//! Client of the root keyboard-capture helper (`azad-capture`).
//!
//! The helper owns global shortcut capture below application event taps: it seizes keyboards,
//! applies the shared key policy to the context published here, and sends the claimed actions
//! stamped with their capture time. The app renews its context from the main thread; if the
//! main thread stalls, the helper stops claiming everything but the listen chord.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use azad_capture::clock::now_nanos;
use azad_capture::policy::{KeyAction, KeyContext};
use azad_capture::protocol::{
  AppMessage, HelperMessage, HelperStatus, PROTOCOL_VERSION, SOCKET_PATH,
};

use cocoa::base::{id, nil};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};

use crate::app::{AppEvent, send_event};

const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);
/// Bounds a main-thread write if the helper stops reading.
const WRITE_TIMEOUT: Duration = Duration::from_millis(50);

struct Connection {
  writer: Option<UnixStream>,
  context: KeyContext,
  status: Option<HelperStatus>,
}

static CONNECTION: Mutex<Connection> =
  Mutex::new(Connection { writer: None, context: KEY_CONTEXT_NONE, status: None });

const KEY_CONTEXT_NONE: KeyContext = KeyContext {
  listen_modifiers: 0,
  escape: false,
  enter: false,
  arrows: false,
  arrow_left: false,
  arrow_right: false,
  search_input: false,
};

/// Starts connecting to the helper in the background; reconnects whenever it goes away.
pub fn start() {
  let _ = thread::Builder::new().name("azad-capture-client".into()).spawn(|| {
    loop {
      if let Ok(stream) = UnixStream::connect(SOCKET_PATH) {
        run_connection(stream);
      }
      thread::sleep(RECONNECT_INTERVAL);
    }
  });
}

/// Publishes the key context the helper applies to the next key edge.
pub fn publish_context(context: KeyContext) {
  let mut connection = CONNECTION.lock().unwrap_or_else(|poison| poison.into_inner());
  connection.context = context;
  send(&mut connection, &AppMessage::Context { context });
}

/// Renews the context lease. Called from a main-thread timer so a stalled main thread lets the
/// lease lapse.
pub fn heartbeat() {
  let mut connection = CONNECTION.lock().unwrap_or_else(|poison| poison.into_inner());
  send(&mut connection, &AppMessage::Heartbeat);
}

/// The helper's latest status, or `None` while it is not connected.
pub fn status() -> Option<HelperStatus> {
  CONNECTION.lock().unwrap_or_else(|poison| poison.into_inner()).status.clone()
}

fn send(connection: &mut Connection, message: &AppMessage) {
  let Some(writer) = connection.writer.as_mut() else { return };
  let Ok(mut line) = serde_json::to_string(message) else { return };
  line.push('\n');
  if writer.write_all(line.as_bytes()).is_err() {
    let _ = writer.shutdown(std::net::Shutdown::Both);
    connection.writer = None;
  }
}

fn run_connection(stream: UnixStream) {
  let Ok(writer) = stream.try_clone() else { return };
  let _ = writer.set_write_timeout(Some(WRITE_TIMEOUT));
  {
    let mut connection = CONNECTION.lock().unwrap_or_else(|poison| poison.into_inner());
    connection.writer = Some(writer);
    let context = connection.context;
    send(&mut connection, &AppMessage::Hello { protocol: PROTOCOL_VERSION });
    send(&mut connection, &AppMessage::Context { context });
  }
  eprintln!("AZAD_CAPTURE connected");
  for line in BufReader::new(&stream).lines() {
    let Ok(line) = line else { break };
    match serde_json::from_str::<HelperMessage>(&line) {
      Ok(HelperMessage::Action { action, timestamp_ns }) => {
        send_event(app_event(action, capture_instant(timestamp_ns, now_nanos(), Instant::now())));
      }
      Ok(HelperMessage::Status { status }) => {
        eprintln!(
          "AZAD_CAPTURE status capture={:?} permission={:?} driver={:?} seized={} unavailable={:?}",
          status.capture,
          status.permission,
          status.driver,
          status.seized_devices,
          status.unavailable_devices
        );
        CONNECTION.lock().unwrap_or_else(|poison| poison.into_inner()).status = Some(status);
        send_event(AppEvent::KeyboardCaptureStatusChanged);
      }
      Err(_) => {}
    }
  }
  let mut connection = CONNECTION.lock().unwrap_or_else(|poison| poison.into_inner());
  connection.writer = None;
  connection.status = None;
  drop(connection);
  eprintln!("AZAD_CAPTURE disconnected");
  send_event(AppEvent::KeyboardCaptureStatusChanged);
}

/// Registration state of the helper's LaunchDaemon (`SMAppService`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperRegistration {
  Enabled,
  /// Registered; the user must allow it in Login Items & Extensions.
  RequiresApproval,
  /// The helper or its LaunchDaemon plist is missing from the app bundle.
  NotFound,
  Failed(String),
}

const DAEMON_PLIST: &str = "ai.azad.capture.plist";
const HELPER_BUNDLE: &str = "Contents/Library/Helpers/Azad Capture.app";

/// Registers the helper LaunchDaemon if it is not registered yet and reports its state.
pub fn ensure_helper_registered() -> HelperRegistration {
  // SAFETY: SMAppService is available on macOS 13+; every object is owned by autorelease pools
  // or borrowed for the duration of the call.
  unsafe {
    let name = NSString::alloc(nil).init_str(DAEMON_PLIST);
    let service: id = msg_send![class!(SMAppService), daemonServiceWithPlistName: name];
    if service == nil {
      return HelperRegistration::NotFound;
    }
    let status: isize = msg_send![service, status];
    if status == 1 {
      return HelperRegistration::Enabled;
    }
    if status == 2 {
      return HelperRegistration::RequiresApproval;
    }
    let mut error: id = nil;
    let registered: bool = msg_send![service, registerAndReturnError: &mut error];
    let status: isize = msg_send![service, status];
    match (registered, status) {
      (_, 1) => HelperRegistration::Enabled,
      (_, 2) => HelperRegistration::RequiresApproval,
      (_, 3) => HelperRegistration::NotFound,
      _ => HelperRegistration::Failed(describe_error(error)),
    }
  }
}

/// Opens Login Items & Extensions, where a background item awaiting approval is allowed.
pub fn open_login_items_settings() {
  // SAFETY: Class method with no preconditions.
  unsafe {
    let _: () = msg_send![class!(SMAppService), openSystemSettingsLoginItems];
  }
}

/// Asks macOS for Input Monitoring on behalf of the helper. A root daemon cannot be prompted,
/// so the helper bundle is launched once in this user session to show the standard prompt; the
/// grant is recorded for its bundle identity and applies to the daemon.
pub fn request_helper_input_monitoring() {
  let Some(helper) = helper_bundle_path() else { return };
  let _ = std::process::Command::new("/usr/bin/open")
    .arg("-a")
    .arg(helper)
    .args(["--args", "--request-access"])
    .spawn();
}

fn helper_bundle_path() -> Option<std::path::PathBuf> {
  let executable = std::env::current_exe().ok()?;
  // …/Azad.app/Contents/MacOS/azad → …/Azad.app
  let app = executable.parent()?.parent()?.parent()?;
  let helper = app.join(HELPER_BUNDLE);
  helper.exists().then_some(helper)
}

unsafe fn describe_error(error: id) -> String {
  if error == nil {
    return "unknown".into();
  }
  // SAFETY: `error` is an NSError returned by the failed call.
  unsafe {
    let description: id = msg_send![error, localizedDescription];
    let bytes: *const std::ffi::c_char = msg_send![description, UTF8String];
    if bytes.is_null() {
      return "unknown".into();
    }
    std::ffi::CStr::from_ptr(bytes).to_string_lossy().into_owned()
  }
}

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// Maps a capture timestamp (host monotonic nanoseconds) onto this process's `Instant` clock.
/// Both clocks are the same monotonic source, so the age of the event is preserved exactly.
fn capture_instant(captured_ns: u64, now_ns: u64, now: Instant) -> Instant {
  now
    .checked_sub(Duration::from_nanos(now_ns.saturating_sub(captured_ns)))
    .unwrap_or(now)
}

fn app_event(action: KeyAction, captured_at: Instant) -> AppEvent {
  match action {
    KeyAction::HotkeyPressed => AppEvent::HotkeyPressed { captured_at },
    KeyAction::HotkeyReleased { raw_requested } => AppEvent::HotkeyReleased { raw_requested },
    KeyAction::Finalize { raw_requested } => AppEvent::FinalizeHotkeyPressed { raw_requested },
    KeyAction::Navigate { direction } => AppEvent::ArrowNavigate(direction),
    KeyAction::Cancel => AppEvent::OverlayCancel,
    KeyAction::HistoryCollapse => AppEvent::HistoryCollapse,
    KeyAction::HistoryExpand => AppEvent::HistoryExpand,
    KeyAction::SearchBackspace => AppEvent::HistorySearchBackspace,
    KeyAction::SearchDeleteWord => AppEvent::HistorySearchDeleteWord,
    KeyAction::SearchClear => AppEvent::HistorySearchClear,
    KeyAction::SearchKey { usage, modifiers } => AppEvent::HistorySearchKey { usage, modifiers },
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn capture_time_keeps_event_age() {
    let now = Instant::now();
    let captured = capture_instant(1_000_000_000, 1_250_000_000, now);
    assert_eq!(now.duration_since(captured), Duration::from_millis(250));
  }

  #[test]
  fn future_capture_time_clamps_to_now() {
    let now = Instant::now();
    assert_eq!(capture_instant(2_000, 1_000, now), now);
  }
}
