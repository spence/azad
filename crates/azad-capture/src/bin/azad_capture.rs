//! Root helper: seizes keyboards, sends Azad's claimed shortcuts to the app, and forwards all
//! other input through the virtual keyboard.
//!
//! Capture is fail-open. Keyboards are seized only while the Input Monitoring permission is
//! granted, the virtual keyboard is ready, and an authenticated Azad client is connected. Losing
//! any of these closes every device, which returns input to the OS. The kernel also releases
//! seized devices when this process exits, so a watchdog exits if the main loop stalls.

use std::ffi::c_void;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use serde_json::json;

use azad_capture::engine::{BatchOutcome, Engine, ForwardState, PointerReport};
use azad_capture::hid::{DeviceEvent, DeviceState, Manager, Mode};
use azad_capture::ipc::{self, ClientEvent, ClientSender};
use azad_capture::policy::KeyContext;
use azad_capture::protocol::{
  self, AppMessage, CaptureStatus, DeviceSummary, DriverStatus, HelperMessage, HelperStatus,
  Permission,
};
use azad_capture::sys::*;
use azad_capture::vhid::{self, ServiceState};

const DRIVER_DAEMON_PATH: &str = "/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/Applications/Karabiner-VirtualHIDDevice-Daemon.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Daemon";
const TICK_SECONDS: f64 = 0.25;
/// A main loop silent this long while holding seized keyboards is treated as hung.
const WATCHDOG_LIMIT_NANOS: u64 = 1_000_000_000;
const WATCHDOG_EXIT_CODE: i32 = 75;
const FORWARD_RETRY_DELAY_NANOS: u64 = 3_000_000_000;
const EX_TEMPFAIL: i32 = 75;
const COUNTERS_EVERY_TICKS: u32 = 20;
const RESCAN_EVERY_TICKS: u32 = 20;
const ACCESS_PROBE_EVERY_TICKS: u32 = 8;
const UPDATE_CHECK_EVERY_TICKS: u32 = 20;

static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
static SEIZED: AtomicU64 = AtomicU64::new(0);
static DRIVER_GENERATION: AtomicU64 = AtomicU64::new(0);
static INBOX: OnceLock<Inbox> = OnceLock::new();

enum External {
  Client(ClientEvent),
  /// A driver connection's state, tagged with the connection generation that reported it.
  Driver(u64, ServiceState),
  Shutdown,
}

struct Inbox {
  queue: Mutex<Vec<External>>,
  source: usize,
  run_loop: usize,
}

impl Inbox {
  fn push(&self, event: External) {
    self.queue.lock().expect("inbox").push(event);
    // SAFETY: The source and run loop live for the process lifetime.
    unsafe {
      CFRunLoopSourceSignal(self.source as CFRunLoopSourceRef);
      CFRunLoopWakeUp(self.run_loop as CFRunLoopRef);
    }
  }
}

#[derive(Default)]
struct Counters {
  batches: u64,
  claimed_edges: u64,
  forwarded_edges: u64,
  actions: u64,
  forward_posts: u64,
  forward_errors: u64,
  rejected_clients: u64,
}

struct Core {
  manager: &'static mut Manager,
  engine: Engine,
  context: KeyContext,
  /// Capture stays off until then after a forwarding failure, so a persistently broken
  /// forwarding path does not re-seize keyboards and swallow keys.
  forward_retry_after: u64,
  mode: Mode,
  /// The driver connection generation whose virtual pointing device was requested.
  pointing_requested: Option<u64>,
  client_uid: u32,
  /// The console belongs to the client's user; false during fast user switching or at the
  /// login window, when that user's shortcuts must not claim someone else's typing.
  console_matches: bool,
  context_renewed: u64,
  lease_expired: bool,
  client: Option<(u64, ClientSender)>,
  driver: Arc<Mutex<Option<Arc<vhid::Client>>>>,
  driver_state: ServiceState,
  permission: Permission,
  access_requested: bool,
  last_status: Option<String>,
  counters: Counters,
  last_counters: Option<String>,
  last_posted: ForwardState,
  ticks: u32,
  /// Identity of the executable this process started from; a change means it was updated.
  executable: Option<(u64, i64)>,
}

