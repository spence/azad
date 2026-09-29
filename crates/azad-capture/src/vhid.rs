//! Client for the Karabiner DriverKit VirtualHIDDevice service (client protocol 7).
//!
//! The service is a root daemon that owns the signed virtual keyboard driver. Each client
//! connection gets its own virtual keyboard. Frames are a big-endian `u32` body length, a message
//! type byte, an optional big-endian `u64` request id, then the payload. Request payloads start
//! with the little-endian protocol version and a request byte followed by the raw report struct.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::engine::{ForwardState, PointerReport};

pub const CLIENT_PROTOCOL_VERSION: u16 = 7;
pub const SERVER_SOCKET_PATH: &str =
  "/Library/Application Support/org.pqrs/tmp/rootonly/karabiner_virtual_hid_device_service.sock";
/// Largest payload the service accepts.
pub const MAX_MESSAGE_SIZE: usize = 1024;
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);

/// Vendor ID the Karabiner driver uses for remapper-owned virtual keyboards by default.
pub const DEFAULT_REMAPPER_VENDOR_ID: u32 = 0x16c0;
pub const DEFAULT_REMAPPER_PRODUCT_ID: u32 = 0x27db;
/// Identity of Azad's own virtual output keyboard.
pub const AZAD_OUTPUT_VENDOR_ID: u32 = 0xfeed;
pub const AZAD_OUTPUT_PRODUCT_ID: u32 = 0xa2ad;
pub const VIRTUAL_KEYBOARD_MANUFACTURER: &str = "pqrs.org";
pub const VIRTUAL_KEYBOARD_PRODUCT_PREFIX: &str = "Karabiner DriverKit VirtualHIDKeyboard";

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
  Heartbeat = 0,
  UserData = 1,
  HealthCheck = 2,
  HealthCheckResponse = 3,
  Request = 4,
  Response = 5,
}

impl MessageType {
  fn from_byte(value: u8) -> Option<Self> {
    Some(match value {
      0 => Self::Heartbeat,
      1 => Self::UserData,
      2 => Self::HealthCheck,
      3 => Self::HealthCheckResponse,
      4 => Self::Request,
      5 => Self::Response,
      _ => return None,
    })
  }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
  VirtualHidKeyboardInitialize = 0,
  VirtualHidKeyboardTerminate = 1,
  VirtualHidKeyboardReset = 2,
  VirtualHidPointingInitialize = 3,
  PostKeyboardInputReport = 6,
  PostConsumerInputReport = 7,
  PostAppleVendorKeyboardInputReport = 8,
  PostAppleVendorTopCaseInputReport = 9,
  PostGenericDesktopInputReport = 10,
  PostPointingInputReport = 11,
}

/// Service state reported through response pairs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServiceState {
  pub connected: bool,
  pub driver_activated: Option<bool>,
  pub driver_connected: Option<bool>,
  pub driver_version_mismatched: Option<bool>,
  pub keyboard_ready: Option<bool>,
  pub pointing_ready: Option<bool>,
}

impl ServiceState {
  /// True only when the service confirmed a usable virtual keyboard for this client.
  pub fn can_forward(&self) -> bool {
    self.connected
      && self.driver_version_mismatched != Some(true)
      && self.driver_activated != Some(false)
      && self.keyboard_ready == Some(true)
  }

  fn apply_pairs(&mut self, payload: &[u8]) -> bool {
    if payload.len() % 2 != 0 {
      return false;
    }
    for pair in payload.chunks_exact(2) {
      let value = Some(pair[1] != 0);
      match pair[0] {
        1 => self.driver_activated = value,
        2 => self.driver_connected = value,
        3 => self.driver_version_mismatched = value,
        4 => self.keyboard_ready = value,
        5 => self.pointing_ready = value,
        _ => {}
      }
    }
    true
  }
}

pub fn encode_frame(kind: MessageType, request_id: Option<u64>, payload: &[u8]) -> Vec<u8> {
  let id_len = if request_id.is_some() { 8 } else { 0 };
  let body = 1 + id_len + payload.len();
  let mut frame = Vec::with_capacity(4 + body);
  frame.extend_from_slice(&(body as u32).to_be_bytes());
  frame.push(kind as u8);
  if let Some(id) = request_id {
    frame.extend_from_slice(&id.to_be_bytes());
  }
  frame.extend_from_slice(payload);
  frame
}

pub fn request_payload(request: Request, data: &[u8]) -> Vec<u8> {
  let mut payload = Vec::with_capacity(3 + data.len());
  payload.extend_from_slice(&CLIENT_PROTOCOL_VERSION.to_le_bytes());
  payload.push(request as u8);
  payload.extend_from_slice(data);
  payload
}

