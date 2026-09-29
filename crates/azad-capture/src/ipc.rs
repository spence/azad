//! Authenticated channel between the root helper and the Azad app.
//!
//! The helper listens on a world-connectable Unix socket but serves only a peer whose code
//! signature satisfies Azad's requirement (identifier `ai.azad`, signed by the helper's own
//! team). Messages are JSON lines. Unclaimed typing never crosses this channel.

use std::ffi::c_void;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::AsRawFd;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::hid::DeviceInfo;
use crate::policy::{KeyAction, KeyContext};
use crate::sys::*;

pub const SOCKET_PATH: &str = "/var/run/ai.azad.capture.sock";
pub const APP_IDENTIFIER: &str = "ai.azad";
pub const PROTOCOL_VERSION: u32 = 1;
/// Outgoing messages buffered per client before it is treated as stalled and dropped.
const CLIENT_QUEUE_DEPTH: usize = 256;
const SOL_LOCAL: libc::c_int = 0;
const LOCAL_PEERTOKEN: libc::c_int = 6;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
  Hello {
    protocol: u32,
  },
  Context {
    context: KeyContext,
  },
  /// Sent from the app's main thread at `HEARTBEAT_INTERVAL`; renews the context lease.
  Heartbeat,
}

/// How often the app renews its context lease.
pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// A context not renewed for this long is treated as coming from a hung app.
pub const CONTEXT_LEASE: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
  Granted,
  Denied,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverStatus {
  /// The virtual keyboard service socket is absent or refused the connection.
  ServiceUnavailable,
  /// The driver extension is not activated (not installed or not approved).
  NotActivated,
  VersionMismatch,
  Starting,
  Ready,
}