impl Core {
  fn active(&self) -> bool {
    self.client.is_some()
      && self.console_matches
      && self.permission == Permission::Granted
      && self.driver_state.can_forward()
      && now_nanos() >= self.forward_retry_after
  }

  /// Before seizing anything, posts an all-up report to prove forwarding works; seizing with a
  /// broken forwarding path would swallow the next key.
  fn forwarding_healthy(&mut self) -> bool {
    let client = self.driver.lock().expect("driver").clone();
    let probe = match client {
      Some(_) if fault::forward_fails() => Err(std::io::Error::other("injected forward failure")),
      Some(client) => client.release_all(),
      None => Err(std::io::Error::other("driver not connected")),
    };
    if probe.is_err() {
      self.forwarding_failed();
    }
    probe.is_ok()
  }

  fn forwarding_failed(&mut self) {
    self.counters.forward_errors += 1;
    self.forward_retry_after = now_nanos() + FORWARD_RETRY_DELAY_NANOS;
    // Drop the connection so the next one re-creates the virtual keyboard.
    self.driver_state = ServiceState::default();
    if let Some(client) = self.driver.lock().expect("driver").take() {
      client.shutdown();
    }
  }

  fn check_console(&mut self) {
    use std::os::unix::fs::MetadataExt;
    let console = std::fs::metadata("/dev/console").map(|m| m.uid()).ok();
    let matches = self.client.is_some() && console == Some(self.client_uid);
    if matches != self.console_matches {
      self.console_matches = matches;
      if self.client.is_some() {
        log(json!({ "event": "console_session", "client_owns_console": matches }));
      }
    }
  }

  /// The app's context while its lease is current. A hung app keeps only the listen chord: its
  /// overlay or search context would otherwise keep claiming Enter, Escape, arrows or all
  /// typing system-wide with nobody handling them.
  fn effective_context(&self) -> KeyContext {
    if self.lease_expired {
      KeyContext { listen_modifiers: self.context.listen_modifiers, ..KeyContext::default() }
    } else {
      self.context
    }
  }

  fn check_lease(&mut self) {
    if self.client.is_none() {
      return;
    }
    let silent = now_nanos().saturating_sub(self.context_renewed);
    let expired = silent > protocol::CONTEXT_LEASE.as_nanos() as u64;
    if expired != self.lease_expired {
      self.lease_expired = expired;
      let event = if expired { "context_lease_expired" } else { "context_lease_renewed" };
      log(json!({ "event": event, "silent_ms": silent / 1_000_000 }));
    }
  }

  fn driver_status(&self) -> DriverStatus {
    let state = self.driver_state;
    if !state.connected {
      DriverStatus::ServiceUnavailable
    } else if state.driver_version_mismatched == Some(true) {
      DriverStatus::VersionMismatch
    } else if state.driver_activated == Some(false) {
      DriverStatus::NotActivated
    } else if state.can_forward() {
      DriverStatus::Ready
    } else {
      DriverStatus::Starting
    }
  }

  fn status(&self) -> HelperStatus {
    let seized = self.manager.seized_count();
    let capture = if self.permission != Permission::Granted {
      CaptureStatus::PermissionDenied
    } else if !self.driver_state.can_forward() {
      CaptureStatus::DriverUnavailable
    } else if self.client.is_none() || !self.console_matches {
      CaptureStatus::Idle
    } else if seized == 0 {
      CaptureStatus::NoCapturableKeyboard
    } else {
      CaptureStatus::Capturing
    };
    let devices = self.manager.devices();
    let unavailable_devices = devices
      .iter()
      .filter(|device| matches!(device.state, DeviceState::OwnedByOther | DeviceState::Failed(_)))
      .map(|device| device.product.clone())
      .collect();
    HelperStatus {
      capture,
      permission: self.permission,
      driver: self.driver_status(),
      seized_devices: seized,
      unavailable_devices,
      devices: devices.iter().map(DeviceSummary::from).collect(),
    }
  }