/// `virtual_hid_keyboard_parameters`: vendor, product, and country code as native `u64`s.
pub fn keyboard_parameters(vendor_id: u32, product_id: u32, country_code: u32) -> [u8; 24] {
  let mut data = [0u8; 24];
  data[0..8].copy_from_slice(&(vendor_id as u64).to_le_bytes());
  data[8..16].copy_from_slice(&(product_id as u64).to_le_bytes());
  data[16..24].copy_from_slice(&(country_code as u64).to_le_bytes());
  data
}

fn put_keys(out: &mut [u8], keys: &[u16]) {
  for (slot, key) in out.chunks_exact_mut(2).zip(keys) {
    slot.copy_from_slice(&key.to_le_bytes());
  }
}

/// `hid_report::keyboard_input`: report id 1, modifier bits, reserved byte, 32 `u16` usages.
pub fn keyboard_report(modifiers: u8, keys: &[u16]) -> [u8; 67] {
  let mut report = [0u8; 67];
  report[0] = 1;
  report[1] = modifiers;
  put_keys(&mut report[3..], keys);
  report
}

/// `hid_report::pointing_input`: button bits, then x, y, vertical and horizontal wheel as `i8`.
pub fn pointing_report(buttons: u32, x: i8, y: i8, wheel: i8, horizontal_wheel: i8) -> [u8; 8] {
  let mut report = [0u8; 8];
  report[0..4].copy_from_slice(&buttons.to_le_bytes());
  report[4] = x as u8;
  report[5] = y as u8;
  report[6] = wheel as u8;
  report[7] = horizontal_wheel as u8;
  report
}

/// Consumer (id 2), Apple top case (id 3) and Apple keyboard (id 4) reports: 32 `u16` usages.
pub fn keys_report(report_id: u8, keys: &[u16]) -> [u8; 65] {
  let mut report = [0u8; 65];
  report[0] = report_id;
  put_keys(&mut report[1..], keys);
  report
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
  pub kind: MessageType,
  pub request_id: Option<u64>,
  pub payload: Vec<u8>,
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
  let mut header = [0u8; 4];
  reader.read_exact(&mut header)?;
  let body = u32::from_be_bytes(header) as usize;
  if !(1..=MAX_MESSAGE_SIZE + 9).contains(&body) {
    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame size out of range"));
  }
  let mut bytes = vec![0u8; body];
  reader.read_exact(&mut bytes)?;
  let kind = MessageType::from_byte(bytes[0])
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown frame type"))?;
  let (request_id, payload) = match kind {
    MessageType::Request | MessageType::Response => {
      if bytes.len() < 9 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short request frame"));
      }
      let id = u64::from_be_bytes(bytes[1..9].try_into().expect("slice length"));
      (Some(id), bytes[9..].to_vec())
    }
    _ => (None, bytes[1..].to_vec()),
  };
  Ok(Frame { kind, request_id, payload })
}

/// Notified from the reader thread whenever the service state changes.
pub type StateCallback = Box<dyn Fn(ServiceState) + Send + Sync>;

/// One connection to the service, owning one virtual keyboard.
pub struct Client {
  writer: Mutex<UnixStream>,
  next_request_id: AtomicU64,
  state: Arc<Mutex<ServiceState>>,
}

impl Client {
  /// Connects, starts heartbeat and reader threads, and requests Azad's virtual keyboard.
  pub fn connect(path: &str, on_state: StateCallback) -> io::Result<Arc<Client>> {
    Self::connect_keyboard(path, AZAD_OUTPUT_VENDOR_ID, AZAD_OUTPUT_PRODUCT_ID, 0, on_state)
  }

  /// Connects and requests a virtual keyboard with the given identity.
  pub fn connect_keyboard(
    path: &str,
    vendor_id: u32,
    product_id: u32,
    country_code: u32,
    on_state: StateCallback,
  ) -> io::Result<Arc<Client>> {
    let stream = UnixStream::connect(path)?;
    let reader = stream.try_clone()?;
    let state = Arc::new(Mutex::new(ServiceState { connected: true, ..ServiceState::default() }));
    let client = Arc::new(Client {
      writer: Mutex::new(stream),
      next_request_id: AtomicU64::new(1),
      state: state.clone(),
    });
    let on_state: Arc<StateCallback> = Arc::new(on_state);

    let heartbeat = Arc::downgrade(&client);
    thread::Builder::new().name("azad-vhid-heartbeat".into()).spawn(move || {
      while let Some(client) = heartbeat.upgrade() {
        if client.write_frame(&encode_frame(MessageType::Heartbeat, None, &[])).is_err() {
          break;
        }
        drop(client);
        thread::sleep(HEARTBEAT_INTERVAL);
      }
    })?;

    let responder = Arc::downgrade(&client);
    let reader_state = state.clone();
    let reader_callback = on_state.clone();
    thread::Builder::new().name("azad-vhid-reader".into()).spawn(move || {
      let mut reader = reader;
      while let Ok(frame) = read_frame(&mut reader) {
        let changed = match frame.kind {
          MessageType::Request | MessageType::Response => {
            if frame.kind == MessageType::Request
              && let (Some(client), Some(id)) = (responder.upgrade(), frame.request_id)
            {
              let _ = client.write_frame(&encode_frame(MessageType::Response, Some(id), &[]));
            }
            let mut state = reader_state.lock().expect("vhid state");
            let before = *state;
            state.apply_pairs(&frame.payload);
            (*state != before).then_some(*state)
          }
          _ => None,
        };
        if let Some(state) = changed {
          reader_callback(state);
        }
      }
      let mut state = reader_state.lock().expect("vhid state");
      *state = ServiceState::default();
      let snapshot = *state;
      drop(state);
      reader_callback(snapshot);
    })?;

    let params = keyboard_parameters(vendor_id, product_id, country_code);
    client.request(Request::VirtualHidKeyboardInitialize, &params)?;
    Ok(client)
  }

