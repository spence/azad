use std::ffi::c_void;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait};
use crossbeam_channel::{Receiver, Sender, TryRecvError, select, unbounded};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceInfo {
  pub id: String,
  pub name: String,
  pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStateSnapshot {
  pub devices: Vec<InputDeviceInfo>,
  pub current_id: Option<String>,
  pub preferred_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DeviceEvent {
  StateChanged(DeviceStateSnapshot),
  Error(String),
}

pub trait DeviceEventSink: Send + Sync {
  fn on_event(&self, event: DeviceEvent);
}

pub trait DeviceControllerHandle: Send + Sync {
  fn snapshot(&self) -> Result<DeviceStateSnapshot>;
  fn set_preferred(&self, preferred_id: Option<String>) -> Result<()>;
  fn select_current(&self, id: String) -> Result<()>;
  fn refresh_now(&self) -> Result<()>;
  fn shutdown(&self) -> Result<()>;
}

#[derive(Debug, Clone, Copy)]
pub struct DeviceControllerConfig {
  pub enable_safety_poll: bool,
  pub safety_poll_interval: Duration,
  pub watcher_event_coalescing: bool,
  pub watcher_max_drain: usize,
}

impl Default for DeviceControllerConfig {
  fn default() -> Self {
    Self {
      enable_safety_poll: true,
      safety_poll_interval: Duration::from_secs(60),
      watcher_event_coalescing: true,
      watcher_max_drain: 1024,
    }
  }
}

pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>> {
  let host = cpal::default_host();
  let default_id = host.default_input_device().and_then(|d| d.id().ok()).map(|id| id.to_string());

  let mut out = Vec::new();
  for device in host.input_devices().context("failed to enumerate input devices")? {
    let id = device
      .id()
      .map(|id| id.to_string())
      .map_err(|e| anyhow!("failed to obtain input device id: {e}"))?;
    let desc = device
      .description()
      .map_err(|e| anyhow!("failed to obtain input device description: {e}"))?;
    out.push(InputDeviceInfo {
      is_default: default_id.as_deref() == Some(id.as_str()),
      id,
      name: desc.name().to_string(),
    });
  }

  Ok(out)
}

pub fn default_input_device_id() -> Result<Option<String>> {
  let host = cpal::default_host();
  let id = host
    .default_input_device()
    .map(|d| {
      d.id()
        .map(|id| id.to_string())
        .map_err(|e| anyhow!("failed to read default input device id: {e}"))
    })
    .transpose()?;
  Ok(id)
}

pub fn resolve_device_name(id: &str) -> Result<Option<String>> {
  let device = lookup_input_device_by_id(id)?;
  let Some(device) = device else {
    return Ok(None);
  };
  let desc = device
    .description()
    .map_err(|e| anyhow!("failed to read device description: {e}"))?;
  Ok(Some(desc.name().to_string()))
}

pub fn validate_device_exists(id: &str) -> Result<bool> {
  Ok(lookup_input_device_by_id(id)?.is_some())
}

pub fn open_input_device_by_id(id: &str) -> Result<cpal::Device> {
  let device =
    lookup_input_device_by_id(id)?.ok_or_else(|| anyhow!("input device not found for id: {id}"))?;
  if !device.supports_input() {
    return Err(anyhow!("device exists but does not support input: {id}"));
  }
  Ok(device)
}

pub fn open_default_input_device() -> Result<cpal::Device> {
  let host = cpal::default_host();
  host
    .default_input_device()
    .ok_or_else(|| anyhow!("no default input device available"))
}

pub fn start_device_controller(
  initial_preferred: Option<String>,
  sink: Arc<dyn DeviceEventSink>,
) -> Result<Arc<dyn DeviceControllerHandle>> {
  start_device_controller_with_config(initial_preferred, sink, DeviceControllerConfig::default())
}

pub fn start_device_controller_with_config(
  initial_preferred: Option<String>,
  sink: Arc<dyn DeviceEventSink>,
  config: DeviceControllerConfig,
) -> Result<Arc<dyn DeviceControllerHandle>> {
  let (cmd_tx, cmd_rx) = unbounded::<Command>();

  std::thread::spawn(move || run_controller(cmd_rx, sink, initial_preferred, config));

  let handle: Arc<dyn DeviceControllerHandle> = Arc::new(LiveDeviceControllerHandle { cmd_tx });
  Ok(handle)
}

struct LiveDeviceControllerHandle {
  cmd_tx: Sender<Command>,
}

impl DeviceControllerHandle for LiveDeviceControllerHandle {
  fn snapshot(&self) -> Result<DeviceStateSnapshot> {
    let (tx, rx) = unbounded();
    self
      .cmd_tx
      .send(Command::Snapshot(tx))
      .context("device controller not running")?;
    rx.recv_timeout(Duration::from_secs(5))
      .context("device controller snapshot timed out")?
  }

  fn set_preferred(&self, preferred_id: Option<String>) -> Result<()> {
    let (tx, rx) = unbounded();
    self
      .cmd_tx
      .send(Command::SetPreferred { preferred_id, resp: tx })
      .context("device controller not running")?;
    rx.recv().context("device controller dropped set_preferred")?
  }

  fn select_current(&self, id: String) -> Result<()> {
    let (tx, rx) = unbounded();
    self
      .cmd_tx
      .send(Command::SelectCurrent { id, resp: tx })
      .context("device controller not running")?;
    rx.recv().context("device controller dropped select_current")?
  }

  fn refresh_now(&self) -> Result<()> {
    self.cmd_tx.send(Command::Refresh).context("device controller not running")
  }

  fn shutdown(&self) -> Result<()> {
    let (tx, rx) = unbounded();
    self
      .cmd_tx
      .send(Command::Shutdown(tx))
      .context("device controller not running")?;
    rx.recv().context("device controller dropped shutdown")?
  }
}

#[derive(Debug)]
enum Command {
  Refresh,
  Snapshot(Sender<Result<DeviceStateSnapshot>>),
  SetPreferred { preferred_id: Option<String>, resp: Sender<Result<()>> },
  SelectCurrent { id: String, resp: Sender<Result<()>> },
  Shutdown(Sender<Result<()>>),
}

#[derive(Debug, Clone)]
struct ControllerState {
  preferred_id: Option<String>,
  last_snapshot: Option<DeviceStateSnapshot>,
  last_topology_fingerprint: Option<TopologyFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TopologyFingerprint {
  input_device_ids: Vec<String>,
  default_input_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum RefreshReason {
  Startup,
  ManualRefresh,
  SetPreferred,
  SelectCurrent,
  Watcher,
  SafetyPoll,
}

impl RefreshReason {
  fn as_str(self) -> &'static str {
    match self {
      Self::Startup => "startup",
      Self::ManualRefresh => "manual_refresh",
      Self::SetPreferred => "set_preferred",
      Self::SelectCurrent => "select_current",
      Self::Watcher => "watcher",
      Self::SafetyPoll => "safety_poll",
    }
  }
}

fn run_controller(
  cmd_rx: Receiver<Command>,
  sink: Arc<dyn DeviceEventSink>,
  initial_preferred: Option<String>,
  config: DeviceControllerConfig,
) {
  let mut state = ControllerState {
    preferred_id: initial_preferred,
    last_snapshot: None,
    last_topology_fingerprint: None,
  };

  eprintln!(
    "asr devices: controller startup mode=event-driven safety_poll_enabled={} safety_poll_interval_s={} watcher_event_coalescing={} watcher_max_drain={}",
    config.enable_safety_poll,
    config.safety_poll_interval.as_secs(),
    config.watcher_event_coalescing,
    config.watcher_max_drain
  );

  if let Err(err) = refresh_and_emit(&mut state, &*sink, RefreshReason::Startup) {
    sink.on_event(DeviceEvent::Error(err.to_string()));
  }

  // Install the CoreAudio topology watcher after the initial refresh so a
  // slow HAL initialization doesn't block the first device snapshot.
  let (watch_tx, watch_rx) = unbounded::<()>();
  let _watcher = match TopologyWatcher::install(watch_tx) {
    Ok(watcher) => Some(watcher),
    Err(err) => {
      sink.on_event(DeviceEvent::Error(format!(
        "failed to install device topology watcher; using polling fallback: {err}"
      )));
      None
    }
  };

  loop {
    select! {
        recv(cmd_rx) -> msg => {
            let Some(cmd) = msg.ok() else {
                break;
            };
            match cmd {
                Command::Refresh => {
                    if let Err(err) = refresh_and_emit(&mut state, &*sink, RefreshReason::ManualRefresh) {
                        sink.on_event(DeviceEvent::Error(err.to_string()));
                    }
                }
                Command::Snapshot(resp) => {
                    let out = state
                        .last_snapshot
                        .clone()
                        .ok_or_else(|| anyhow!("device controller has no snapshot yet"));
                    let _ = resp.send(out);
                }
                Command::SetPreferred { preferred_id, resp } => {
                    state.preferred_id = preferred_id;
                    let out = refresh_and_emit(&mut state, &*sink, RefreshReason::SetPreferred).map(|_| ());
                    let _ = resp.send(out);
                }
                Command::SelectCurrent { id, resp } => {
                    let exists = state
                        .last_snapshot
                        .as_ref()
                        .map(|snap| snap.devices.iter().any(|d| d.id == id))
                        .unwrap_or(false);
                    if !exists {
                        let out = Err(anyhow!("selected device id is not currently available: {id}"));
                        let _ = resp.send(out);
                        continue;
                    }

                    state.preferred_id = Some(id);
                    let out = refresh_and_emit(&mut state, &*sink, RefreshReason::SelectCurrent).map(|_| ());
                    let _ = resp.send(out);
                }
                Command::Shutdown(resp) => {
                    let _ = resp.send(Ok(()));
                    break;
                }
            }
        }
        recv(watch_rx) -> msg => {
            if msg.is_err() {
                break;
            }
            let coalesced = if config.watcher_event_coalescing {
                drain_watcher_events(&watch_rx, config.watcher_max_drain)
            } else {
                0
            };
            if coalesced > 0 {
                eprintln!(
                    "asr devices: watcher burst coalesced={} max_drain={}",
                    coalesced,
                    config.watcher_max_drain
                );
            }
            if let Err(err) = refresh_and_emit(&mut state, &*sink, RefreshReason::Watcher) {
                sink.on_event(DeviceEvent::Error(err.to_string()));
            }
        }
        default(config.safety_poll_interval) => {
            if !config.enable_safety_poll {
                continue;
            }
            if let Err(err) = refresh_if_topology_drifted(&mut state, &*sink) {
                sink.on_event(DeviceEvent::Error(err.to_string()));
            }
        }
    }
  }
}

fn refresh_if_topology_drifted(
  state: &mut ControllerState,
  sink: &dyn DeviceEventSink,
) -> Result<()> {
  let next = collect_topology_fingerprint()?;
  let unchanged = state.last_topology_fingerprint.as_ref().is_some_and(|prev| prev == &next);
  if unchanged {
    return Ok(());
  }

  eprintln!("asr devices: safety poll detected topology drift; forcing refresh");
  refresh_and_emit(state, sink, RefreshReason::SafetyPoll)?;
  Ok(())
}

fn refresh_and_emit(
  state: &mut ControllerState,
  sink: &dyn DeviceEventSink,
  reason: RefreshReason,
) -> Result<bool> {
  let refresh_start = Instant::now();
  let devices = list_input_devices()?;
  let observed_default = default_input_device_id()?;

  let preferred_available = state
    .preferred_id
    .as_ref()
    .filter(|pid| devices.iter().any(|d| d.id == **pid))
    .cloned();

  let default_available = observed_default
    .as_ref()
    .filter(|id| devices.iter().any(|d| d.id == id.as_str()))
    .cloned();

  let desired_current = preferred_available
    .or_else(|| default_available.clone())
    .or_else(|| devices.first().map(|d| d.id.clone()));
  // Current device is app-local selection logic (preferred -> OS default -> first available).
  // Do not mutate the operating system's default input device here.
  let current_id = desired_current;

  let snapshot =
    DeviceStateSnapshot { devices, current_id, preferred_id: state.preferred_id.clone() };

  let mut input_device_ids = snapshot.devices.iter().map(|d| d.id.clone()).collect::<Vec<_>>();
  input_device_ids.sort();
  state.last_topology_fingerprint =
    Some(TopologyFingerprint { input_device_ids, default_input_id: observed_default });

  let changed = state.last_snapshot.as_ref().map(|prev| prev != &snapshot).unwrap_or(true);

  if changed {
    state.last_snapshot = Some(snapshot.clone());
    sink.on_event(DeviceEvent::StateChanged(snapshot));
  }

  eprintln!(
    "asr devices: refresh reason={} changed={} duration_ms={}",
    reason.as_str(),
    changed,
    refresh_start.elapsed().as_millis()
  );

  Ok(changed)
}

fn collect_topology_fingerprint() -> Result<TopologyFingerprint> {
  let mut input_device_ids = collect_input_device_ids()?;
  input_device_ids.sort();
  Ok(TopologyFingerprint { input_device_ids, default_input_id: default_input_device_id()? })
}

fn collect_input_device_ids() -> Result<Vec<String>> {
  let host = cpal::default_host();
  let mut ids = Vec::new();
  for device in host
    .input_devices()
    .context("failed to enumerate input devices for topology fingerprint")?
  {
    let id = device
      .id()
      .map(|id| id.to_string())
      .map_err(|e| anyhow!("failed to obtain input device id for fingerprint: {e}"))?;
    ids.push(id);
  }
  Ok(ids)
}

fn drain_watcher_events(watch_rx: &Receiver<()>, max_drain: usize) -> usize {
  if max_drain == 0 {
    return 0;
  }

  let mut drained = 0usize;
  while drained < max_drain {
    match watch_rx.try_recv() {
      Ok(()) => drained += 1,
      Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
    }
  }
  drained
}

fn lookup_input_device_by_id(id: &str) -> Result<Option<cpal::Device>> {
  let parsed_id = cpal::DeviceId::from_str(id).context("invalid device id format")?;
  let host = cpal::platform::host_from_id(parsed_id.0)
    .map_err(|e| anyhow!("failed to open host for device id: {e}"))?;

  let Some(device) = host.device_by_id(&parsed_id) else {
    return Ok(None);
  };
  if !device.supports_input() {
    return Ok(None);
  }
  Ok(Some(device))
}

#[cfg(target_os = "macos")]
struct TopologyWatcher {
  tx_ptr: *mut Sender<()>,
  addrs: [coreaudio_sys::AudioObjectPropertyAddress; 2],
  installed: usize,
}

#[cfg(target_os = "macos")]
impl TopologyWatcher {
  fn install(tx: Sender<()>) -> Result<Self> {
    let tx_ptr = Box::into_raw(Box::new(tx));
    let addrs = [
      coreaudio_sys::AudioObjectPropertyAddress {
        mSelector: coreaudio_sys::kAudioHardwarePropertyDevices,
        mScope: coreaudio_sys::kAudioObjectPropertyScopeGlobal,
        mElement: coreaudio_sys::kAudioObjectPropertyElementMain,
      },
      coreaudio_sys::AudioObjectPropertyAddress {
        mSelector: coreaudio_sys::kAudioHardwarePropertyDefaultInputDevice,
        mScope: coreaudio_sys::kAudioObjectPropertyScopeGlobal,
        mElement: coreaudio_sys::kAudioObjectPropertyElementMain,
      },
    ];

    let mut installed = 0usize;
    for addr in &addrs {
      let status = unsafe {
        coreaudio_sys::AudioObjectAddPropertyListener(
          coreaudio_sys::kAudioObjectSystemObject,
          addr,
          Some(on_audio_topology_changed),
          tx_ptr.cast::<c_void>(),
        )
      };
      if status != 0 {
        unsafe {
          for remove_addr in addrs.iter().take(installed) {
            let _ = coreaudio_sys::AudioObjectRemovePropertyListener(
              coreaudio_sys::kAudioObjectSystemObject,
              remove_addr,
              Some(on_audio_topology_changed),
              tx_ptr.cast::<c_void>(),
            );
          }
          drop(Box::from_raw(tx_ptr));
        }
        return Err(anyhow!("AudioObjectAddPropertyListener failed with status {}", status));
      }
      installed += 1;
    }

    Ok(Self { tx_ptr, addrs, installed })
  }
}

#[cfg(target_os = "macos")]
impl Drop for TopologyWatcher {
  fn drop(&mut self) {
    unsafe {
      for addr in self.addrs.iter().take(self.installed) {
        let _ = coreaudio_sys::AudioObjectRemovePropertyListener(
          coreaudio_sys::kAudioObjectSystemObject,
          addr,
          Some(on_audio_topology_changed),
          self.tx_ptr.cast::<c_void>(),
        );
      }
      drop(Box::from_raw(self.tx_ptr));
    }
  }
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn on_audio_topology_changed(
  _object_id: coreaudio_sys::AudioObjectID,
  _num_addresses: coreaudio_sys::UInt32,
  _addresses: *const coreaudio_sys::AudioObjectPropertyAddress,
  client_data: *mut c_void,
) -> coreaudio_sys::OSStatus {
  if client_data.is_null() {
    return 0;
  }

  let tx = unsafe { &*(client_data as *const Sender<()>) };
  let _ = tx.try_send(());
  0
}

#[cfg(not(target_os = "macos"))]
struct TopologyWatcher;

#[cfg(not(target_os = "macos"))]
impl TopologyWatcher {
  fn install(_tx: Sender<()>) -> Result<Self> {
    Ok(Self)
  }
}

#[cfg(test)]
mod tests {
  use super::{DeviceControllerConfig, TopologyFingerprint, drain_watcher_events};
  use crossbeam_channel::unbounded;
  use std::time::Duration;

  #[test]
  fn default_device_controller_config_is_slow_safety_poll() {
    let cfg = DeviceControllerConfig::default();
    assert!(cfg.enable_safety_poll);
    assert_eq!(cfg.safety_poll_interval, Duration::from_secs(60));
    assert!(cfg.watcher_event_coalescing);
    assert_eq!(cfg.watcher_max_drain, 1024);
  }

  #[test]
  fn drain_watcher_events_coalesces_up_to_max() {
    let (tx, rx) = unbounded();
    for _ in 0..10 {
      tx.send(()).unwrap();
    }

    let drained = drain_watcher_events(&rx, 4);
    assert_eq!(drained, 4);
    // 6 remain queued.
    assert_eq!(drain_watcher_events(&rx, 10), 6);
  }

  #[test]
  fn drain_watcher_events_noop_when_max_is_zero() {
    let (tx, rx) = unbounded();
    tx.send(()).unwrap();
    assert_eq!(drain_watcher_events(&rx, 0), 0);
    assert_eq!(drain_watcher_events(&rx, 10), 1);
  }

  #[test]
  fn topology_fingerprint_equality_tracks_default_and_ids() {
    let a = TopologyFingerprint {
      input_device_ids: vec!["a".into(), "b".into()],
      default_input_id: Some("a".into()),
    };
    let b = TopologyFingerprint {
      input_device_ids: vec!["a".into(), "b".into()],
      default_input_id: Some("a".into()),
    };
    let c = TopologyFingerprint {
      input_device_ids: vec!["a".into(), "b".into()],
      default_input_id: Some("b".into()),
    };
    let d = TopologyFingerprint {
      input_device_ids: vec!["a".into(), "c".into()],
      default_input_id: Some("a".into()),
    };

    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
  }
}