  fn refresh(&mut self) {
    let starting = self.mode == Mode::Off;
    let mode = if self.active() && (!starting || self.forwarding_healthy()) {
      Mode::Capture
    } else {
      Mode::Off
    };
    self.mode = mode;
    self.manager.set_mode(mode);
    self.ensure_pointing();
    self.drain_devices();
    SEIZED.store(self.manager.seized_count() as u64, Ordering::Release);
    let status = self.status();
    // Report counts change on every batch; only state changes are status changes.
    let mut key = status.clone();
    key.devices.iter_mut().for_each(|device| device.seized_reports = 0);
    let line = serde_json::to_string(&key).unwrap_or_default();
    if self.last_status.as_deref() != Some(line.as_str()) {
      log(json!({ "event": "status", "status": status }));
      self.last_status = Some(line);
      self.send(&HelperMessage::Status { status });
    }
  }

  fn drain_devices(&mut self) {
    loop {
      let events = self.manager.take_events();
      if events.is_empty() {
        break;
      }
      for event in events {
        let outcome = match event {
          DeviceEvent::Batch { device, values, timestamp, translation } => {
            fault::maybe_hang();
            if let Some(translation) = translation {
              self.engine.set_translation(device, translation);
            }
            self.counters.batches += 1;
            let context = self.effective_context();
            self.engine.apply_batch(device, &values, timestamp, &context)
          }
          DeviceEvent::Released { device } => {
            let context = self.effective_context();
            self.engine.remove_device(device, now_nanos(), &context)
          }
          DeviceEvent::Changed => continue,
        };
        self.deliver(outcome);
      }
    }
  }

  fn deliver(&mut self, outcome: BatchOutcome) {
    self.counters.claimed_edges += outcome.claimed_edges as u64;
    self.counters.forwarded_edges += outcome.forwarded_edges as u64;
    if let Some(next) = outcome.forward {
      self.post(next);
    }
    if let Some(pointer) = outcome.pointer {
      self.post_pointer(pointer);
    }
    for timed in outcome.actions {
      self.counters.actions += 1;
      self.send(&HelperMessage::Action { action: timed.action, timestamp_ns: timed.timestamp });
    }
  }

  fn post(&mut self, next: ForwardState) {
    let client = self.driver.lock().expect("driver").clone();
    let previous = self.last_posted.clone();
    let result = match client {
      Some(_) if fault::forward_fails() => Err(std::io::Error::other("injected forward failure")),
      Some(client) => client.post_state(&previous, &next),
      None => Err(std::io::Error::other("driver not connected")),
    };
    match result {
      Ok(()) => {
        self.counters.forward_posts += 1;
        self.last_posted = next;
      }
      // Forwarding is broken: stop capturing so input returns to the OS directly.
      Err(_) => self.forwarding_failed(),
    }
  }

  fn post_pointer(&mut self, report: PointerReport) {
    if self.driver_state.pointing_ready != Some(true) {
      return;
    }
    let client = self.driver.lock().expect("driver").clone();
    match client {
      Some(client) if client.post_pointer(&report).is_ok() => self.counters.forward_posts += 1,
      _ => self.forwarding_failed(),
    }
  }

  /// Creates the virtual pointing device once a seized keyboard carries a pointer collection.
  fn ensure_pointing(&mut self) {
    let generation = DRIVER_GENERATION.load(Ordering::Acquire);
    if self.pointing_requested == Some(generation) || !self.driver_state.can_forward() {
      return;
    }
    let wanted = self
      .manager
      .devices()
      .iter()
      .any(|device| device.pointer && device.state == DeviceState::Seized);
    if !wanted {
      return;
    }
    if let Some(client) = self.driver.lock().expect("driver").clone()
      && client.initialize_pointing().is_ok()
    {
      self.pointing_requested = Some(generation);
      log(json!({ "event": "pointing_initialized" }));
    }
  }

  fn send(&mut self, message: &HelperMessage) {
    let Some((_, sender)) = &self.client else { return };
    if !sender.send(message) {
      log(json!({ "event": "client_dropped", "reason": "stalled_or_closed" }));
      sender.close();
      self.client = None;
    }
  }

  fn deactivate_engine(&mut self) {
    let outcome = self.engine.reset(now_nanos());
    self.deliver(outcome);
    if let Some(client) = self.driver.lock().expect("driver").clone() {
      let _ = client.release_all();
    }
    self.last_posted = ForwardState::default();
  }

