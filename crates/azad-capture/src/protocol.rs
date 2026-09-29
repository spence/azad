//! Messages between the capture helper and the Azad app, as JSON lines on a Unix socket.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::policy::{KeyAction, KeyContext};

pub const SOCKET_PATH: &str = "/var/run/ai.azad.capture.sock";
pub const APP_IDENTIFIER: &str = "ai.azad";
pub const PROTOCOL_VERSION: u32 = 1;

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
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(250);
/// A context not renewed for this long is treated as coming from a hung app.
pub const CONTEXT_LEASE: Duration = Duration::from_millis(1500);

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