  pub fn state(&self) -> ServiceState {
    *self.state.lock().expect("vhid state")
  }

  pub fn request(&self, request: Request, data: &[u8]) -> io::Result<()> {
    let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
    self.write_frame(&encode_frame(MessageType::Request, Some(id), &request_payload(request, data)))
  }

  /// Posts every report whose page changed between `previous` and `next`.
  pub fn post_state(&self, previous: &ForwardState, next: &ForwardState) -> io::Result<()> {
    if previous.modifiers != next.modifiers || previous.keyboard != next.keyboard {
      self.request(
        Request::PostKeyboardInputReport,
        &keyboard_report(next.modifiers, &next.keyboard),
      )?;
    }
    if previous.consumer != next.consumer {
      self.request(Request::PostConsumerInputReport, &keys_report(2, &next.consumer))?;
    }
    if previous.apple_top_case != next.apple_top_case {
      self.request(
        Request::PostAppleVendorTopCaseInputReport,
        &keys_report(3, &next.apple_top_case),
      )?;
    }
    if previous.generic_desktop != next.generic_desktop {
      self
        .request(Request::PostGenericDesktopInputReport, &keys_report(7, &next.generic_desktop))?;
    }
    if previous.apple_keyboard != next.apple_keyboard {
      self.request(
        Request::PostAppleVendorKeyboardInputReport,
        &keys_report(4, &next.apple_keyboard),
      )?;
    }
    Ok(())
  }

  /// Creates this connection's virtual pointing device, for keyboards with a pointer collection.
  pub fn initialize_pointing(&self) -> io::Result<()> {
    self.request(Request::VirtualHidPointingInitialize, &[])
  }

  /// Posts a pointer report, split so each motion component fits the report's `i8` range.
  pub fn post_pointer(&self, report: &PointerReport) -> io::Result<()> {
    let mut remaining = *report;
    loop {
      let x = remaining.x.clamp(-127, 127);
      let y = remaining.y.clamp(-127, 127);
      let wheel = remaining.wheel.clamp(-127, 127);
      let horizontal = remaining.horizontal_wheel.clamp(-127, 127);
      self.request(
        Request::PostPointingInputReport,
        &pointing_report(report.buttons, x as i8, y as i8, wheel as i8, horizontal as i8),
      )?;
      remaining.x -= x;
      remaining.y -= y;
      remaining.wheel -= wheel;
      remaining.horizontal_wheel -= horizontal;
      if remaining.x == 0
        && remaining.y == 0
        && remaining.wheel == 0
        && remaining.horizontal_wheel == 0
      {
        return Ok(());
      }
    }
  }

  /// Posts empty reports on every page so no forwarded key stays pressed.
  pub fn release_all(&self) -> io::Result<()> {
    self.request(Request::PostKeyboardInputReport, &keyboard_report(0, &[]))?;
    self.request(Request::PostConsumerInputReport, &keys_report(2, &[]))?;
    self.request(Request::PostAppleVendorTopCaseInputReport, &keys_report(3, &[]))?;
    self.request(Request::PostGenericDesktopInputReport, &keys_report(7, &[]))?;
    self.request(Request::PostAppleVendorKeyboardInputReport, &keys_report(4, &[]))
  }

  pub fn shutdown(&self) {
    let _ = self.release_all();
    let _ = self.request(Request::VirtualHidKeyboardTerminate, &[]);
    if let Ok(stream) = self.writer.lock() {
      let _ = stream.shutdown(std::net::Shutdown::Both);
    }
  }

  fn write_frame(&self, frame: &[u8]) -> io::Result<()> {
    self.writer.lock().expect("vhid writer").write_all(frame)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
  }