  fn handle(&mut self, event: External) -> bool {
    match event {
      External::Client(ClientEvent::Connected { id, uid, sender }) => {
        if let Some((_, previous)) = self.client.take() {
          previous.close();
          log(json!({ "event": "client_replaced" }));
        }
        self.client = Some((id, sender));
        self.client_uid = uid;
        self.check_console();
        self.context = KeyContext::default();
        self.context_renewed = now_nanos();
        self.lease_expired = false;
        self.last_status = None;
        log(json!({ "event": "client_connected", "client": id }));
      }
      External::Client(ClientEvent::Message { id, message }) => {
        if self.client.as_ref().map(|(current, _)| *current) != Some(id) {
          return true;
        }
        self.context_renewed = now_nanos();
        match message {
          AppMessage::Heartbeat => {}
          AppMessage::Hello { protocol } => {
            log(json!({ "event": "client_hello", "protocol": protocol }));
          }
          AppMessage::Context { context } => self.context = context,
        }
      }
      External::Client(ClientEvent::Disconnected { id }) => {
        if self.client.as_ref().map(|(current, _)| *current) == Some(id) {
          self.client = None;
          log(json!({ "event": "client_disconnected", "client": id }));
        }
      }
      External::Client(ClientEvent::Rejected { reason }) => {
        self.counters.rejected_clients += 1;
        log(json!({ "event": "client_rejected", "reason": reason }));
      }
      External::Driver(generation, state) => {
        // A replaced connection's reader may report after its successor; only the latest counts.
        if generation == DRIVER_GENERATION.load(Ordering::Acquire) {
          self.driver_state = state;
        }
      }
      External::Shutdown => return false,
    }
    true
  }

  fn tick(&mut self) {
    if self.ticks % UPDATE_CHECK_EVERY_TICKS == 0
      && self.executable.is_some()
      && executable_identity() != self.executable
    {
      // An app update replaced the helper; exit so launchd starts the new one. Exiting
      // releases every seized keyboard.
      log(json!({ "event": "executable_updated_exit" }));
      shutdown(self);
    }
    // SAFETY: No preconditions.
    let access = unsafe { IOHIDCheckAccess(kIOHIDRequestTypeListenEvent) };
    let permission = if access == kIOHIDAccessTypeGranted {
      Permission::Granted
    } else if access == kIOHIDAccessTypeDenied {
      Permission::Denied
    } else {
      Permission::Unknown
    };
    let granted_now = permission == Permission::Granted && self.permission != Permission::Granted;
    self.permission = permission;
    // IOKit caches a denial for the life of the process, so a grant made in System Settings is
    // only visible to a new process. Probe with a fresh child and restart into the grant.
    if permission != Permission::Granted && self.ticks % ACCESS_PROBE_EVERY_TICKS == 0 {
      let fresh_grant = std::env::current_exe()
        .ok()
        .and_then(|exe| Command::new(exe).arg("--check-access").status().ok())
        .is_some_and(|status| status.success());
      if fresh_grant {
        log(json!({ "event": "permission_granted_restart" }));
        restart_self();
      }
    }
    if granted_now || (permission == Permission::Granted && self.ticks % RESCAN_EVERY_TICKS == 0) {
      self.manager.rescan();
    }
    if self.permission != Permission::Granted && !self.access_requested {
      // Registers the helper in Input Monitoring so the user can enable it; a root daemon
      // cannot show the consent prompt itself.
      // SAFETY: No preconditions.
      unsafe { IOHIDRequestAccess(kIOHIDRequestTypeListenEvent) };
      self.access_requested = true;
    }
    self.check_lease();
    self.check_console();
    if self.manager.seized_count() > 0
      && let Some(on) = caps_lock_state()
    {
      self.manager.set_caps_lock_led(on);
    }
    self.ticks += 1;
    if self.ticks % COUNTERS_EVERY_TICKS == 0 {
      let counters = json!({
        "event": "counters",
        "batches": self.counters.batches,
        "claimed_edges": self.counters.claimed_edges,
        "forwarded_edges": self.counters.forwarded_edges,
        "actions": self.counters.actions,
        "forward_posts": self.counters.forward_posts,
        "forward_errors": self.counters.forward_errors,
        "rejected_clients": self.counters.rejected_clients,
      });
      let text = counters.to_string();
      if self.last_counters.as_deref() != Some(text.as_str()) {
        log(counters);
        self.last_counters = Some(text);
      }
    }
  }
}