/// Why the helper is or is not capturing. `Capturing` requires every precondition; anything
/// else means claimed shortcuts are not being received and must not be reported as healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStatus {
  Capturing,
  PermissionDenied,
  DriverUnavailable,
  NoCapturableKeyboard,
  Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperStatus {
  pub capture: CaptureStatus,
  pub permission: Permission,
  pub driver: DriverStatus,
  pub seized_devices: usize,
  /// Keyboards held exclusively by another process whose output Azad cannot see. Capture from
  /// them is unavailable even while `capture` is `capturing` for other keyboards.
  pub unavailable_devices: Vec<String>,
  pub devices: Vec<DeviceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSummary {
  pub kind: String,
  pub vendor_id: u32,
  pub product_id: u32,
  pub product: String,
  pub transport: String,
  pub state: String,
  pub seized_reports: u64,
  pub key_translation: bool,
}

impl From<&DeviceInfo> for DeviceSummary {
  fn from(info: &DeviceInfo) -> Self {
    let kind = serde_json::to_value(info.kind)
      .ok()
      .and_then(|v| v.as_str().map(str::to_string));
    let state = match info.state {
      crate::hid::DeviceState::Failed(code) => format!("failed:{code:#x}"),
      other => serde_json::to_value(other)
        .ok()
        .and_then(|value| value.get("state").and_then(|s| s.as_str().map(str::to_string)))
        .unwrap_or_default(),
    };
    Self {
      kind: kind.unwrap_or_default(),
      vendor_id: info.vendor_id,
      product_id: info.product_id,
      product: info.product.clone(),
      transport: info.transport.clone(),
      state,
      seized_reports: info.seized_reports,
      key_translation: info.key_translation,
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperMessage {
  Status {
    status: HelperStatus,
  },
  /// A claimed shortcut edge. `timestamp_ns` is the capture time on the host monotonic clock
  /// (`mach_absolute_time` in nanoseconds), not the delivery time.
  Action {
    action: KeyAction,
    timestamp_ns: u64,
  },
}

/// Connection lifecycle and requests delivered to the helper core.
pub enum ClientEvent {
  Connected { id: u64, sender: ClientSender },
  Message { id: u64, message: AppMessage },
  Disconnected { id: u64 },
  Rejected { reason: String },
}

#[derive(Clone)]
pub struct ClientSender {
  tx: SyncSender<String>,
}

impl ClientSender {
  /// Queues a message; returns false when the client is gone or not keeping up.
  pub fn send(&self, message: &HelperMessage) -> bool {
    let Ok(mut line) = serde_json::to_string(message) else { return false };
    line.push('\n');
    match self.tx.try_send(line) {
      Ok(()) => true,
      Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
    }
  }
}

/// Binds the socket and serves clients on background threads. `deliver` wakes the core.
pub fn serve(
  path: &str,
  requirement: String,
  deliver: impl Fn(ClientEvent) + Send + Sync + 'static,
) -> io::Result<()> {
  let _ = std::fs::remove_file(path);
  let listener = UnixListener::bind(path)?;
  // The socket is connectable by the console user; authorization is by code signature.
  std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o666))?;
  let deliver = std::sync::Arc::new(deliver);
  thread::Builder::new().name("azad-capture-accept".into()).spawn(move || {
    let mut next_id = 1u64;
    for stream in listener.incoming() {
      let Ok(stream) = stream else { continue };
      let id = next_id;
      next_id += 1;
      if let Err(reason) = authenticate(&stream, &requirement) {
        deliver(ClientEvent::Rejected { reason });
        let _ = stream.shutdown(std::net::Shutdown::Both);
        continue;
      }
      let deliver = deliver.clone();
      let _ = thread::Builder::new()
        .name("azad-capture-client".into())
        .spawn(move || run_client(id, stream, deliver.as_ref()));
    }
  })?;
  Ok(())
}

fn run_client(id: u64, stream: UnixStream, deliver: &(dyn Fn(ClientEvent) + Send + Sync)) {
  let Ok(writer) = stream.try_clone() else { return };
  let (tx, rx): (SyncSender<String>, Receiver<String>) = mpsc::sync_channel(CLIENT_QUEUE_DEPTH);
  let write_thread = thread::Builder::new().name("azad-capture-writer".into()).spawn(move || {
    let mut writer = writer;
    for line in rx {
      if writer.write_all(line.as_bytes()).is_err() {
        break;
      }
    }
    let _ = writer.shutdown(std::net::Shutdown::Both);
  });
  if write_thread.is_err() {
    return;
  }
  deliver(ClientEvent::Connected { id, sender: ClientSender { tx } });
  let reader = BufReader::new(&stream);
  for line in reader.lines() {
    let Ok(line) = line else { break };
    let Ok(message) = serde_json::from_str::<AppMessage>(&line) else { break };
    deliver(ClientEvent::Message { id, message });
  }
  let _ = stream.shutdown(std::net::Shutdown::Both);
  deliver(ClientEvent::Disconnected { id });
}

/// The requirement a client must satisfy: Azad's identifier signed by the helper's own team.
/// An unsigned or ad-hoc helper (a local development build) has no team to pin, so it falls
/// back to the identifier alone.
pub fn client_requirement() -> String {
  match own_team_identifier() {
    Some(team) => format!(
      "identifier \"{APP_IDENTIFIER}\" and anchor apple generic and certificate leaf[subject.OU] = \"{team}\""
    ),
    None => format!("identifier \"{APP_IDENTIFIER}\""),
  }
}

pub fn own_team_identifier() -> Option<String> {
  // SAFETY: Out-parameters are valid; every created object is released before returning.
  unsafe {
    let mut code: SecCodeRef = std::ptr::null_mut();
    if SecCodeCopySelf(kSecCSDefaultFlags, &mut code) != 0 {
      return None;
    }
    let mut static_code: SecStaticCodeRef = std::ptr::null_mut();
    let status = SecCodeCopyStaticCode(code, kSecCSDefaultFlags, &mut static_code);
    CFRelease(code.cast_const());
    if status != 0 {
      return None;
    }
    let mut info: CFDictionaryRef = std::ptr::null();
    let status = SecCodeCopySigningInformation(static_code, kSecCSSigningInformation, &mut info);
    CFRelease(static_code.cast_const());
    if status != 0 || info.is_null() {
      return None;
    }
    let team = cf_string(CFDictionaryGetValue(info, kSecCodeInfoTeamIdentifier.cast()));
    CFRelease(info);
    team
  }
}

fn authenticate(stream: &UnixStream, requirement: &str) -> Result<(), String> {
  let mut token = [0u8; 32];
  let mut length = token.len() as libc::socklen_t;
  // SAFETY: `token` is writable for `length` bytes; the descriptor is a connected socket.
  let result = unsafe {
    libc::getsockopt(
      stream.as_raw_fd(),
      SOL_LOCAL,
      LOCAL_PEERTOKEN,
      token.as_mut_ptr().cast::<c_void>(),
      &mut length,
    )
  };
  if result != 0 || length as usize != token.len() {
    return Err("peer audit token unavailable".into());
  }
  // SAFETY: All CF objects created here are released before returning; the guest lookup uses
  // the peer's audit token so a reused PID cannot be substituted.
  unsafe {
    let data = CFDataCreate(kCFAllocatorDefault, token.as_ptr(), token.len() as CFIndex);
    let keys = [kSecGuestAttributeAudit.cast::<c_void>()];
    let values = [data.cast::<c_void>()];
    let attributes = CFDictionaryCreate(
      kCFAllocatorDefault,
      keys.as_ptr(),
      values.as_ptr(),
      1,
      (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
      (&raw const kCFTypeDictionaryValueCallBacks).cast(),
    );
    CFRelease(data);
    let mut guest: SecCodeRef = std::ptr::null_mut();
    let status = SecCodeCopyGuestWithAttributes(
      std::ptr::null_mut(),
      attributes,
      kSecCSDefaultFlags,
      &mut guest,
    );
    CFRelease(attributes);
    if status != 0 || guest.is_null() {
      return Err(format!("peer code lookup failed: {status}"));
    }
    let text = CfString::new(requirement);
    let mut compiled: SecRequirementRef = std::ptr::null_mut();
    let status = SecRequirementCreateWithString(text.0, kSecCSDefaultFlags, &mut compiled);
    if status != 0 {
      CFRelease(guest.cast_const());
      return Err(format!("requirement compile failed: {status}"));
    }
    let status = SecCodeCheckValidity(guest, kSecCSDefaultFlags, compiled);
    CFRelease(compiled.cast_const());
    CFRelease(guest.cast_const());
    if status != 0 {
      return Err(format!("peer failed code requirement: {status}"));
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn app_messages_round_trip_as_tagged_json() {
    let message = AppMessage::Context {
      context: KeyContext { listen_modifiers: 4, escape: true, ..KeyContext::default() },
    };
    let json = serde_json::to_string(&message).unwrap();
    assert!(json.starts_with("{\"type\":\"context\""));
    assert_eq!(serde_json::from_str::<AppMessage>(&json).unwrap(), message);
  }

  #[test]
  fn action_message_carries_capture_timestamp() {
    let json = serde_json::to_string(&HelperMessage::Action {
      action: KeyAction::HotkeyReleased { raw_requested: true },
      timestamp_ns: 42,
    })
    .unwrap();
    assert_eq!(
      json,
      "{\"type\":\"action\",\"action\":{\"kind\":\"hotkey_released\",\"raw_requested\":true},\"timestamp_ns\":42}"
    );
  }
}
