//! Helper side of the app channel: the socket server and peer authentication.
//!
//! The helper listens on a world-connectable Unix socket but serves only a peer whose code
//! signature satisfies Azad's requirement (identifier `ai.azad`, signed by the helper's own
//! team). Unclaimed typing never crosses this channel.

use std::ffi::c_void;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::AsRawFd;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;

use crate::hid::DeviceInfo;
use crate::protocol::{APP_IDENTIFIER, AppMessage, DeviceSummary, HelperMessage};
use crate::sys::*;

/// Outgoing messages buffered per client before it is treated as stalled and dropped.
const CLIENT_QUEUE_DEPTH: usize = 256;
const SOL_LOCAL: libc::c_int = 0;
const LOCAL_PEERTOKEN: libc::c_int = 6;

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
      pointer: info.pointer,
    }
  }
}

/// Connection lifecycle and requests delivered to the helper core.
pub enum ClientEvent {
  /// `uid` is the peer's user; capture follows that user only while it owns the console.
  Connected {
    id: u64,
    uid: u32,
    sender: ClientSender,
  },
  Message {
    id: u64,
    message: AppMessage,
  },
  Disconnected {
    id: u64,
  },
  Rejected {
    reason: String,
  },
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
  let mut uid: libc::uid_t = u32::MAX;
  let mut gid: libc::gid_t = 0;
  // SAFETY: Out-parameters are valid; the descriptor is a connected Unix socket.
  unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
  deliver(ClientEvent::Connected { id, uid, sender: ClientSender { tx } });
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