static mut CORE: *mut Core = std::ptr::null_mut();

fn main() {
  let args: Vec<String> = std::env::args().collect();
  if args.get(1).map(String::as_str) == Some("--version") {
    println!("azad-capture {}", env!("CARGO_PKG_VERSION"));
    return;
  }
  if args.get(1).map(String::as_str) == Some("--list-devices") {
    list_devices();
    return;
  }
  if args.get(1).map(String::as_str) == Some("--check-access") {
    // SAFETY: No preconditions.
    let access = unsafe { IOHIDCheckAccess(kIOHIDRequestTypeListenEvent) };
    std::process::exit(if access == kIOHIDAccessTypeGranted { 0 } else { 1 });
  }
  if args.get(1).map(String::as_str) == Some("--request-access") {
    // Run from the user's session (Azad onboarding): tccd cannot prompt for a root requester,
    // but a user-session request under the same bundle identity registers the Input Monitoring
    // entry the root helper then uses.
    // SAFETY: No preconditions.
    let granted = unsafe { IOHIDRequestAccess(kIOHIDRequestTypeListenEvent) } != 0;
    println!("{}", json!({ "event": "request_access", "granted": granted }));
    std::process::exit(if granted { 0 } else { 1 });
  }
  // SAFETY: getuid has no preconditions.
  if unsafe { libc::getuid() } != 0 {
    eprintln!("azad-capture must run as root (installed as a LaunchDaemon)");
    std::process::exit(64);
  }
  block_termination_signals();
  run();
}

/// Prints the keyboards the helper would manage, without opening any of them.
fn list_devices() {
  // SAFETY: A source with a no-op perform keeps the manager's notification contract intact.
  unsafe {
    let mut context = CFRunLoopSourceContext {
      version: 0,
      info: std::ptr::null_mut(),
      retain: std::ptr::null(),
      release: std::ptr::null(),
      copy_description: std::ptr::null(),
      equal: std::ptr::null(),
      hash: std::ptr::null(),
      schedule: std::ptr::null(),
      cancel: std::ptr::null(),
      perform: ignore_events,
    };
    let source = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &mut context);
    CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopDefaultMode);
    let manager = Manager::start(source);
    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.5, 0);
    let devices: Vec<DeviceSummary> = manager.devices().iter().map(DeviceSummary::from).collect();
    println!("{}", serde_json::to_string_pretty(&devices).unwrap_or_default());
  }
}

extern "C" fn ignore_events(_info: *mut c_void) {}

fn run() {
  // SAFETY: Creates the core's run-loop source on the main thread; `perform` only touches the
  // core on this thread.
  let (run_loop, source) = unsafe {
    let run_loop = CFRunLoopGetCurrent();
    let mut context = CFRunLoopSourceContext {
      version: 0,
      info: std::ptr::null_mut(),
      retain: std::ptr::null(),
      release: std::ptr::null(),
      copy_description: std::ptr::null(),
      equal: std::ptr::null(),
      hash: std::ptr::null(),
      schedule: std::ptr::null(),
      cancel: std::ptr::null(),
      perform,
    };
    let source = CFRunLoopSourceCreate(kCFAllocatorDefault, 0, &mut context);
    CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
    (run_loop, source)
  };
  INBOX
    .set(Inbox {
      queue: Mutex::new(Vec::new()),
      source: source as usize,
      run_loop: run_loop as usize,
    })
    .ok()
    .expect("single inbox");

  let driver = Arc::new(Mutex::new(None));
  let manager = Manager::start(source);
  let core = Box::leak(Box::new(Core {
    manager,
    engine: Engine::new(),
    context: KeyContext::default(),
    forward_retry_after: 0,
    mode: Mode::Off,
    pointing_requested: None,
    client_uid: u32::MAX,
    console_matches: false,
    context_renewed: 0,
    lease_expired: false,
    client: None,
    driver: driver.clone(),
    driver_state: ServiceState::default(),
    permission: Permission::Unknown,
    access_requested: false,
    last_status: None,
    counters: Counters::default(),
    last_counters: None,
    last_posted: ForwardState::default(),
    ticks: 0,
    executable: executable_identity(),
  }));
  // SAFETY: Set once on the main thread before the run loop starts; read only on this thread.
  unsafe { CORE = core };

  let requirement = ipc::client_requirement();
  log(json!({
    "event": "started",
    "version": env!("CARGO_PKG_VERSION"),
    "client_requirement": requirement,
  }));
  if let Err(error) = ipc::serve(protocol::SOCKET_PATH, requirement, |event| {
    inbox().push(External::Client(event));
  }) {
    log(json!({ "event": "ipc_failed", "error": error.to_string() }));
    std::process::exit(71);
  }
  spawn_driver_connection(driver);
  spawn_watchdog();
  spawn_signal_thread();

  // SAFETY: The timer calls `tick` on this run loop; its context is unused.
  unsafe {
    let mut context = CFRunLoopTimerContext {
      version: 0,
      info: std::ptr::null_mut(),
      retain: std::ptr::null(),
      release: std::ptr::null(),
      copy_description: std::ptr::null(),
    };
    let timer = CFRunLoopTimerCreate(
      kCFAllocatorDefault,
      CFAbsoluteTimeGetCurrent() + TICK_SECONDS,
      TICK_SECONDS,
      0,
      0,
      on_tick,
      &mut context,
    );
    CFRunLoopAddTimer(run_loop, timer, kCFRunLoopDefaultMode);
  }
  HEARTBEAT.store(now_nanos(), Ordering::Release);
  core_mut().tick();
  core_mut().refresh();
  // SAFETY: Runs the main run loop until process exit.
  unsafe { CFRunLoopRun() };
}