  #[test]
  fn encoding_matches_pinned_driver_headers() {
    let reference = include_str!("../reference/vhid_reference.txt");
    let expected = |name: &str| {
      reference
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("missing {name}"))
        .to_string()
    };
    let init = request_payload(
      Request::VirtualHidKeyboardInitialize,
      &keyboard_parameters(AZAD_OUTPUT_VENDOR_ID, AZAD_OUTPUT_PRODUCT_ID, 0),
    );
    assert_eq!(hex(&init), expected("keyboard_initialize"));
    assert_eq!(
      hex(&encode_frame(MessageType::Request, Some(0x0102030405060708), &init)),
      expected("keyboard_initialize_frame")
    );
    assert_eq!(
      hex(&request_payload(
        Request::PostKeyboardInputReport,
        &keyboard_report(0x04, &[0x2c, 0x152])
      )),
      expected("keyboard_input")
    );
    assert_eq!(
      hex(&request_payload(Request::PostConsumerInputReport, &keys_report(2, &[0xe9]))),
      expected("consumer_input")
    );
    assert_eq!(
      hex(&request_payload(Request::PostAppleVendorTopCaseInputReport, &keys_report(3, &[0x03]))),
      expected("apple_top_case_input")
    );
    assert_eq!(
      hex(&request_payload(Request::PostAppleVendorKeyboardInputReport, &keys_report(4, &[0x01]))),
      expected("apple_keyboard_input")
    );
    assert_eq!(
      hex(&request_payload(Request::PostGenericDesktopInputReport, &keys_report(7, &[0x9b]))),
      expected("generic_desktop_input")
    );
    assert_eq!(
      hex(&request_payload(Request::VirtualHidKeyboardTerminate, &[])),
      expected("keyboard_terminate")
    );
    assert_eq!(
      hex(&request_payload(
        Request::PostPointingInputReport,
        &pointing_report(0b101, -5, 7, -1, 2)
      )),
      expected("pointing_input")
    );
    assert_eq!(
      hex(&request_payload(Request::VirtualHidPointingInitialize, &[])),
      expected("pointing_initialize")
    );
    assert_eq!(hex(&encode_frame(MessageType::Heartbeat, None, &[])), expected("heartbeat_frame"));
    assert_eq!(
      hex(&encode_frame(MessageType::Response, Some(42), &[])),
      expected("response_frame")
    );
  }

  #[test]
  fn heartbeat_frame_is_length_and_type() {
    assert_eq!(encode_frame(MessageType::Heartbeat, None, &[]), vec![0, 0, 0, 1, 0]);
  }

  #[test]
  fn request_frame_carries_big_endian_id_and_little_endian_version() {
    let frame = encode_frame(
      MessageType::Request,
      Some(0x0102),
      &request_payload(Request::VirtualHidKeyboardReset, &[]),
    );
    assert_eq!(frame, vec![0, 0, 0, 12, 4, 0, 0, 0, 0, 0, 0, 1, 2, 7, 0, 2]);
  }

  #[test]
  fn keyboard_report_layout_matches_driver_struct() {
    let report = keyboard_report(0x04, &[0x2C, 0x0152]);
    assert_eq!(report.len(), 67);
    assert_eq!(&report[..7], &[1, 0x04, 0, 0x2C, 0, 0x52, 0x01]);
    assert!(report[7..].iter().all(|byte| *byte == 0));
  }

  #[test]
  fn parameters_are_three_native_u64s() {
    let data = keyboard_parameters(0x16c0, 0x27db, 0);
    assert_eq!(&data[..2], &[0xc0, 0x16]);
    assert_eq!(&data[8..10], &[0xdb, 0x27]);
    assert!(data[16..].iter().all(|byte| *byte == 0));
  }

  #[test]
  fn read_frame_round_trips_request() {
    let bytes = encode_frame(MessageType::Request, Some(9), &[4, 1]);
    let frame = read_frame(&mut bytes.as_slice()).unwrap();
    assert_eq!(
      frame,
      Frame { kind: MessageType::Request, request_id: Some(9), payload: vec![4, 1] }
    );
  }

  #[test]
  fn service_state_requires_ready_keyboard_and_matching_driver() {
    let mut state = ServiceState { connected: true, ..ServiceState::default() };
    assert!(!state.can_forward());
    state.apply_pairs(&[1, 1, 2, 1, 3, 0, 4, 1]);
    assert!(state.can_forward());
    state.apply_pairs(&[3, 1]);
    assert!(!state.can_forward());
  }

  #[test]
  fn odd_response_payload_is_rejected() {
    let mut state = ServiceState::default();
    assert!(!state.apply_pairs(&[4]));
    assert_eq!(state.keyboard_ready, None);
  }
}