fn core_mut() -> &'static mut Core {
  // SAFETY: CORE is set before the run loop starts and is only dereferenced on the main thread,
  // one callback at a time.
  unsafe { &mut *CORE }
}

fn inbox() -> &'static Inbox {
  INBOX.get().expect("inbox initialized")
}

extern "C" fn perform(_info: *mut c_void) {
  let core = core_mut();
  let events = std::mem::take(&mut *inbox().queue.lock().expect("inbox"));
  let mut keep_running = true;
  for event in events {
    keep_running &= core.handle(event);
  }
  let was_active = core.manager.seized_count() > 0;
  core.refresh();
  if was_active && !core.active() {
    core.deactivate_engine();
  }
  if !keep_running {
    shutdown(core);
  }
}

extern "C" fn on_tick(_timer: CFRunLoopTimerRef, _info: *mut c_void) {
  HEARTBEAT.store(now_nanos(), Ordering::Release);
  let core = core_mut();
  core.tick();
  let was_active = core.manager.seized_count() > 0;
  core.refresh();
  if was_active && !core.active() {
    core.deactivate_engine();
  }
}

fn shutdown(core: &mut Core) -> ! {
  core.client = None;
  core.manager.set_mode(Mode::Off);
  core.deactivate_engine();
  if let Some(client) = core.driver.lock().expect("driver").take() {
    client.shutdown();
  }
  stop_owned_driver_daemon();
  let _ = std::fs::remove_file(protocol::SOCKET_PATH);
  log(json!({ "event": "stopped" }));
  std::process::exit(0);
}

fn executable_identity() -> Option<(u64, i64)> {
  use std::os::unix::fs::MetadataExt;
  let metadata = std::fs::metadata(std::env::current_exe().ok()?).ok()?;
  Some((metadata.ino(), metadata.mtime()))
}

/// Replaces this process with a fresh copy; devices are closed (none are seized without a grant).
fn restart_self() -> ! {
  stop_owned_driver_daemon();
  let _ = std::fs::remove_file(protocol::SOCKET_PATH);
  if let Ok(exe) = std::env::current_exe() {
    let error = std::os::unix::process::CommandExt::exec(&mut Command::new(exe));
    log(json!({ "event": "restart_failed", "error": error.to_string() }));
  }
  std::process::exit(EX_TEMPFAIL);
}

static OWNED_DAEMON: Mutex<Option<Child>> = Mutex::new(None);

/// Keeps a connection to the virtual keyboard service. When no service answers and the driver
/// package is installed, runs its daemon as a supervised child; a remapper that already runs
/// the daemon (Karabiner-Elements) is used as-is.
fn spawn_driver_connection(slot: Arc<Mutex<Option<Arc<vhid::Client>>>>) {
  thread::Builder::new()
    .name("azad-vhid-connect".into())
    .spawn(move || {
      loop {
        if yield_to_foreign_driver_daemon() {
          // Our client is attached to the daemon being stopped; reconnect to the live service.
          if let Some(client) = slot.lock().expect("driver").take() {
            client.shutdown();
          }
        }
        let connected = slot.lock().expect("driver").as_ref().is_some_and(|c| c.state().connected);
        if !connected {
          slot.lock().expect("driver").take();
          let generation = DRIVER_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
          seed_keyboard_type();
          match vhid::Client::connect(
            vhid::SERVER_SOCKET_PATH,
            Box::new(move |state| inbox().push(External::Driver(generation, state))),
          ) {
            Ok(client) => {
              log(json!({ "event": "driver_connected" }));
              *slot.lock().expect("driver") = Some(client);
            }
            Err(_) => {
              inbox().push(External::Driver(generation, ServiceState::default()));
              ensure_driver_daemon();
            }
          }
        }
        thread::sleep(Duration::from_secs(1));
      }
    })
    .expect("spawn driver connection");
}

/// Records a layout type for Azad's virtual keyboard before it appears. macOS does not deliver
/// key events from an unidentified keyboard to applications (only to event taps) and opens
/// Keyboard Setup Assistant for it, so forwarding depends on this entry. Uses the type most
/// other keyboards on this Mac were identified as, defaulting to ANSI.
fn seed_keyboard_type() {
  const DOMAIN: &str = "/Library/Preferences/com.apple.keyboardtype";
  const ANSI: i64 = 40;
  let key = format!("{}-{}-0", vhid::AZAD_OUTPUT_PRODUCT_ID, vhid::AZAD_OUTPUT_VENDOR_ID);
  let current = Command::new("/usr/bin/defaults")
    .args(["read", DOMAIN, "keyboardtype"])
    .output();
  let text = current
    .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    .unwrap_or_default();
  if text.contains(&format!("\"{key}\"")) {
    return;
  }
  let mut counts = std::collections::BTreeMap::<i64, usize>::new();
  for line in text.lines() {
    if let Some(value) = line.split('=').nth(1) {
      if let Ok(kind) = value.trim().trim_end_matches(';').parse::<i64>() {
        *counts.entry(kind).or_default() += 1;
      }
    }
  }
  let kind = counts
    .into_iter()
    .max_by_key(|(_, count)| *count)
    .map_or(ANSI, |(kind, _)| kind);
  let status = Command::new("/usr/bin/defaults")
    .args(["write", DOMAIN, "keyboardtype", "-dict-add", &key, "-int", &kind.to_string()])
    .status();
  log(
    json!({ "event": "keyboard_type_seeded", "type": kind, "ok": status.is_ok_and(|s| s.success()) }),
  );
}

/// Stops the daemon this helper started once another one runs, e.g. Karabiner-Elements' own
/// service. The newer daemon rebinds the shared socket path, so keeping ours would split the
/// virtual keyboards across two servers. Returns true when ours was stopped.
fn yield_to_foreign_driver_daemon() -> bool {
  let mut owned = OWNED_DAEMON.lock().expect("daemon");
  let Some(child) = owned.as_mut() else { return false };
  let ours = child.id() as libc::pid_t;
  if !driver_daemon_pids().into_iter().any(|pid| pid != ours) {
    return false;
  }
  let _ = child.kill();
  let _ = child.wait();
  *owned = None;
  log(json!({ "event": "driver_daemon_yielded", "pid": ours }));
  true
}

fn driver_daemon_pids() -> Vec<libc::pid_t> {
  let mut pids = vec![0 as libc::pid_t; 4096];
  let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
  // SAFETY: The buffer holds `bytes` bytes of pid storage.
  let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
  pids.truncate(count.max(0) as usize);
  pids
    .into_iter()
    .filter(|pid| {
      let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
      // SAFETY: `path` is writable for its full length.
      let length = unsafe { libc::proc_pidpath(*pid, path.as_mut_ptr().cast(), path.len() as u32) };
      length > 0 && &path[..length as usize] == DRIVER_DAEMON_PATH.as_bytes()
    })
    .collect()
}

fn ensure_driver_daemon() {
  let mut owned = OWNED_DAEMON.lock().expect("daemon");
  if let Some(child) = owned.as_mut() {
    match child.try_wait() {
      Ok(None) => return,
      Ok(Some(status)) => log(json!({ "event": "driver_daemon_exited", "status": status.code() })),
      Err(_) => {}
    }
    *owned = None;
  }
  // A daemon that is running but not yet accepting connections must not get a competitor.
  if !Path::new(DRIVER_DAEMON_PATH).exists() || !driver_daemon_pids().is_empty() {
    return;
  }
  match Command::new(DRIVER_DAEMON_PATH).spawn() {
    Ok(child) => {
      log(json!({ "event": "driver_daemon_started", "pid": child.id() }));
      *owned = Some(child);
    }
    Err(error) => log(json!({ "event": "driver_daemon_failed", "error": error.to_string() })),
  }
}

fn stop_owned_driver_daemon() {
  if let Some(mut child) = OWNED_DAEMON.lock().expect("daemon").take() {
    let _ = child.kill();
    let _ = child.wait();
  }
}

fn spawn_watchdog() {
  thread::Builder::new()
    .name("azad-capture-watchdog".into())
    .spawn(|| {
      loop {
        thread::sleep(Duration::from_millis(250));
        let silent = now_nanos().saturating_sub(HEARTBEAT.load(Ordering::Acquire));
        if SEIZED.load(Ordering::Acquire) > 0 && silent > WATCHDOG_LIMIT_NANOS {
          eprintln!("{}", json!({ "event": "watchdog_exit", "silent_ms": silent / 1_000_000 }));
          // Exiting makes the kernel release every seized keyboard.
          // SAFETY: Terminates immediately without running destructors on a hung process.
          unsafe { libc::_exit(WATCHDOG_EXIT_CODE) };
        }
      }
    })
    .expect("spawn watchdog");
}

fn block_termination_signals() {
  // SAFETY: Blocks SIGTERM/SIGINT in this thread before others are spawned so they inherit the
  // mask and only the signal thread receives them via sigwait.
  unsafe {
    let mut set: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut set);
    libc::sigaddset(&mut set, libc::SIGTERM);
    libc::sigaddset(&mut set, libc::SIGINT);
    libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
  }
}

fn spawn_signal_thread() {
  thread::Builder::new()
    .name("azad-capture-signals".into())
    .spawn(|| {
      // SAFETY: Waits on the signals blocked in `block_termination_signals`.
      unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        let mut signal = 0;
        libc::sigwait(&set, &mut signal);
      }
      inbox().push(External::Shutdown);
    })
    .expect("spawn signal thread");
}

/// Failure injection for the VM verification matrix. Honoured only inside a macOS virtual
/// machine and only while a root-owned marker file exists.
mod fault {
  use std::sync::OnceLock;

  const HANG_MARKER: &str = "/var/run/ai.azad.capture.fault-hang";
  const FORWARD_MARKER: &str = "/var/run/ai.azad.capture.fault-forward";

  fn armed(marker: &str) -> bool {
    virtual_mac()
      && std::fs::metadata(marker)
        .is_ok_and(|metadata| std::os::unix::fs::MetadataExt::uid(&metadata) == 0)
  }

  pub fn forward_fails() -> bool {
    armed(FORWARD_MARKER)
  }

  pub fn maybe_hang() {
    if !armed(HANG_MARKER) {
      return;
    }
    super::log(serde_json::json!({ "event": "fault_hang" }));
    loop {
      std::thread::park();
    }
  }

  fn virtual_mac() -> bool {
    static VIRTUAL: OnceLock<bool> = OnceLock::new();
    *VIRTUAL.get_or_init(|| {
      let mut model = [0u8; 128];
      let mut size = model.len();
      // SAFETY: `model` is writable for `size` bytes.
      let status = unsafe {
        libc::sysctlbyname(
          c"hw.model".as_ptr(),
          model.as_mut_ptr().cast(),
          &mut size,
          std::ptr::null_mut(),
          0,
        )
      };
      status == 0 && model.starts_with(b"VirtualMac")
    })
  }
}

fn log(value: serde_json::Value) {
  println!("{value}");
}
