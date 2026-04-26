use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use asr::devices::DeviceStateSnapshot;
use asr::pipeline::{DebugStatsEvent, EngineState};

use crate::config::AzadConfig;
use crate::device::{DeviceController, DeviceEvent};
use crate::hotkey_sm::{HotkeyEffect, HotkeyInput, HotkeyState, RuntimeSnapshot};
use crate::metrics_log::{self, MetricsLogEvent, MetricsLogRecord, TranscriptMode};
use crate::model_download::{self, DownloadHandle};
use crate::models::{self, PackStatus};
use crate::platform;
use crate::platform::{
  DeviceMenuModel,
  DeviceMenuRow,
  PasteResult,
  SettingsTab,
  SettingsViewModel,
};
use crate::preferred_store;
use crate::settings::{AutoSubmitMode, PasteMethod};
use crate::speech::{SpeechEvent, SpeechSession, spawn_speech_session};
use crate::transcript_history::TranscriptIndex;

const DEVICE_SWITCH_RESTART_DEBOUNCE_MS: u64 = 250;
const OVERLAY_ACTIVITY_HISTORY_LEN: usize = 96;
const OVERLAY_ACTIVITY_IDLE_TIMEOUT_MS: u64 = 220;
const OVERLAY_ACTIVITY_DECAY_PER_TICK: f32 = 0.88;
const OVERLAY_BUSY_PHASE_STEP: f32 = 0.24;
const LISTEN_TOGGLE_NOTICE_DURATION_MS: u64 = 600;
const LISTEN_RECOVERING_NOTICE_DURATION_MS: u64 = 1200;
const CANCEL_VAD_SHOW_SUPPRESSION_MS: u64 = 500;
const SESSION_FAULT_WINDOW_MS: u64 = 30_000;
const SESSION_IMMEDIATE_RETRY_LIMIT: usize = 2;
const SESSION_DEGRADED_THRESHOLD: usize = 3;

#[derive(Debug, Clone)]
pub enum AppEvent {
  HotkeyPressed,
  HotkeyReleased,
  FinalizeHotkeyPressed { raw_requested: bool },
  MenuToggleAlwaysListening,
  MenuToggleDevices,
  MenuSelectDevice(String),
  MenuOpenSettings,
  MenuOpened,
  MenuClosed,
  SettingsToggleRunOnStartup(bool),
  SettingsToggleDebugStats(bool),
  SettingsSelectPasteMethod(PasteMethod),
  SettingsSelectAutoSubmit(AutoSubmitMode),
  SettingsToggleAppendTrailingSpace(bool),
  SettingsAddRemovedWord(String),
  SettingsRemoveRemovedWord(String),
  SettingsRefresh,
  SettingsDownloadModel(String),
  SettingsCancelDownload,
  ModelDownloadProgress { pack_id: String, bytes_done: u64, bytes_total: u64 },
  ModelDownloadCompleted(String),
  ModelDownloadError { pack_id: String, message: String },
  OverlayCancel,
  ArrowNavigate(i32),
  Speech(SpeechEvent),
  Device(DeviceEvent),
}

static EVENT_TX: OnceLock<Sender<AppEvent>> = OnceLock::new();
static EVENT_RX: OnceLock<Mutex<Receiver<AppEvent>>> = OnceLock::new();
static CONTROLLER: OnceLock<Mutex<AppController>> = OnceLock::new();
static HOTKEY_CLOCK_START: OnceLock<Instant> = OnceLock::new();

/// Heartbeat log cadence. Only emits while `AzadDebugStatsEnabled` is set, so it's quiet for
/// normal users. The point is to have a timestamped breadcrumb trail of steady-state flags
/// right up to the moment the app goes silent — so when we get another "it stopped responding
/// and a restart fixed it" report, the tail of the log tells us the last observed values of
/// `capture_enabled`, `always_listening`, `manual_hold_active`, etc.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

pub fn run() {
  platform::check_required_permissions_on_startup();

  let (tx, rx) = mpsc::channel::<AppEvent>();
  let _ = EVENT_TX.set(tx);
  let _ = EVENT_RX.set(Mutex::new(rx));

  let mut controller = AppController::new(AzadConfig::default());
  controller.bootstrap();
  let _ = CONTROLLER.set(Mutex::new(controller));

  spawn_heartbeat_thread();

  platform::run_app();
}

fn spawn_heartbeat_thread() {
  std::thread::Builder::new()
    .name("azad-heartbeat".to_string())
    .spawn(|| {
      loop {
        std::thread::sleep(HEARTBEAT_INTERVAL);
        let Some(ctrl_lock) = CONTROLLER.get() else { continue };
        // try_lock so a stuck main thread doesn't stall the heartbeat — we can still emit a
        // line saying we couldn't observe state, which is itself a signal.
        match ctrl_lock.try_lock() {
          Ok(ctrl) => {
            if !ctrl.debug_stats_enabled {
              continue;
            }
            eprintln!(
              "AZAD_HEARTBEAT session_present={} always_listening={} manual_hold_active={} \
             engine_state={:?} overlay_visible={} current_turn={:?} latest_seen_turn={} \
             last_pasted_turn={:?} cancelled={} hold_saw_speech={} pending_recovery={}",
              ctrl.session.is_some(),
              ctrl.always_listening_enabled,
              ctrl.manual_hold_active,
              ctrl.engine_state,
              ctrl.overlay_visible,
              ctrl.current_turn_id,
              ctrl.latest_seen_turn_id,
              ctrl.last_pasted_turn_id,
              ctrl.cancelled,
              ctrl.hold_saw_speech,
              ctrl.pending_recovery_restart,
            );
          }
          Err(_) => {
            eprintln!("AZAD_HEARTBEAT controller mutex busy — main thread may be stalled");
          }
        }
      }
    })
    .expect("spawn heartbeat thread");
}

pub fn send_event(event: AppEvent) {
  if let Some(tx) = EVENT_TX.get() {
    let _ = tx.send(event);
  }
}

pub fn drain_events() {
  let Some(rx) = EVENT_RX.get() else {
    return;
  };
  let Some(controller_mutex) = CONTROLLER.get() else {
    return;
  };

  let mut pending = Vec::new();
  {
    let rx = rx.lock().unwrap();
    loop {
      match rx.try_recv() {
        Ok(event) => pending.push(event),
        Err(TryRecvError::Empty) => break,
        Err(TryRecvError::Disconnected) => break,
      }
    }
  }

  let mut controller = controller_mutex.lock().unwrap();
  for event in pending {
    controller.handle_event(event);
  }
  controller.on_tick();
}

struct AppController {
  cfg: AzadConfig,
  session: Option<SpeechSession>,
  session_device_id: Option<String>,
  next_session_id: u64,

  device_controller: Option<DeviceController>,
  device_snapshot: Option<DeviceStateSnapshot>,
  device_menu_expanded: bool,
  always_listening_enabled: bool,
  pending_always_listening_enabled: Option<bool>,

  manual_hold_active: bool,
  hold_saw_speech: bool,
  overlay_visible: bool,
  overlay_pending_vad_text: bool,
  cancelled: bool,
  last_pasted_turn_id: Option<u64>,
  latest_draft: String,
  finalizing_draft: String,
  finalizing_activity_history: Vec<f32>,
  latest_final: Option<String>,
  finalizing_deadline: Option<Instant>,
  finalizing_turn_id: Option<u64>,
  held_top_active: bool,
  held_top_draft: String,
  saw_vad_start_during_finalizing: bool,
  raw_handled_turn_id: Option<u64>,
  deferred_vad_start: bool,
  accessibility_notice_deadline: Option<Instant>,
  listen_toggle_notice: Option<ListenToggleNotice>,
  latest_seen_turn_id: u64,
  turn_accept_floor: u64,
  current_turn_id: Option<u64>,
  activity_history: Vec<f32>,
  latest_activity_level: f32,
  last_activity_at: Option<Instant>,
  busy_border_phase: f32,
  pending_device_switch_target: Option<String>,
  pending_device_switch_deadline: Option<Instant>,
  engine_state: EngineState,
  hotkey_state: HotkeyState,
  raw_finalize_requested: bool,
  run_on_startup_enabled: bool,
  paste_method: PasteMethod,
  auto_submit_mode: AutoSubmitMode,
  append_trailing_space_on_paste: bool,
  debug_stats_enabled: bool,
  turn_started_at: HashMap<u64, Instant>,
  turn_finalize_outcomes: HashMap<u64, (String, String)>,
  session_recovery_state: SessionRecoveryState,
  session_fault_window: Vec<Instant>,
  last_session_error_was_stream_fault: bool,
  pending_recovery_restart: bool,
  cancel_vad_show_suppressed_until: Option<Instant>,
  active_pack_id: String,
  models_ready: bool,
  pending_first_launch_settings: bool,
  download_handle: Option<DownloadHandle>,
  download_progress: (u64, u64),
  download_progress_dirty: bool,
  transcript_index: Option<TranscriptIndex>,
  history_browsing: bool,
  history_browse_index: usize,
  removed_words: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawFinalizeUiPlan {
  hide_overlay: bool,
  disable_capture: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ManualHoldReleasePlan {
  capture_enabled: bool,
  action: ManualHoldReleaseAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManualHoldReleaseAction {
  KeepLive,
  HideOverlay,
  FinalizeTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionRecoveryState {
  Healthy,
  Recovering,
  Degraded,
}

#[derive(Debug, Clone, Copy)]
struct ListenToggleNotice {
  enabled: bool,
  started_at: Instant,
  duration: Duration,
}

fn raw_finalize_ui_plan(
  always_listening_enabled: bool,
  manual_hold_active: bool,
  forced_by_finalize_hotkey: bool,
) -> RawFinalizeUiPlan {
  RawFinalizeUiPlan {
    hide_overlay: forced_by_finalize_hotkey || !manual_hold_active,
    disable_capture: !always_listening_enabled && !manual_hold_active,
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftOverlayAction {
  /// Show the overlay now and clear the pending flag.
  Show,
  /// Cancel-suppression is still active. Leave the pending flag as-is so a later
  /// DraftUpdate past the window can still bring the overlay up.
  KeepPendingForLater,
  /// Nothing to show (overlay already visible, or never pending). Clear the flag.
  Clear,
}

/// Decide what to do with `overlay_pending_vad_text` when a `DraftUpdated` arrives.
///
/// The previous implementation unconditionally cleared the pending flag after checking
/// whether to show. That had a bug: if the user hit Escape and started talking again within
/// `CANCEL_VAD_SHOW_SUPPRESSION_MS`, the new turn's first DraftUpdate hit the suppression
/// branch, the show was correctly skipped — but the pending flag was then cleared, so no
/// subsequent DraftUpdate past the suppression window could ever bring the overlay up.
/// Transcription continued in the background with no visible overlay.
fn draft_update_overlay_action(
  pending: bool,
  overlay_visible: bool,
  cancel_suppression_active: bool,
) -> DraftOverlayAction {
  if cancel_suppression_active {
    DraftOverlayAction::KeepPendingForLater
  } else if pending && !overlay_visible {
    DraftOverlayAction::Show
  } else {
    DraftOverlayAction::Clear
  }
}

fn manual_hold_release_plan(
  always_listening_enabled: bool,
  should_finalize: bool,
  has_started_turn: bool,
) -> ManualHoldReleasePlan {
  let action = if should_finalize {
    if has_started_turn {
      ManualHoldReleaseAction::FinalizeTurn
    } else {
      ManualHoldReleaseAction::HideOverlay
    }
  } else {
    ManualHoldReleaseAction::KeepLive
  };

  ManualHoldReleasePlan { capture_enabled: always_listening_enabled, action }
}

fn should_ignore_finalizing_event(raw_handled_turn_id: Option<u64>, turn_id: u64) -> bool {
  raw_handled_turn_id == Some(turn_id)
}

fn split_overlay_active_for_turns(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
) -> bool {
  finalizing_turn_id
    .zip(current_turn_id)
    .is_some_and(|(finalizing, current)| current > finalizing)
}

fn split_overlay_visible_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
) -> bool {
  split_overlay_active_for_turns(finalizing_turn_id, current_turn_id)
    && !live_draft.trim().is_empty()
}

fn split_overlay_visible_with_hold_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
  hold_active: bool,
) -> bool {
  split_overlay_visible_for_state(finalizing_turn_id, current_turn_id, live_draft)
    || (hold_active && !live_draft.trim().is_empty())
}

fn split_overlay_visible_with_vad_hint_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
  hold_active: bool,
  saw_vad_start_during_finalizing: bool,
) -> bool {
  split_overlay_visible_with_hold_for_state(
    finalizing_turn_id,
    current_turn_id,
    live_draft,
    hold_active,
  ) || (finalizing_turn_id.is_some()
    && saw_vad_start_during_finalizing
    && !live_draft.trim().is_empty())
}

fn draft_matches_finalized_text(live_draft: &str, finalized_text: &str) -> bool {
  let live_tokens = live_draft
    .split_whitespace()
    .map(|token| token.to_ascii_lowercase())
    .collect::<Vec<_>>();
  let final_tokens = finalized_text
    .split_whitespace()
    .map(|token| token.to_ascii_lowercase())
    .collect::<Vec<_>>();

  if live_tokens.is_empty() || final_tokens.is_empty() {
    return false;
  }
  if live_tokens == final_tokens {
    return true;
  }

  let min_len = live_tokens.len().min(final_tokens.len());
  let max_len = live_tokens.len().max(final_tokens.len());
  let lcp = live_tokens.iter().zip(final_tokens.iter()).take_while(|(a, b)| a == b).count();

  if lcp == min_len {
    // One side is a strict token-prefix of the other.
    return true;
  }

  // Treat near-identical beginnings as the same finalized lane.
  // This prevents VAD-hint-only split mode from getting stuck on replayed
  // same-turn drafts that differ only by minor tail edits/punctuation.
  lcp * 100 >= min_len * 85 && (max_len - min_len) <= 2
}

fn split_overlay_visible_with_live_divergence_for_state(
  finalizing_turn_id: Option<u64>,
  live_draft: &str,
  finalizing_draft: &str,
) -> bool {
  finalizing_turn_id.is_some()
    && !live_draft.trim().is_empty()
    && !finalizing_draft.trim().is_empty()
    && !draft_matches_finalized_text(live_draft, finalizing_draft)
}

fn split_top_completion_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
  hold_active: bool,
  saw_vad_start_during_finalizing: bool,
  finalized_turn_id: u64,
  finalized_text: &str,
) -> bool {
  let live_draft = live_draft.trim();
  if live_draft.is_empty() || finalizing_turn_id != Some(finalized_turn_id) {
    return false;
  }

  if split_overlay_active_for_turns(finalizing_turn_id, current_turn_id) {
    return true;
  }

  if hold_active {
    return true;
  }

  if saw_vad_start_during_finalizing {
    return !draft_matches_finalized_text(live_draft, finalized_text);
  }

  false
}

fn raw_finalize_target_turn_id_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  latest_seen_turn_id: u64,
  live_draft: &str,
) -> Option<u64> {
  if split_overlay_visible_for_state(finalizing_turn_id, current_turn_id, live_draft) {
    current_turn_id
  } else {
    finalizing_turn_id
      .or(current_turn_id)
      .or_else(|| (latest_seen_turn_id > 0).then_some(latest_seen_turn_id))
  }
}

fn next_current_turn_id(current_turn_id: Option<u64>, incoming_turn_id: u64) -> u64 {
  current_turn_id
    .map(|current| current.max(incoming_turn_id))
    .unwrap_or(incoming_turn_id)
}

fn has_turn_context_for_snapshot(
  engine_state: EngineState,
  current_turn_id: Option<u64>,
  finalizing_turn_id: Option<u64>,
  latest_draft: &str,
) -> bool {
  engine_state == EngineState::Speech
    || current_turn_id.is_some()
    || finalizing_turn_id.is_some()
    || !latest_draft.trim().is_empty()
}

fn has_actionable_turn_context_for_snapshot(
  engine_state: EngineState,
  current_turn_id: Option<u64>,
  finalizing_turn_id: Option<u64>,
  latest_draft: &str,
  overlay_visible: bool,
  manual_hold_active: bool,
) -> bool {
  if !has_turn_context_for_snapshot(engine_state, current_turn_id, finalizing_turn_id, latest_draft)
  {
    return false;
  }

  // Ignore stale post-turn ids/text once UI is fully idle; they should not
  // block an idle double-tap from toggling always-listening back on.
  engine_state == EngineState::Speech
    || finalizing_turn_id.is_some()
    || overlay_visible
    || manual_hold_active
}

fn has_started_turn_for_snapshot(
  manual_hold_active: bool,
  hold_saw_speech: bool,
  engine_state: EngineState,
  finalizing_turn_id: Option<u64>,
  latest_draft: &str,
) -> bool {
  if manual_hold_active {
    return hold_saw_speech;
  }
  // Outside active hold, treat engine speech/finalizing as active turn
  // progress for state decisions.
  if engine_state == EngineState::Speech || finalizing_turn_id.is_some() {
    return true;
  }
  !latest_draft.trim().is_empty()
}

fn preview_text_for_metrics(text: &str, max_chars: usize) -> String {
  let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if normalized.chars().count() <= max_chars {
    return normalized;
  }
  let mut out = String::new();
  for ch in normalized.chars().take(max_chars) {
    out.push(ch);
  }
  out.push_str("...");
  out
}

fn transcript_mode_label(mode: TranscriptMode) -> &'static str {
  match mode {
    TranscriptMode::Normal => "normal",
    TranscriptMode::Raw => "raw",
  }
}

fn paste_result_label(result: PasteResult) -> &'static str {
  match result {
    PasteResult::Pasted => "pasted",
    PasteResult::AccessibilityRequired => "accessibility_required",
    PasteResult::EmptyText => "empty_text",
    PasteResult::ClipboardWriteFailed => "clipboard_write_failed",
    PasteResult::InputEventFailed => "input_event_failed",
  }
}

fn paste_method_label(method: PasteMethod) -> &'static str {
  match method {
    PasteMethod::ClipboardPaste => "clipboard_paste",
    PasteMethod::DirectTyping => "direct_typing",
    PasteMethod::DirectTypingAndCopyClipboard => "direct_typing_copy_clipboard",
  }
}

fn auto_submit_mode_label(mode: AutoSubmitMode) -> &'static str {
  match mode {
    AutoSubmitMode::Off => "off",
    AutoSubmitMode::Enter => "enter",
    AutoSubmitMode::CtrlEnter => "ctrl_enter",
    AutoSubmitMode::ShiftEnter => "shift_enter",
  }
}

fn build_paste_text(text: &str, append_trailing_space: bool, removed_words: &[String]) -> String {
  let mut paste_text = if removed_words.is_empty() {
    text.to_string()
  } else {
    strip_removed_words(text, removed_words)
  };
  if append_trailing_space && !paste_text.chars().last().is_some_and(|ch| ch.is_whitespace()) {
    paste_text.push(' ');
  }
  paste_text
}

fn strip_removed_words(text: &str, removed_words: &[String]) -> String {
  let words: Vec<&str> = text.split_whitespace().collect();
  let kept: Vec<&str> = words
    .into_iter()
    .filter(|w| {
      let bare = w.trim_matches(|c: char| c.is_ascii_punctuation());
      !removed_words.iter().any(|rw| rw.eq_ignore_ascii_case(bare))
    })
    .collect();
  kept.join(" ")
}

fn listen_toggle_notice(enabled: bool) -> (&'static str, Vec<platform::OverlayNoticeSegment>) {
  if enabled { ("Listen ENABLED", Vec::new()) } else { ("Listen DISABLED", Vec::new()) }
}

fn is_stream_fault_message(message: &str) -> bool {
  let msg = message.to_ascii_lowercase();
  msg.contains("audio input stream ended after error")
    || msg.contains("audio input stream error")
    || msg.contains("failed to open microphone capture")
    || msg.contains("requested device is no longer available")
}

fn recovery_state_for_fault_count(faults_in_window: usize) -> SessionRecoveryState {
  if faults_in_window >= SESSION_DEGRADED_THRESHOLD {
    SessionRecoveryState::Degraded
  } else if faults_in_window > 0 {
    SessionRecoveryState::Recovering
  } else {
    SessionRecoveryState::Healthy
  }
}

fn allow_immediate_restart_for_fault_count(faults_in_window: usize) -> bool {
  faults_in_window <= SESSION_IMMEDIATE_RETRY_LIMIT
}

impl AppController {
  fn new(cfg: AzadConfig) -> Self {
    let always_listening_enabled = preferred_store::load_always_listening_enabled();
    let run_on_startup_enabled = preferred_store::load_run_on_startup_enabled();
    let paste_method = preferred_store::load_paste_method();
    let auto_submit_mode = preferred_store::load_auto_submit_mode();
    let append_trailing_space_on_paste = preferred_store::load_append_trailing_space_on_paste();
    let debug_stats_enabled = preferred_store::load_debug_stats_enabled();
    let active_pack_id = preferred_store::load_active_model_pack()
      .unwrap_or_else(|| models::default_pack().id.to_string());
    let transcript_index = TranscriptIndex::load();
    let removed_words = preferred_store::load_removed_words();
    Self {
      cfg,
      session: None,
      session_device_id: None,
      next_session_id: 1,
      device_controller: None,
      device_snapshot: None,
      device_menu_expanded: false,
      always_listening_enabled,
      pending_always_listening_enabled: None,
      manual_hold_active: false,
      hold_saw_speech: false,
      overlay_visible: false,
      overlay_pending_vad_text: false,
      cancelled: false,
      last_pasted_turn_id: None,
      latest_draft: String::new(),
      finalizing_draft: String::new(),
      finalizing_activity_history: vec![0.0; OVERLAY_ACTIVITY_HISTORY_LEN],
      latest_final: None,
      finalizing_deadline: None,
      finalizing_turn_id: None,
      held_top_active: false,
      held_top_draft: String::new(),
      saw_vad_start_during_finalizing: false,
      raw_handled_turn_id: None,
      deferred_vad_start: false,
      accessibility_notice_deadline: None,
      listen_toggle_notice: None,
      latest_seen_turn_id: 0,
      turn_accept_floor: 1,
      current_turn_id: None,
      activity_history: vec![0.0; OVERLAY_ACTIVITY_HISTORY_LEN],
      latest_activity_level: 0.0,
      last_activity_at: None,
      busy_border_phase: 0.0,
      pending_device_switch_target: None,
      pending_device_switch_deadline: None,
      engine_state: EngineState::Idle,
      hotkey_state: HotkeyState::default(),
      raw_finalize_requested: false,
      run_on_startup_enabled,
      paste_method,
      auto_submit_mode,
      append_trailing_space_on_paste,
      debug_stats_enabled,
      turn_started_at: HashMap::new(),
      turn_finalize_outcomes: HashMap::new(),
      session_recovery_state: SessionRecoveryState::Healthy,
      session_fault_window: Vec::new(),
      last_session_error_was_stream_fault: false,
      pending_recovery_restart: false,
      cancel_vad_show_suppressed_until: None,
      active_pack_id,
      models_ready: false,
      pending_first_launch_settings: false,
      download_handle: None,
      download_progress: (0, 0),
      download_progress_dirty: false,
      transcript_index,
      history_browsing: false,
      history_browse_index: 0,
      removed_words,
    }
  }

  fn bootstrap(&mut self) {
    self.apply_run_on_startup_preference();
    self.refresh_models_ready();
    if !self.models_ready {
      self.pending_first_launch_settings = true;
    }
    self.start_device_controller();
    self.render_device_menu();
    self.ensure_session();
  }

  fn refresh_models_ready(&mut self) {
    let pack = models::pack_by_id(&self.active_pack_id).unwrap_or_else(models::default_pack);
    self.models_ready = models::check_pack_status(pack) == PackStatus::Ready;
    if self.models_ready {
      self.cfg.rebuild_pipeline_paths(pack);
    }
  }

  fn start_device_controller(&mut self) {
    let preferred = preferred_store::load_preferred_device_id();

    let emit: Arc<dyn Fn(DeviceEvent) + Send + Sync> =
      Arc::new(|ev| send_event(AppEvent::Device(ev)));

    match DeviceController::start(preferred, emit) {
      Ok(controller) => {
        match controller.snapshot() {
          Ok(snapshot) => self.handle_device_state_changed(snapshot),
          Err(err) => eprintln!("Azad: initial device snapshot failed: {err}"),
        }
        self.device_controller = Some(controller);
      }
      Err(err) => {
        eprintln!("Azad: failed to start device controller: {err}");
        self.device_controller = None;
      }
    }
  }

  fn current_device_id(&self) -> Option<&str> {
    self.device_snapshot.as_ref().and_then(|s| s.current_id.as_deref())
  }

  fn prune_session_fault_window(&mut self, now: Instant) {
    self.session_fault_window.retain(|ts| {
      now.saturating_duration_since(*ts) <= Duration::from_millis(SESSION_FAULT_WINDOW_MS)
    });
  }

  fn note_stream_fault(&mut self, message: &str) {
    let now = Instant::now();
    self.prune_session_fault_window(now);
    self.session_fault_window.push(now);

    let faults = self.session_fault_window.len();
    self.session_recovery_state = recovery_state_for_fault_count(faults);
    self.last_session_error_was_stream_fault = true;
    self.pending_recovery_restart = true;

    eprintln!(
      "Azad: stream fault detected faults_window={} recovery_state={:?} message={}",
      faults, self.session_recovery_state, message
    );

    let body = if self.session_recovery_state == SessionRecoveryState::Degraded {
      "Audio unstable; waiting for device change or hold hotkey retry"
    } else {
      "Audio stream interrupted; retrying live"
    };
    self.show_overlay_notice(
      "Listening recovering",
      body,
      Duration::from_millis(LISTEN_RECOVERING_NOTICE_DURATION_MS),
    );
  }

  fn note_stable_stream_progress(&mut self) {
    if self.session_recovery_state == SessionRecoveryState::Healthy
      && self.session_fault_window.is_empty()
      && !self.pending_recovery_restart
    {
      return;
    }

    self.session_recovery_state = SessionRecoveryState::Healthy;
    self.session_fault_window.clear();
    self.last_session_error_was_stream_fault = false;
    self.pending_recovery_restart = false;
  }

  fn should_restart_after_session_end(&mut self) -> bool {
    if !self.last_session_error_was_stream_fault {
      return true;
    }

    self.last_session_error_was_stream_fault = false;
    let faults = self.session_fault_window.len();
    if allow_immediate_restart_for_fault_count(faults) {
      self.session_recovery_state = SessionRecoveryState::Recovering;
      self.pending_recovery_restart = true;
      true
    } else {
      self.session_recovery_state = SessionRecoveryState::Degraded;
      self.pending_recovery_restart = true;
      false
    }
  }

  fn handle_event(&mut self, event: AppEvent) {
    match event {
      AppEvent::HotkeyPressed => self.handle_hotkey_pressed(),
      AppEvent::HotkeyReleased => self.handle_hotkey_released(),
      AppEvent::FinalizeHotkeyPressed { raw_requested } => {
        self.handle_finalize_hotkey_pressed(raw_requested)
      }
      AppEvent::MenuToggleAlwaysListening => self.handle_menu_toggle_always_listening(),
      AppEvent::MenuToggleDevices => self.handle_menu_toggle_devices(),
      AppEvent::MenuSelectDevice(device_id) => self.handle_menu_select_device(device_id),
      AppEvent::MenuOpenSettings => self.handle_menu_open_settings(),
      AppEvent::MenuOpened => self.handle_menu_opened(),
      AppEvent::MenuClosed => self.handle_menu_closed(),
      AppEvent::SettingsToggleRunOnStartup(enabled) => {
        self.handle_settings_toggle_run_on_startup(enabled)
      }
      AppEvent::SettingsToggleDebugStats(enabled) => {
        self.handle_settings_toggle_debug_stats(enabled)
      }
      AppEvent::SettingsSelectPasteMethod(method) => {
        self.handle_settings_select_paste_method(method)
      }
      AppEvent::SettingsSelectAutoSubmit(mode) => self.handle_settings_select_auto_submit(mode),
      AppEvent::SettingsToggleAppendTrailingSpace(enabled) => {
        self.handle_settings_toggle_append_trailing_space(enabled)
      }
      AppEvent::SettingsAddRemovedWord(word) => self.handle_settings_add_removed_word(word),
      AppEvent::SettingsRemoveRemovedWord(word) => self.handle_settings_remove_removed_word(word),
      AppEvent::SettingsRefresh => self.handle_settings_refresh(),
      AppEvent::SettingsDownloadModel(pack_id) => self.handle_settings_download_model(&pack_id),
      AppEvent::SettingsCancelDownload => self.handle_settings_cancel_download(),
      AppEvent::ModelDownloadProgress { pack_id, bytes_done, bytes_total } => {
        self.handle_model_download_progress(&pack_id, bytes_done, bytes_total)
      }
      AppEvent::ModelDownloadCompleted(pack_id) => self.handle_model_download_completed(&pack_id),
      AppEvent::ModelDownloadError { pack_id, message } => {
        self.handle_model_download_error(&pack_id, &message)
      }
      AppEvent::OverlayCancel => self.handle_overlay_cancel(),
      AppEvent::ArrowNavigate(direction) => self.handle_arrow_navigate(direction),
      AppEvent::Speech(ev) => self.handle_speech_event(ev),
      AppEvent::Device(ev) => self.handle_device_event(ev),
    }
  }

  fn start_session(&mut self) {
    if !self.models_ready {
      return;
    }
    let Some(snapshot) = self.device_snapshot.as_ref() else {
      self.session = None;
      self.session_device_id = None;
      return;
    };

    if snapshot.devices.is_empty() {
      self.session = None;
      self.session_device_id = None;
      return;
    }

    let session_id = self.next_session_id;
    self.next_session_id = self.next_session_id.saturating_add(1);
    self.latest_draft.clear();
    self.finalizing_draft.clear();
    self.finalizing_activity_history.resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
    self.latest_final = None;
    self.finalizing_deadline = None;
    self.finalizing_turn_id = None;
    self.clear_held_top_overlay();
    self.raw_handled_turn_id = None;
    self.deferred_vad_start = false;
    self.accessibility_notice_deadline = None;
    self.last_pasted_turn_id = None;
    self.cancelled = false;
    self.hold_saw_speech = false;
    self.overlay_pending_vad_text = false;
    self.latest_seen_turn_id = 0;
    self.turn_accept_floor = 1;
    self.current_turn_id = None;
    self.dispatch_hotkey_input(HotkeyInput::SessionReset);
    self.raw_finalize_requested = false;
    self.reset_activity_history();
    self.busy_border_phase = 0.0;
    self.turn_started_at.clear();
    self.turn_finalize_outcomes.clear();

    let device_id = self.current_device_id().map(ToOwned::to_owned);
    let emit: Arc<dyn Fn(SpeechEvent) + Send + Sync> =
      Arc::new(|ev| send_event(AppEvent::Speech(ev)));
    match spawn_speech_session(
      session_id,
      self.cfg.to_session_config(
        device_id.clone(),
        self.always_listening_enabled,
        self.always_listening_enabled,
        self.debug_stats_enabled,
      ),
      emit,
    ) {
      Ok(session) => {
        session.set_auto_vad_enabled(self.always_listening_enabled);
        session.set_capture_enabled(self.always_listening_enabled);
        session.set_debug_stats_enabled(self.debug_stats_enabled);
        self.session = Some(session);
        self.session_device_id = device_id;
      }
      Err(err) => {
        eprintln!("Azad: failed to start speech session: {err}");
        self.session = None;
        self.session_device_id = None;
      }
    }
  }

  fn current_session_id(&self) -> Option<u64> {
    self.session.as_ref().map(|s| s.session_id)
  }

  fn ensure_session(&mut self) {
    if !self.models_ready {
      return;
    }
    if self.session.is_none() {
      self.start_session();
    }
  }

  fn restart_session_for_device_change(&mut self) {
    if let Some(session) = &self.session {
      session.cancel();
    }

    self.session = None;
    self.session_device_id = None;
    self.start_session();

    if self.manual_hold_active {
      if let Some(session) = &self.session {
        session.set_capture_enabled(true);
        session.start_or_resume_manual_hold();
      }
      self.show_overlay_listening();
    }
  }

  fn handle_hotkey_pressed(&mut self) {
    if !self.models_ready {
      self.show_overlay_notice(
        "Models required",
        "Open Settings to download",
        Duration::from_secs(3),
      );
      return;
    }
    self.cancel_vad_show_suppressed_until = None;
    self.dispatch_hotkey_input(HotkeyInput::HoldPressed {
      now_ms: self.hotkey_now_ms(),
      snapshot: self.hotkey_snapshot(),
    });
  }

  fn handle_hotkey_released(&mut self) {
    if self.history_browsing {
      // Release-to-paste mirrors the speech-mode finalize gesture: hold opt+space,
      // navigate, let go to commit. `paste_from_history` exits history mode after
      // pasting, so the overlay also closes.
      self.paste_from_history();
      return;
    }
    if !self.models_ready {
      return;
    }
    self.dispatch_hotkey_input(HotkeyInput::HoldReleased { snapshot: self.hotkey_snapshot() });
  }

  fn handle_finalize_hotkey_pressed(&mut self, raw_requested: bool) {
    if self.history_browsing {
      self.paste_from_history();
      return;
    }
    if !self.models_ready {
      return;
    }
    if raw_requested {
      self.raw_finalize_requested = true;
    }
    self.dispatch_hotkey_input(HotkeyInput::FinalizePressed {
      overlay_visible: self.actionable_overlay_visible(),
    });
    if raw_requested {
      let turn_id = self.raw_finalize_target_turn_id();
      let _ = self.try_finalize_with_raw_text(turn_id);
    }
  }

  fn handle_menu_toggle_always_listening(&mut self) {
    self.dispatch_hotkey_input(HotkeyInput::MenuToggleAlwaysListening);
  }

  fn has_active_transcription_turn(&self) -> bool {
    let has_started_turn = has_started_turn_for_snapshot(
      self.manual_hold_active,
      self.hold_saw_speech,
      self.engine_state,
      self.finalizing_turn_id,
      &self.latest_draft,
    );
    has_started_turn
      && has_actionable_turn_context_for_snapshot(
        self.engine_state,
        self.current_turn_id,
        self.finalizing_turn_id,
        &self.latest_draft,
        self.overlay_visible,
        self.manual_hold_active,
      )
  }

  fn next_menu_toggle_target(&self) -> bool {
    !self.pending_always_listening_enabled.unwrap_or(self.always_listening_enabled)
  }

  fn apply_always_listening_state(&mut self, enabled: bool, show_toggle_notice: bool) {
    self.always_listening_enabled = enabled;
    preferred_store::save_always_listening_enabled(enabled);
    if enabled && !self.ensure_paste_accessibility_or_disable_listening() {
      return;
    }

    self.ensure_session();
    if let Some(session) = &self.session {
      session.set_auto_vad_enabled(self.always_listening_enabled);
      let should_capture = self.always_listening_enabled || self.manual_hold_active;
      session.set_capture_enabled(should_capture);
    }
    self.overlay_pending_vad_text = false;
    self.render_device_menu();
    if show_toggle_notice && !cfg!(test) {
      self.show_listen_toggle_notice(
        enabled,
        Duration::from_millis(LISTEN_TOGGLE_NOTICE_DURATION_MS),
      );
    }
  }

  fn maybe_apply_pending_always_listening_toggle(&mut self) {
    let Some(target) = self.pending_always_listening_enabled else {
      return;
    };
    if self.has_active_transcription_turn() {
      return;
    }
    self.pending_always_listening_enabled = None;
    if target == self.always_listening_enabled {
      self.render_device_menu();
      return;
    }
    self.apply_always_listening_state(target, true);
  }

  fn interrupt_current_turn_for_hotkey_toggle(&mut self) {
    self.manual_hold_active = false;
    self.hold_saw_speech = false;
    self.pending_always_listening_enabled = None;
    self.raw_finalize_requested = false;
    self.overlay_pending_vad_text = false;
    self.clear_held_top_overlay();

    if let Some(session) = &self.session {
      session.release_manual_hold();
      session.cancel_current_turn();
    }

    self.hide_overlay();
    self.reset_turn_state();
  }

  fn apply_always_listening_toggle(&mut self) {
    let target = !self.always_listening_enabled;
    self.apply_always_listening_state(target, true);
  }

  fn hotkey_now_ms(&self) -> u64 {
    let start = HOTKEY_CLOCK_START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
  }

  fn hotkey_snapshot(&self) -> RuntimeSnapshot {
    let has_active_speech_turn =
      self.engine_state == EngineState::Speech && self.finalizing_turn_id.is_none();
    let has_turn_context = has_actionable_turn_context_for_snapshot(
      self.engine_state,
      self.current_turn_id,
      self.finalizing_turn_id,
      &self.latest_draft,
      self.overlay_visible,
      self.manual_hold_active,
    );
    let has_started_turn = has_started_turn_for_snapshot(
      self.manual_hold_active,
      self.hold_saw_speech,
      self.engine_state,
      self.finalizing_turn_id,
      &self.latest_draft,
    );
    RuntimeSnapshot {
      always_listening_enabled: self.always_listening_enabled,
      has_active_speech_turn,
      has_turn_context,
      has_started_turn,
      overlay_visible: self.overlay_visible,
      manual_hold_active: self.manual_hold_active,
    }
  }

  fn split_overlay_active(&self) -> bool {
    split_overlay_active_for_turns(self.finalizing_turn_id, self.current_turn_id)
  }

  fn held_top_overlay_active(&self) -> bool {
    self.held_top_active && !self.held_top_draft.trim().is_empty()
  }

  fn clear_held_top_overlay(&mut self) {
    self.held_top_active = false;
    self.held_top_draft.clear();
    self.saw_vad_start_during_finalizing = false;
  }

  fn split_overlay_visible(&self) -> bool {
    split_overlay_visible_with_vad_hint_for_state(
      self.finalizing_turn_id,
      self.current_turn_id,
      &self.latest_draft,
      self.held_top_overlay_active(),
      self.saw_vad_start_during_finalizing,
    ) || split_overlay_visible_with_live_divergence_for_state(
      self.finalizing_turn_id,
      &self.latest_draft,
      &self.finalizing_draft,
    )
  }

  fn actionable_overlay_visible(&self) -> bool {
    if !self.overlay_visible {
      return false;
    }
    if self.split_overlay_active() {
      return self.split_overlay_visible()
        || self.engine_state == EngineState::Speech
        || self.manual_hold_active;
    }
    true
  }

  fn raw_finalize_target_turn_id(&self) -> Option<u64> {
    raw_finalize_target_turn_id_for_state(
      self.finalizing_turn_id,
      self.current_turn_id,
      self.latest_seen_turn_id,
      &self.latest_draft,
    )
  }

  fn clear_live_lane_state(&mut self) {
    self.cancelled = false;
    self.last_pasted_turn_id = None;
    self.hold_saw_speech = false;
    self.latest_draft.clear();
    self.finalizing_draft.clear();
    self.finalizing_activity_history.resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
    self.latest_final = None;
    self.raw_handled_turn_id = None;
    self.raw_finalize_requested = false;
    self.deferred_vad_start = false;
    self.accessibility_notice_deadline = None;
    self.overlay_pending_vad_text = false;
    self.clear_held_top_overlay();
    self.current_turn_id = self.finalizing_turn_id;
    self.turn_accept_floor = self
      .finalizing_turn_id
      .unwrap_or_else(|| self.latest_seen_turn_id.saturating_add(1));
    self.reset_activity_history();
  }

  fn dispatch_hotkey_input(&mut self, input: HotkeyInput) {
    let effects = self.hotkey_state.reduce(input);
    for effect in effects {
      self.apply_hotkey_effect(effect);
    }
  }

  fn apply_hotkey_effect(&mut self, effect: HotkeyEffect) {
    match effect {
      HotkeyEffect::InterruptAndToggleAlwaysListening => {
        self.interrupt_current_turn_for_hotkey_toggle();
        self.apply_always_listening_toggle();
      }
      HotkeyEffect::MenuToggleAlwaysListening => {
        let target = self.next_menu_toggle_target();
        if self.has_active_transcription_turn() {
          self.pending_always_listening_enabled = Some(target);
          self.render_device_menu();
        } else {
          self.pending_always_listening_enabled = None;
          self.apply_always_listening_state(target, true);
        }
      }
      HotkeyEffect::ActivateManualHold { reset_turn_state, release_should_finalize: _ } => {
        if !self.ensure_paste_accessibility_or_disable_listening() {
          return;
        }
        self.manual_hold_active = true;
        self.hold_saw_speech = false;
        self.overlay_pending_vad_text = false;
        if reset_turn_state {
          self.reset_turn_state_preserving_hotkey_state();
        }
        self.ensure_session();
        if let Some(session) = &self.session {
          session.set_capture_enabled(true);
          session.start_or_resume_manual_hold();
        }
        self.show_overlay_listening();
      }
      HotkeyEffect::ReleaseManualHold { should_finalize, has_started_turn } => {
        self.manual_hold_active = false;
        self.hold_saw_speech = false;
        let plan = manual_hold_release_plan(
          self.always_listening_enabled,
          should_finalize,
          has_started_turn,
        );
        if let Some(session) = &self.session {
          session.release_manual_hold();
          // Keep capture state in sync immediately on hold release so
          // hotkey-driven listen disable mirrors menu-toggle behavior.
          session.set_capture_enabled(plan.capture_enabled);
          match plan.action {
            ManualHoldReleaseAction::FinalizeTurn => session.finalize_current_turn(),
            ManualHoldReleaseAction::HideOverlay => {
              self.hide_overlay();
              self.reset_turn_state();
            }
            ManualHoldReleaseAction::KeepLive => {}
          }
        } else if should_finalize {
          // If no live session exists, treat release-finalize as an empty
          // turn cleanup so overlay state cannot get stuck open.
          self.hide_overlay();
          self.reset_turn_state();
        }
      }
      HotkeyEffect::FinalizeFromHotkey => {
        if !self.overlay_visible {
          return;
        }
        self.manual_hold_active = false;
        self.hold_saw_speech = false;
        if let Some(session) = &self.session {
          session.release_manual_hold();
          session.finalize_current_turn();
        }
      }
    }
  }

  fn handle_menu_toggle_devices(&mut self) {
    self.device_menu_expanded = !self.device_menu_expanded;
    self.render_device_menu();
  }

  fn handle_menu_select_device(&mut self, device_id: String) {
    preferred_store::save_preferred_device_id(&device_id);

    if let Some(controller) = &self.device_controller {
      if let Err(err) = controller.set_preferred(Some(device_id)) {
        eprintln!("Azad: failed to set preferred device: {err}");
      }
    }
  }

  fn handle_menu_open_settings(&mut self) {
    platform::show_settings_window(self.settings_view_model());
  }

  fn apply_run_on_startup_preference(&mut self) {
    platform::create_launch_agent_plist_if_missing();
    if platform::set_launch_agent_startup_enabled(self.run_on_startup_enabled) {
      return;
    }
    eprintln!(
      "Azad: failed to apply run-on-startup preference (enabled={})",
      self.run_on_startup_enabled
    );
  }

  fn handle_settings_toggle_run_on_startup(&mut self, enabled: bool) {
    if platform::set_launch_agent_startup_enabled(enabled) {
      self.run_on_startup_enabled = enabled;
      preferred_store::save_run_on_startup_enabled(enabled);
    } else {
      eprintln!("Azad: failed to set run-on-startup to {enabled}");
    }
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_toggle_debug_stats(&mut self, enabled: bool) {
    self.debug_stats_enabled = enabled;
    preferred_store::save_debug_stats_enabled(enabled);
    if let Some(session) = &self.session {
      session.set_debug_stats_enabled(enabled);
    }
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_select_paste_method(&mut self, method: PasteMethod) {
    self.paste_method = method;
    preferred_store::save_paste_method(method);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_select_auto_submit(&mut self, mode: AutoSubmitMode) {
    self.auto_submit_mode = mode;
    preferred_store::save_auto_submit_mode(mode);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_toggle_append_trailing_space(&mut self, enabled: bool) {
    self.append_trailing_space_on_paste = enabled;
    preferred_store::save_append_trailing_space_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_add_removed_word(&mut self, word: String) {
    let word = word.trim().to_ascii_lowercase();
    if word.is_empty() || self.removed_words.iter().any(|w| w == &word) {
      return;
    }
    self.removed_words.push(word);
    preferred_store::save_removed_words(&self.removed_words);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_remove_removed_word(&mut self, word: String) {
    self.removed_words.retain(|w| w != &word);
    preferred_store::save_removed_words(&self.removed_words);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_refresh(&mut self) {
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_download_model(&mut self, pack_id: &str) {
    if self.download_handle.is_some() {
      return;
    }
    let Some(pack) = models::pack_by_id(pack_id) else {
      return;
    };
    self.active_pack_id = pack_id.to_string();
    preferred_store::save_active_model_pack(pack_id);
    self.download_progress = (0, pack.total_size_bytes);
    self.download_handle = Some(model_download::start_pack_download(pack));
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_cancel_download(&mut self) {
    if let Some(handle) = self.download_handle.take() {
      handle.cancel();
    }
    self.download_progress = (0, 0);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_model_download_progress(&mut self, _pack_id: &str, bytes_done: u64, bytes_total: u64) {
    self.download_progress = (bytes_done, bytes_total);
    self.download_progress_dirty = true;
  }

  fn handle_model_download_completed(&mut self, pack_id: &str) {
    self.download_handle = None;
    self.download_progress = (0, 0);
    self.active_pack_id = pack_id.to_string();
    preferred_store::save_active_model_pack(pack_id);
    self.refresh_models_ready();
    platform::update_settings_window(self.settings_view_model());
    if self.models_ready {
      self.ensure_session();
    }
  }

  fn handle_model_download_error(&mut self, _pack_id: &str, message: &str) {
    eprintln!("Azad: model download error: {message}");
    self.download_handle = None;
    self.download_progress = (0, 0);
    platform::update_settings_window(self.settings_view_model());
  }

  fn settings_view_model(&self) -> SettingsViewModel {
    let metrics_text = match metrics_log::summarize_last_24h() {
      Ok(summary) => metrics_log::render_summary(&summary),
      Err(err) => format!("Failed to load debug metrics: {err}"),
    };

    let pack = models::pack_by_id(&self.active_pack_id).unwrap_or_else(models::default_pack);
    let pack_status = if self.download_handle.is_some() {
      let pct = if self.download_progress.1 > 0 {
        ((self.download_progress.0 as f64 / self.download_progress.1 as f64) * 100.0) as u8
      } else {
        0
      };
      PackStatus::Downloading { progress_pct: pct }
    } else {
      models::check_pack_status(pack)
    };

    SettingsViewModel {
      selected_tab: SettingsTab::General,
      run_on_startup_enabled: self.run_on_startup_enabled,
      paste_method: self.paste_method,
      auto_submit_mode: self.auto_submit_mode,
      append_trailing_space_on_paste: self.append_trailing_space_on_paste,
      debug_stats_enabled: self.debug_stats_enabled,
      metrics_text,
      model_pack_size_label: models::format_size(pack.total_size_bytes),
      model_pack_status: pack_status,
      model_download_bytes_done: self.download_progress.0,
      model_download_bytes_total: self.download_progress.1,
      removed_words: self.removed_words.clone(),
    }
  }

  fn handle_menu_opened(&mut self) {
    if let Some(controller) = &self.device_controller {
      let _ = controller.refresh_now();
    }
  }

  fn handle_menu_closed(&mut self) {
    if !self.device_menu_expanded {
      return;
    }
    self.device_menu_expanded = false;
    self.render_device_menu();
  }

  fn handle_overlay_cancel(&mut self) {
    if self.history_browsing {
      self.exit_history_mode();
      return;
    }
    if !self.overlay_visible {
      return;
    }
    let split_active = self.split_overlay_visible();
    self.cancelled = true;
    self.cancel_vad_show_suppressed_until =
      Some(Instant::now() + Duration::from_millis(CANCEL_VAD_SHOW_SUPPRESSION_MS));
    self.manual_hold_active = false;
    self.hold_saw_speech = false;
    self.dispatch_hotkey_input(HotkeyInput::OverlayCancelled);
    self.raw_finalize_requested = false;
    self.overlay_pending_vad_text = false;
    self.clear_held_top_overlay();
    if !split_active {
      self.finalizing_deadline = None;
      self.finalizing_turn_id = None;
      self.finalizing_draft.clear();
    }
    self.raw_handled_turn_id = None;
    self.turn_started_at.clear();
    if let Some(session) = &self.session {
      session.release_manual_hold();
      session.cancel_current_turn();
      if !self.always_listening_enabled {
        session.set_capture_enabled(false);
      }
    }
    if split_active {
      self.clear_live_lane_state();
      self.render_finalizing_overlay_state();
    } else {
      self.hide_overlay();
    }
  }

  fn handle_device_event(&mut self, event: DeviceEvent) {
    match event {
      DeviceEvent::StateChanged(snapshot) => self.handle_device_state_changed(snapshot),
      DeviceEvent::Error(message) => {
        eprintln!("Azad: device event error: {message}");
      }
    }
  }

  fn handle_device_state_changed(&mut self, snapshot: DeviceStateSnapshot) {
    self.device_snapshot = Some(snapshot);
    self.render_device_menu();

    let snapshot = self.device_snapshot.as_ref().unwrap();
    if snapshot.devices.is_empty() {
      if let Some(session) = &self.session {
        session.cancel();
      }
      self.session = None;
      self.session_device_id = None;
      self.pending_device_switch_target = None;
      self.pending_device_switch_deadline = None;
      return;
    }

    if self.session.is_none() {
      self.start_session();
    }

    let Some(next_current) = self.current_device_id().map(ToOwned::to_owned) else {
      // Device updates can briefly report "no current" while CoreAudio settles.
      // Keep the current stream alive and wait for a concrete current device id.
      return;
    };

    if self.session.is_none() {
      return;
    }

    if self.session_device_id.as_deref() != Some(next_current.as_str()) {
      self.pending_device_switch_target = Some(next_current);
      self.pending_device_switch_deadline =
        Some(Instant::now() + Duration::from_millis(DEVICE_SWITCH_RESTART_DEBOUNCE_MS));
    }

    if self.session.is_none() && self.pending_recovery_restart {
      eprintln!(
        "Azad: recovery restart triggered by device update state={:?}",
        self.session_recovery_state
      );
      self.pending_recovery_restart = false;
      self.start_session();
    }
  }

  fn render_device_menu(&self) {
    let mut model = DeviceMenuModel {
      always_listening_enabled: self.always_listening_enabled,
      header_label: "No Input Device".to_string(),
      expanded: self.device_menu_expanded,
      rows: Vec::new(),
    };

    if let Some(snapshot) = &self.device_snapshot {
      if let Some(current_id) = snapshot.current_id.as_deref() {
        if let Some(current) = snapshot.devices.iter().find(|d| d.id == current_id) {
          model.header_label = current.name.clone();
        }
      }

      let current_id = snapshot.current_id.as_deref();
      let mut rows = snapshot
        .devices
        .iter()
        .map(|d| DeviceMenuRow {
          id: d.id.clone(),
          label: d.name.clone(),
          checked: Some(d.id.as_str()) == current_id,
        })
        .collect::<Vec<_>>();

      rows.sort_by(|a, b| {
        let a_current = a.checked;
        let b_current = b.checked;
        b_current
          .cmp(&a_current)
          .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
      });

      model.rows = rows;
    }

    platform::set_device_menu(model);
  }

  fn handle_speech_event(&mut self, event: SpeechEvent) {
    let event_session_id = match &event {
      SpeechEvent::SessionStarted { session_id }
      | SpeechEvent::Listening { session_id }
      | SpeechEvent::SpeechStartedByVad { session_id }
      | SpeechEvent::DraftUpdated { session_id, .. }
      | SpeechEvent::Finalizing { session_id, .. }
      | SpeechEvent::FinalText { session_id, .. }
      | SpeechEvent::SessionEnded { session_id }
      | SpeechEvent::Error { session_id, .. }
      | SpeechEvent::Status { session_id, .. }
      | SpeechEvent::Meter { session_id, .. }
      | SpeechEvent::DebugStats { session_id, .. } => *session_id,
    };

    if Some(event_session_id) != self.current_session_id() {
      return;
    }

    // History-browse mode owns the overlay. Speech events that would render
    // into the overlay (DraftUpdated, Meter, Finalizing, FinalText), hide it
    // (SessionEnded), or otherwise compete for it must be dropped here.
    // Without this guard, an in-flight turn cancelled by `enter_history_mode`
    // continues firing events as the worker thread drains, stomping the
    // history list with old draft text — the user-reported "overlay freezes"
    // / "transcription disappears" symptoms. We accept the consequence that
    // any speech captured while history is open won't surface; the user
    // pivoted away from speaking on purpose.
    if self.history_browsing {
      return;
    }

    match event {
      SpeechEvent::SessionStarted { .. } => {}
      SpeechEvent::Listening { .. } => {
        if !self.ensure_paste_accessibility_or_disable_listening() {
          return;
        }
        if self.overlay_visible {
          self.render_listening_overlay();
        }
      }
      SpeechEvent::SpeechStartedByVad { .. } => {
        self.note_stable_stream_progress();
        if self.finalizing_turn_id.is_some() {
          // Keep finalizing lane visible and prepare a fresh live lane below it.
          self.saw_vad_start_during_finalizing = true;
          self.latest_draft.clear();
          self.latest_final = None;
          self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
          self.reset_activity_history();
          return;
        }
        self.saw_vad_start_during_finalizing = false;
        self.reset_turn_state();
        if self.overlay_visible {
          self.hide_overlay();
        }
        // In auto-VAD mode, wait for actual draft text before showing overlay.
        self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
      }
      SpeechEvent::DraftUpdated { turn_id, committed, live, .. } => {
        if !self.accept_turn(turn_id) {
          return;
        }
        self.note_stable_stream_progress();
        self.observe_turn(turn_id);
        let merged = format!("{committed}{live}");
        let merged = merged.trim().to_string();
        if !merged.is_empty() {
          if self.manual_hold_active {
            self.hold_saw_speech = true;
          }
          if self.held_top_overlay_active() {
            self.clear_held_top_overlay();
          }
          self.latest_draft = merged;
          let cancel_suppression_active = self
            .cancel_vad_show_suppressed_until
            .is_some_and(|deadline| Instant::now() < deadline);
          match draft_update_overlay_action(
            self.overlay_pending_vad_text,
            self.overlay_visible,
            cancel_suppression_active,
          ) {
            DraftOverlayAction::Show => {
              self.show_overlay_listening();
              self.overlay_pending_vad_text = false;
            }
            DraftOverlayAction::Clear => {
              self.overlay_pending_vad_text = false;
            }
            DraftOverlayAction::KeepPendingForLater => {
              // Leave `overlay_pending_vad_text` alone; the next DraftUpdate after the
              // cancel-suppression window expires will re-evaluate and show the overlay.
            }
          }
        }
        if self.overlay_visible {
          if self.finalizing_deadline.is_some() {
            self.render_finalizing_overlay_state();
          } else {
            self.render_listening_overlay();
          }
        }
      }
      SpeechEvent::Meter { peak_db, vad_speech, vad_prob, .. } => {
        self.update_activity_from_meter(peak_db, vad_speech, vad_prob);
        if self.overlay_visible && self.accessibility_notice_deadline.is_none() {
          if self.finalizing_deadline.is_some() {
            self.render_finalizing_overlay_state();
          } else {
            self.render_listening_overlay();
          }
        }
      }
      SpeechEvent::DebugStats { event, .. } => {
        self.handle_debug_stats_event(event);
      }
      SpeechEvent::Finalizing { turn_id, current_draft, .. } => {
        if !self.accept_turn(turn_id) {
          return;
        }
        if self.finalizing_turn_id.is_some_and(|existing_finalizing_turn_id| {
          turn_id > existing_finalizing_turn_id && self.current_turn_id == Some(turn_id)
        }) {
          // Keep one finalizing lane authoritative at a time.
          self.deferred_vad_start = true;
          return;
        }
        if should_ignore_finalizing_event(self.raw_handled_turn_id, turn_id) {
          self.finalizing_turn_id = None;
          self.finalizing_deadline = None;
          self.finalizing_draft.clear();
          return;
        }

        self.observe_turn(turn_id);
        if !current_draft.trim().is_empty() {
          if self.manual_hold_active {
            self.hold_saw_speech = true;
          }
          self.finalizing_draft = current_draft.trim().to_string();
          if self.current_turn_id == Some(turn_id) {
            self.latest_draft = self.finalizing_draft.clone();
          }
        } else if self.finalizing_draft.is_empty() {
          self.finalizing_draft = self.latest_draft.clone();
        }
        self.finalizing_activity_history.clone_from(&self.activity_history);
        self.finalizing_turn_id = Some(turn_id);
        self.clear_held_top_overlay();
        self.raw_handled_turn_id = None;
        // If we never surfaced any draft text in auto mode, keep overlay hidden.
        // This avoids noise-only VAD turns flashing the overlay.
        let has_visible_text = !self.finalizing_draft.trim().is_empty();
        if !has_visible_text && !self.overlay_visible && !self.manual_hold_active {
          self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
          self.finalizing_deadline = None;
          return;
        }

        let raw_requested = self.raw_finalize_requested || platform::is_raw_mode_pressed();
        if raw_requested && has_visible_text {
          self.raw_finalize_requested = true;
          if self.try_finalize_with_raw_text(Some(turn_id)) {
            return;
          }
        }

        self.overlay_pending_vad_text = false;
        self.finalizing_deadline =
          Some(Instant::now() + Duration::from_millis(self.cfg.final_pass_timeout_ms));
        self.render_finalizing_overlay_state();
      }
      SpeechEvent::FinalText { turn_id, text, .. } => {
        if !self.accept_turn(turn_id) {
          return;
        }

        let cleaned = text.trim().to_string();
        let hold_top_for_next_turn = self.finalizing_turn_id == Some(turn_id)
          && self.saw_vad_start_during_finalizing
          && self.latest_draft.trim().is_empty()
          && !cleaned.is_empty();
        let split_top_completion = split_top_completion_for_state(
          self.finalizing_turn_id,
          self.current_turn_id,
          &self.latest_draft,
          self.held_top_overlay_active(),
          self.saw_vad_start_during_finalizing,
          turn_id,
          &cleaned,
        ) || split_overlay_visible_with_live_divergence_for_state(
          self.finalizing_turn_id,
          &self.latest_draft,
          &self.finalizing_draft,
        );
        if !split_top_completion {
          self.observe_turn(turn_id);
        }
        self.finalizing_turn_id = if split_top_completion {
          None
        } else {
          self.finalizing_turn_id.and_then(|id| (id != turn_id).then_some(id))
        };
        self.finalizing_deadline =
          if self.finalizing_turn_id.is_some() { self.finalizing_deadline } else { None };
        if split_top_completion || self.finalizing_turn_id.is_none() {
          self.finalizing_draft.clear();
        }
        self.raw_finalize_requested = false;
        if !split_top_completion {
          self.dispatch_hotkey_input(HotkeyInput::SpeechFinalized);
        }
        if self.finalizing_turn_id != Some(turn_id) {
          self.saw_vad_start_during_finalizing = false;
        }

        if hold_top_for_next_turn {
          self.held_top_draft = cleaned.clone();
          self.held_top_active = true;
        }

        if split_top_completion {
          self.turn_started_at.remove(&turn_id);
          self.raw_handled_turn_id = None;
          if !cleaned.is_empty() && !self.cancelled && self.last_pasted_turn_id != Some(turn_id) {
            if self.try_paste(turn_id, TranscriptMode::Normal, &cleaned) {
              self.last_pasted_turn_id = Some(turn_id);
              if let Some(index) = &mut self.transcript_index {
                index.append(turn_id, &self.finalizing_draft, &cleaned);
              }
            } else {
              eprintln!("Azad: failed to auto-paste transcript (clipboard still contains text)");
            }
          }
          if self.overlay_visible {
            self.render_listening_overlay();
          }
          return;
        }

        if cleaned.is_empty() {
          self.clear_held_top_overlay();
          self.turn_started_at.remove(&turn_id);
          self.raw_handled_turn_id = None;
          self.maybe_start_deferred_vad_turn();
          if !self.always_listening_enabled && !self.manual_hold_active {
            if let Some(session) = &self.session {
              session.set_capture_enabled(false);
            }
          }
          return;
        }
        if self.raw_handled_turn_id == Some(turn_id) {
          self.clear_held_top_overlay();
          self.turn_started_at.remove(&turn_id);
          self.raw_handled_turn_id = None;
          self.maybe_start_deferred_vad_turn();
          if !self.always_listening_enabled && !self.manual_hold_active {
            if let Some(session) = &self.session {
              session.set_capture_enabled(false);
            }
          }
          return;
        }
        self.raw_handled_turn_id = None;
        self.latest_final = Some(cleaned.clone());
        if !self.cancelled && self.last_pasted_turn_id != Some(turn_id) {
          // Keep the finalizing spinner on screen through the paste window so the visual
          // transition coincides with the paste appearing in the target app. Hiding first and
          // then doing a ~100 ms blocking paste creates a perceptible "overlay gone / nothing
          // happening / paste appears" gap; hiding after leaves the overlay responsible for
          // "still working" state right up until the moment the text lands.
          let should_hide_overlay = !self.manual_hold_active && !hold_top_for_next_turn;
          if self.try_paste(turn_id, TranscriptMode::Normal, &cleaned) {
            self.last_pasted_turn_id = Some(turn_id);
            if let Some(index) = &mut self.transcript_index {
              index.append(turn_id, &self.finalizing_draft, &cleaned);
            }
          } else {
            eprintln!("Azad: failed to auto-paste transcript (clipboard still contains text)");
          }
          if should_hide_overlay {
            self.hide_overlay();
          }
        }
        self.maybe_start_deferred_vad_turn();
        if !self.always_listening_enabled && !self.manual_hold_active {
          if let Some(session) = &self.session {
            session.set_capture_enabled(false);
          }
        }
      }
      SpeechEvent::SessionEnded { session_id } => {
        let should_restart = self.should_restart_after_session_end() || self.manual_hold_active;
        if self.debug_stats_enabled {
          eprintln!(
            "AZAD_SESSION_ENDED session_id={session_id} should_restart={should_restart} \
             always_listening={} manual_hold_active={} last_was_stream_fault={}",
            self.always_listening_enabled,
            self.manual_hold_active,
            self.last_session_error_was_stream_fault,
          );
        }
        self.engine_state = EngineState::Idle;
        if !self.cancelled
          && self.latest_seen_turn_id > 0
          && self.last_pasted_turn_id != Some(self.latest_seen_turn_id)
        {
          // Paste-then-hide: the overlay's "still working" state stays on screen until the
          // paste actually lands, so dismissal and paste appear on the same frame.
          if let Some(final_text) = self.latest_final.as_ref() {
            let cleaned = final_text.trim().to_string();
            if !cleaned.is_empty() {
              if self.try_paste(self.latest_seen_turn_id, TranscriptMode::Normal, &cleaned) {
                self.last_pasted_turn_id = Some(self.latest_seen_turn_id);
              }
            }
          }
          self.hide_overlay();
        }

        self.hide_overlay();
        self.session = None;
        self.latest_draft.clear();
        self.finalizing_draft.clear();
        self.finalizing_activity_history.resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
        self.latest_final = None;
        self.finalizing_deadline = None;
        self.finalizing_turn_id = None;
        self.clear_held_top_overlay();
        self.raw_handled_turn_id = None;
        self.raw_finalize_requested = false;
        self.hold_saw_speech = false;
        self.deferred_vad_start = false;
        self.accessibility_notice_deadline = None;
        self.overlay_pending_vad_text = false;
        self.cancelled = false;
        self.last_pasted_turn_id = None;
        self.session_device_id = None;
        self.dispatch_hotkey_input(HotkeyInput::SessionReset);
        self.reset_activity_history();
        self.busy_border_phase = 0.0;
        self.turn_started_at.clear();

        if should_restart {
          self.start_session();

          if self.manual_hold_active {
            if let Some(session) = &self.session {
              session.set_capture_enabled(true);
              session.start_or_resume_manual_hold();
            }
            self.show_overlay_listening();
          } else if !self.always_listening_enabled {
            if let Some(session) = &self.session {
              session.set_capture_enabled(false);
            }
          }
        } else if self.pending_recovery_restart {
          self.show_overlay_notice(
            "Listening recovering",
            "Waiting for audio device change or hold hotkey retry",
            Duration::from_millis(LISTEN_RECOVERING_NOTICE_DURATION_MS),
          );
        }
      }
      SpeechEvent::Error { message, .. } => {
        if is_stream_fault_message(&message) {
          self.note_stream_fault(&message);
        } else if self.overlay_visible {
          platform::set_overlay_notice_content("Error", &message);
        }
      }
      SpeechEvent::Status { state, detail, .. } => {
        let _ = detail;
        self.engine_state = state;
        if matches!(state, EngineState::Idle) {
          self.dispatch_hotkey_input(HotkeyInput::SpeechIdle {
            manual_hold_active: self.manual_hold_active,
          });
        }
        if matches!(state, EngineState::Idle)
          && self.overlay_visible
          && self.finalizing_deadline.is_none()
          && self.accessibility_notice_deadline.is_none()
          && !self.manual_hold_active
          && self.latest_draft.trim().is_empty()
        {
          // Empty/noisy VAD turns can end without a final-pass event. Close the
          // overlay when engine reports idle and there is no draft to finalize.
          self.hide_overlay();
          if !self.always_listening_enabled && !self.manual_hold_active {
            if let Some(session) = &self.session {
              session.set_capture_enabled(false);
            }
          }
          return;
        }
      }
    }
  }

  fn on_tick(&mut self) {
    if self.pending_first_launch_settings {
      self.pending_first_launch_settings = false;
      let mut vm = self.settings_view_model();
      vm.selected_tab = SettingsTab::Models;
      platform::show_settings_window(vm);
    }

    self.advance_activity_timeline();
    self.maybe_apply_pending_always_listening_toggle();

    if let Some(deadline) = self.pending_device_switch_deadline {
      if Instant::now() >= deadline {
        self.pending_device_switch_deadline = None;
        let target = self.pending_device_switch_target.take();
        if let Some(target) = target {
          let still_current = self.current_device_id() == Some(target.as_str());
          let needs_restart =
            self.session.is_some() && self.session_device_id.as_deref() != Some(target.as_str());
          if still_current && needs_restart {
            self.restart_session_for_device_change();
          }
        }
      }
    }

    if let Some(deadline) = self.finalizing_deadline {
      let now = Instant::now();
      if now >= deadline {
        // Keep waiting for the real final-pass completion signal instead of hiding
        // the overlay on a fixed timeout.
        self.finalizing_deadline =
          Some(now + Duration::from_millis(self.cfg.final_pass_timeout_ms));
      }

      self.busy_border_phase =
        (self.busy_border_phase + OVERLAY_BUSY_PHASE_STEP).rem_euclid(std::f32::consts::TAU);
      if self.accessibility_notice_deadline.is_none() {
        self.render_finalizing_overlay_state();
      }
    } else if self.overlay_visible && self.accessibility_notice_deadline.is_none() {
      self.render_listening_overlay();
    }

    if let Some(deadline) = self.accessibility_notice_deadline {
      if let Some(notice) = self.listen_toggle_notice {
        let (title, body_segments) = listen_toggle_notice(notice.enabled);
        let elapsed = notice.started_at.elapsed();
        let progress = if notice.duration.is_zero() {
          1.0
        } else {
          (elapsed.as_secs_f32() / notice.duration.as_secs_f32()).clamp(0.0, 1.0)
        };
        platform::set_overlay_listen_toggle_notice_content(
          title,
          &body_segments,
          notice.enabled,
          progress,
        );
      }
      if Instant::now() >= deadline {
        self.accessibility_notice_deadline = None;
        self.listen_toggle_notice = None;
        if self.overlay_visible && !self.manual_hold_active && self.finalizing_deadline.is_none() {
          self.hide_overlay();
        }
      }
    }

    if self.download_progress_dirty {
      self.download_progress_dirty = false;
      platform::update_settings_window(self.settings_view_model());
    }
  }

  fn show_overlay_listening(&mut self) {
    self.overlay_pending_vad_text = false;
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    self.render_listening_overlay();
  }

  fn render_finalizing_overlay_state(&mut self) {
    if self.accessibility_notice_deadline.is_some() {
      return;
    }
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }

    if self.split_overlay_visible() {
      platform::show_overlay_top();
      platform::set_overlay_top_stream_content(
        &self.finalizing_draft,
        &self.finalizing_activity_history,
        Some(self.busy_border_phase),
      );
      platform::set_overlay_stream_content(
        &self.latest_draft,
        &self.activity_history,
        None,
        self.raw_badge_visible(),
        self.hold_badge_visible(),
        "",
      );
      return;
    }

    platform::hide_overlay_top();
    platform::set_overlay_stream_content(
      &self.finalizing_draft,
      &self.finalizing_activity_history,
      Some(self.busy_border_phase),
      self.raw_badge_visible(),
      self.hold_badge_visible(),
      "",
    );
  }

  fn render_listening_overlay(&self) {
    if self.accessibility_notice_deadline.is_some() {
      return;
    }
    let held_active = self.held_top_overlay_active();
    let live_has_text = !self.latest_draft.trim().is_empty();
    if held_active && live_has_text {
      platform::show_overlay_top();
      platform::set_overlay_top_stream_content(
        &self.held_top_draft,
        &self.finalizing_activity_history,
        None,
      );
    } else {
      platform::hide_overlay_top();
    }
    let body_text = if held_active && !live_has_text {
      self.held_top_draft.as_str()
    } else {
      self.latest_draft.as_str()
    };
    platform::set_overlay_stream_content(
      body_text,
      &self.activity_history,
      None,
      self.raw_badge_visible(),
      self.hold_badge_visible(),
      "",
    );
  }

  fn raw_badge_visible(&self) -> bool {
    let raw_pressed = platform::is_raw_mode_pressed();
    if !raw_pressed {
      return false;
    }
    if !self.manual_hold_active {
      return true;
    }
    !platform::hold_hotkey_overlaps_raw_modifier()
  }

  fn hold_badge_visible(&self) -> bool {
    self.manual_hold_active
  }

  fn show_overlay_notice(&mut self, title: &str, body: &str, duration: Duration) {
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    platform::hide_overlay_top();
    self.listen_toggle_notice = None;
    self.accessibility_notice_deadline = Some(Instant::now() + duration);
    platform::set_overlay_notice_content(title, body);
  }

  fn show_listen_toggle_notice(&mut self, enabled: bool, duration: Duration) {
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    platform::hide_overlay_top();
    let (title, body_segments) = listen_toggle_notice(enabled);
    self.listen_toggle_notice =
      Some(ListenToggleNotice { enabled, started_at: Instant::now(), duration });
    self.accessibility_notice_deadline = Some(Instant::now() + duration);
    platform::set_overlay_listen_toggle_notice_content(title, &body_segments, enabled, 0.0);
  }

  fn show_accessibility_overlay_notice(&mut self) {
    self.show_overlay_notice(
      "Auto-paste blocked",
      "Enable Azad in System Settings -> Privacy & Security -> Accessibility",
      Duration::from_secs(6),
    );
  }

  fn disable_listening_due_to_accessibility(&mut self) {
    self.always_listening_enabled = false;
    self.pending_always_listening_enabled = None;
    preferred_store::save_always_listening_enabled(false);
    self.manual_hold_active = false;
    self.hold_saw_speech = false;
    self.overlay_pending_vad_text = false;
    self.raw_finalize_requested = false;
    self.deferred_vad_start = false;
    self.finalizing_deadline = None;
    self.finalizing_turn_id = None;
    self.finalizing_draft.clear();
    self.latest_draft.clear();
    self.latest_final = None;
    self.raw_handled_turn_id = None;
    self.current_turn_id = None;
    self.turn_accept_floor = self.latest_seen_turn_id.saturating_add(1);
    self.clear_held_top_overlay();
    self.reset_activity_history();
    self.busy_border_phase = 0.0;
    self.listen_toggle_notice = None;
    self.dispatch_hotkey_input(HotkeyInput::TurnReset);
    self.render_device_menu();
    self.session_recovery_state = SessionRecoveryState::Healthy;
    self.session_fault_window.clear();
    self.last_session_error_was_stream_fault = false;
    self.pending_recovery_restart = false;
    if let Some(session) = &self.session {
      session.release_manual_hold();
      session.cancel_current_turn();
      session.set_auto_vad_enabled(false);
      session.set_capture_enabled(false);
    }
    self.show_accessibility_overlay_notice();
  }

  fn ensure_paste_accessibility_or_disable_listening(&mut self) -> bool {
    if platform::ensure_accessibility_for_auto_paste() {
      return true;
    }
    self.disable_listening_due_to_accessibility();
    false
  }

  fn try_paste(&mut self, turn_id: u64, mode: TranscriptMode, text: &str) -> bool {
    let paste_text =
      build_paste_text(text, self.append_trailing_space_on_paste, &self.removed_words);

    if !matches!(self.paste_method, PasteMethod::ClipboardPaste) {
      let payload_json =
        serde_json::to_string(&paste_text).unwrap_or_else(|_| "\"<serialize_error>\"".to_string());
      eprintln!(
        "AZAD_DIRECT_PAYLOAD turn_id={} transcript_mode={} method={} chars={} payload={}",
        turn_id,
        transcript_mode_label(mode),
        paste_method_label(self.paste_method),
        paste_text.chars().count(),
        payload_json
      );
    }

    let paste_started = Instant::now();
    let insert_started = Instant::now();
    let paste_result =
      platform::insert_text(&paste_text, self.paste_method, self.cfg.paste_delay_ms);
    let insert_duration_ms =
      u64::try_from(insert_started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let auto_submit_started = Instant::now();
    let (auto_submit_sent, auto_submit_ok) = if matches!(paste_result, PasteResult::Pasted) {
      let sent = !matches!(self.auto_submit_mode, AutoSubmitMode::Off);
      let ok = platform::send_auto_submit(self.auto_submit_mode);
      (sent, ok)
    } else {
      (false, true)
    };
    let auto_submit_duration_ms = if matches!(paste_result, PasteResult::Pasted) {
      u64::try_from(auto_submit_started.elapsed().as_millis()).unwrap_or(u64::MAX)
    } else {
      0
    };
    if matches!(paste_result, PasteResult::Pasted) && !auto_submit_ok {
      eprintln!(
        "Azad: failed to send auto-submit key event (mode={})",
        auto_submit_mode_label(self.auto_submit_mode)
      );
    }
    if matches!(paste_result, PasteResult::AccessibilityRequired) {
      self.disable_listening_due_to_accessibility();
    }
    let paste_duration_ms = u64::try_from(paste_started.elapsed().as_millis()).unwrap_or(u64::MAX);

    eprintln!(
      "AZAD_INSERT_PERF turn_id={} transcript_mode={} method={} chars={} result={} insert_ms={} auto_submit_mode={} auto_submit_sent={} auto_submit_ms={} auto_submit_ok={} total_ms={}",
      turn_id,
      transcript_mode_label(mode),
      paste_method_label(self.paste_method),
      paste_text.chars().count(),
      paste_result_label(paste_result),
      insert_duration_ms,
      auto_submit_mode_label(self.auto_submit_mode),
      auto_submit_sent,
      auto_submit_duration_ms,
      auto_submit_ok,
      paste_duration_ms
    );

    let transcription_duration_ms = self
      .turn_started_at
      .remove(&turn_id)
      .map(|started_at| u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX))
      .unwrap_or(0);
    let (fallback, fallback_reason) = self
      .turn_finalize_outcomes
      .remove(&turn_id)
      .map(|(outcome, reason)| (outcome == "full_pass_bailout", reason))
      .unwrap_or((false, "unavailable".to_string()));

    if self.debug_stats_enabled {
      let _ = metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::PasteCompleted {
        turn_id,
        mode,
        paste_duration_ms,
        result: paste_result_label(paste_result).to_string(),
      }));

      if transcription_duration_ms > 0 {
        let _ =
          metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::TurnCompleted {
            turn_id,
            mode,
            transcription_duration_ms,
          }));
      }
      let _ = metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::TurnSnapshot {
        turn_id,
        mode,
        transcription_duration_ms,
        fallback,
        fallback_reason,
        text_preview: preview_text_for_metrics(text, 45),
      }));
    }

    matches!(paste_result, PasteResult::Pasted)
  }

  fn hide_overlay(&mut self) {
    self.overlay_pending_vad_text = false;
    self.clear_held_top_overlay();
    self.listen_toggle_notice = None;
    if self.overlay_visible {
      platform::hide_overlay();
      self.overlay_visible = false;
    }
  }

  fn handle_arrow_navigate(&mut self, direction: i32) {
    // Up arrow at any moment during opt+space hold pivots into history mode.
    // `enter_history_mode` cancels any in-flight transcription cleanly via
    // `session.cancel_current_turn`, so we can drop the previous gates that
    // required no draft text and no speech yet.
    if !self.history_browsing && direction == -1 && self.overlay_visible {
      self.enter_history_mode();
      return;
    }
    if !self.history_browsing {
      return;
    }
    let count = self.transcript_index.as_ref().map(|i| i.entry_count()).unwrap_or(0);
    if count == 0 {
      return;
    }
    match direction {
      -1 => {
        if self.history_browse_index + 1 < count {
          self.history_browse_index += 1;
        }
      }
      1 => {
        if self.history_browse_index > 0 {
          self.history_browse_index -= 1;
        }
      }
      _ => {}
    }
    self.render_history_overlay();
  }

  fn render_history_overlay(&self) {
    let Some(index) = &self.transcript_index else {
      platform::set_overlay_history_content(&[], 0);
      return;
    };
    let count = index.entry_count();
    let entries: Vec<platform::HistoryEntryView<'_>> = (0..count)
      .filter_map(|i| index.entry_text(i).map(|text| platform::HistoryEntryView { text }))
      .collect();
    let selected = self.history_browse_index.min(count.saturating_sub(1));
    platform::set_overlay_history_content(&entries, selected);
  }

  fn paste_from_history(&mut self) {
    let text = self
      .transcript_index
      .as_ref()
      .and_then(|index| index.entry_text(self.history_browse_index))
      .map(|s| s.to_string());
    if let Some(text) = text {
      let paste_text =
        build_paste_text(&text, self.append_trailing_space_on_paste, &self.removed_words);
      let _ = platform::insert_text(&paste_text, self.paste_method, self.cfg.paste_delay_ms);
      let _ = platform::send_auto_submit(self.auto_submit_mode);
    }
    // Exit even on an empty-state release so the overlay closes — otherwise
    // the "No transcripts" overlay would linger after opt+space release.
    self.exit_history_mode();
  }

  fn enter_history_mode(&mut self) {
    // Cancel any in-flight turn and let go of the manual-hold flag. We do NOT
    // call `set_capture_enabled(false)`: that fires a `SessionEnded` event,
    // whose handler calls `self.hide_overlay()` — which would tear down the
    // overlay we're about to rebuild as the history list. `release_manual_hold`
    // is enough to drop manual-hold capture; always-listening users keep
    // capture rolling and any incoming speech events are dropped by the
    // `history_browsing` guard at the top of `handle_speech_event`.
    self.manual_hold_active = false;
    self.hold_saw_speech = false;
    if let Some(session) = &self.session {
      session.release_manual_hold();
      session.cancel_current_turn();
    }
    self.latest_draft.clear();
    self.finalizing_draft.clear();
    self.finalizing_turn_id = None;
    self.finalizing_deadline = None;
    self.history_browsing = true;
    self.history_browse_index = 0;
    platform::set_arrow_left_hotkey_enabled(true);
    // Make sure the overlay window itself is shown (e.g. during VAD-only
    // sessions where opt+space wasn't held to bring it up).
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    self.render_history_overlay();
  }

  fn exit_history_mode(&mut self) {
    self.history_browsing = false;
    self.history_browse_index = 0;
    self.overlay_visible = false;
    platform::set_arrow_left_hotkey_enabled(false);
    platform::hide_overlay();
  }

  fn reset_turn_state(&mut self) {
    self.dispatch_hotkey_input(HotkeyInput::TurnReset);
    self.reset_turn_state_preserving_hotkey_state();
  }

  fn reset_turn_state_preserving_hotkey_state(&mut self) {
    self.cancelled = false;
    self.last_pasted_turn_id = None;
    self.hold_saw_speech = false;
    self.latest_draft.clear();
    self.latest_final = None;
    self.finalizing_deadline = None;
    self.finalizing_turn_id = None;
    self.raw_handled_turn_id = None;
    self.raw_finalize_requested = false;
    self.deferred_vad_start = false;
    self.accessibility_notice_deadline = None;
    self.listen_toggle_notice = None;
    self.overlay_pending_vad_text = false;
    self.clear_held_top_overlay();
    self.current_turn_id = None;
    self.turn_accept_floor = self.latest_seen_turn_id.saturating_add(1);
    self.turn_started_at.clear();
    self.turn_finalize_outcomes.clear();
    self.reset_activity_history();
    self.busy_border_phase = 0.0;
  }

  fn try_finalize_with_raw_text(&mut self, turn_id: Option<u64>) -> bool {
    let Some(turn_id) = turn_id else {
      return false;
    };

    let raw_targets_finalizing_lane = self.finalizing_turn_id == Some(turn_id);
    let raw_text = if raw_targets_finalizing_lane {
      self.finalizing_draft.trim().to_string()
    } else {
      self.latest_draft.trim().to_string()
    };
    if raw_text.is_empty() {
      return false;
    }

    let ui_plan = raw_finalize_ui_plan(
      self.always_listening_enabled,
      self.manual_hold_active,
      self.raw_finalize_requested,
    );
    if raw_targets_finalizing_lane {
      self.finalizing_turn_id = None;
      self.finalizing_deadline = None;
      self.finalizing_draft.clear();
    }
    self.raw_handled_turn_id = Some(turn_id);
    self.raw_finalize_requested = false;
    self.dispatch_hotkey_input(HotkeyInput::SpeechFinalized);
    self.latest_final = Some(raw_text.clone());

    if !self.cancelled && self.last_pasted_turn_id != Some(turn_id) {
      // Keep the overlay up through the paste window so the dismissal coincides with the
      // text landing in the target. Hiding first, then blocking in try_paste, opens a
      // visible gap between "overlay gone" and "paste appears".
      let should_hide_overlay =
        ui_plan.hide_overlay && (raw_targets_finalizing_lane || self.finalizing_turn_id.is_none());
      if self.try_paste(turn_id, TranscriptMode::Raw, &raw_text) {
        self.last_pasted_turn_id = Some(turn_id);
      } else {
        eprintln!("Azad: failed to auto-paste raw transcript (clipboard still contains text)");
      }
      if should_hide_overlay {
        self.hide_overlay();
      }
    }

    if !raw_targets_finalizing_lane && self.finalizing_turn_id.is_some() {
      self.clear_live_lane_state();
      self.render_finalizing_overlay_state();
    }

    self.maybe_start_deferred_vad_turn();
    if ui_plan.disable_capture {
      if let Some(session) = &self.session {
        session.set_capture_enabled(false);
      }
    }

    true
  }

  fn maybe_start_deferred_vad_turn(&mut self) {
    if !self.deferred_vad_start {
      return;
    }
    self.deferred_vad_start = false;
    self.reset_turn_state();
    self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
  }

  fn handle_debug_stats_event(&mut self, event: DebugStatsEvent) {
    if !self.debug_stats_enabled {
      return;
    }

    match event {
      DebugStatsEvent::PartialFinalizeOutcome { turn_id, outcome, reason } => {
        self.turn_finalize_outcomes.insert(turn_id, (outcome.clone(), reason.clone()));
        let _ = metrics_log::append_record(&MetricsLogRecord::new(
          MetricsLogEvent::PartialFinalizeOutcome { turn_id, outcome, reason },
        ));
      }
      DebugStatsEvent::PartialAuditResult {
        turn_id,
        emitted_kind,
        exact,
        partial_count,
        emitted_tokens,
        full_tokens,
        edit_distance,
        wer_like,
        lcp_tokens,
        lcp_pct,
      } => {
        let _ =
          metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::PartialAuditResult {
            turn_id,
            emitted_kind,
            exact,
            partial_count,
            emitted_tokens,
            full_tokens,
            edit_distance,
            wer_like,
            lcp_tokens,
            lcp_pct,
          }));
      }
      DebugStatsEvent::PartialAuditError { turn_id, emitted_kind, partial_count, message } => {
        let _ =
          metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::PartialAuditError {
            turn_id,
            emitted_kind,
            partial_count,
            message,
          }));
      }
    }
  }

  fn accept_turn(&self, turn_id: u64) -> bool {
    turn_id >= self.turn_accept_floor
  }

  fn observe_turn(&mut self, turn_id: u64) {
    self.latest_seen_turn_id = self.latest_seen_turn_id.max(turn_id);
    self.current_turn_id = Some(next_current_turn_id(self.current_turn_id, turn_id));
    self.turn_started_at.entry(turn_id).or_insert_with(Instant::now);
  }

  fn update_activity_from_meter(&mut self, peak_db: f32, vad_speech: bool, vad_prob: f32) {
    let normalized_peak = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
    let vad_component = if vad_speech { vad_prob.clamp(0.0, 1.0).max(0.15) } else { 0.0 };
    let mut next = normalized_peak.max(vad_component);
    if !vad_speech {
      next *= 0.7;
    }
    self.latest_activity_level = next.clamp(0.0, 1.0);
    self.last_activity_at = Some(Instant::now());
  }

  fn reset_activity_history(&mut self) {
    self.activity_history.clear();
    self.activity_history.resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
    self.latest_activity_level = 0.0;
    self.last_activity_at = None;
  }

  fn advance_activity_timeline(&mut self) {
    if self.activity_history.len() != OVERLAY_ACTIVITY_HISTORY_LEN {
      self.activity_history.resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
    }

    // Keep activity motion tied to active transcription capture. Once capture/speech
    // stops, freeze the visualization in place.
    let should_animate_activity =
      self.manual_hold_active || self.engine_state == EngineState::Speech;
    if !should_animate_activity {
      return;
    }

    let stale = self
      .last_activity_at
      .map(|t| t.elapsed() >= Duration::from_millis(OVERLAY_ACTIVITY_IDLE_TIMEOUT_MS))
      .unwrap_or(true);
    if stale {
      self.latest_activity_level =
        (self.latest_activity_level * OVERLAY_ACTIVITY_DECAY_PER_TICK).max(0.0);
      if self.latest_activity_level < 0.01 {
        self.latest_activity_level = 0.0;
      }
    }

    if self.activity_history.is_empty() {
      return;
    }
    self.activity_history.rotate_left(1);
    if let Some(last) = self.activity_history.last_mut() {
      *last = self.latest_activity_level.clamp(0.0, 1.0);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{
    AppController,
    AzadConfig,
    DraftOverlayAction,
    EngineState,
    HotkeyEffect,
    ManualHoldReleaseAction,
    ManualHoldReleasePlan,
    RawFinalizeUiPlan,
    SessionRecoveryState,
    allow_immediate_restart_for_fault_count,
    build_paste_text,
    draft_matches_finalized_text,
    draft_update_overlay_action,
    has_actionable_turn_context_for_snapshot,
    has_started_turn_for_snapshot,
    has_turn_context_for_snapshot,
    is_stream_fault_message,
    listen_toggle_notice,
    manual_hold_release_plan,
    next_current_turn_id,
    raw_finalize_target_turn_id_for_state,
    raw_finalize_ui_plan,
    recovery_state_for_fault_count,
    should_ignore_finalizing_event,
    split_overlay_active_for_turns,
    split_overlay_visible_for_state,
    split_overlay_visible_with_hold_for_state,
    split_overlay_visible_with_live_divergence_for_state,
    split_overlay_visible_with_vad_hint_for_state,
    split_top_completion_for_state,
  };

  #[test]
  fn raw_finalize_hotkey_forces_overlay_hide_even_during_manual_hold() {
    let plan = raw_finalize_ui_plan(false, true, true);
    assert_eq!(plan, RawFinalizeUiPlan { hide_overlay: true, disable_capture: false });
  }

  #[test]
  fn non_hotkey_raw_finalize_keeps_overlay_when_manual_hold_is_active() {
    let plan = raw_finalize_ui_plan(false, true, false);
    assert_eq!(plan, RawFinalizeUiPlan { hide_overlay: false, disable_capture: false });
  }

  #[test]
  fn raw_finalize_without_hold_in_manual_mode_hides_overlay_and_disables_capture() {
    let plan = raw_finalize_ui_plan(false, false, false);
    assert_eq!(plan, RawFinalizeUiPlan { hide_overlay: true, disable_capture: true });
  }

  #[test]
  fn raw_finalize_without_hold_in_always_listening_hides_overlay_but_keeps_capture() {
    let plan = raw_finalize_ui_plan(true, false, false);
    assert_eq!(plan, RawFinalizeUiPlan { hide_overlay: true, disable_capture: false });
  }

  #[test]
  fn manual_hold_release_plan_disables_capture_when_listen_is_off() {
    let plan = manual_hold_release_plan(false, true, true);
    assert_eq!(plan, ManualHoldReleasePlan {
      capture_enabled: false,
      action: ManualHoldReleaseAction::FinalizeTurn,
    });
  }

  #[test]
  fn manual_hold_release_plan_keeps_capture_when_listen_is_on() {
    let plan = manual_hold_release_plan(true, true, false);
    assert_eq!(plan, ManualHoldReleasePlan {
      capture_enabled: true,
      action: ManualHoldReleaseAction::HideOverlay,
    });
  }

  #[test]
  fn manual_hold_release_plan_keeps_live_when_not_finalizing() {
    let plan = manual_hold_release_plan(false, false, true);
    assert_eq!(plan, ManualHoldReleasePlan {
      capture_enabled: false,
      action: ManualHoldReleaseAction::KeepLive,
    });
  }

  #[test]
  fn release_manual_hold_without_session_hides_overlay_for_empty_turn() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.overlay_visible = true;
    controller.manual_hold_active = true;
    controller.apply_hotkey_effect(HotkeyEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: false,
    });
    assert!(!controller.overlay_visible);
    assert!(!controller.manual_hold_active);
  }

  #[test]
  fn release_manual_hold_without_session_hides_overlay_for_started_turn() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.overlay_visible = true;
    controller.manual_hold_active = true;
    controller.apply_hotkey_effect(HotkeyEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: true,
    });
    assert!(!controller.overlay_visible);
    assert!(!controller.manual_hold_active);
  }

  #[test]
  fn replayed_finalizing_for_raw_handled_turn_is_ignored() {
    assert!(should_ignore_finalizing_event(Some(42), 42));
  }

  #[test]
  fn finalizing_for_different_turn_is_not_ignored() {
    assert!(!should_ignore_finalizing_event(Some(42), 43));
    assert!(!should_ignore_finalizing_event(None, 43));
  }

  #[test]
  fn split_overlay_active_only_when_current_turn_is_newer_than_finalizing() {
    assert!(split_overlay_active_for_turns(Some(2), Some(3)));
    assert!(!split_overlay_active_for_turns(Some(3), Some(3)));
    assert!(!split_overlay_active_for_turns(Some(3), Some(2)));
    assert!(!split_overlay_active_for_turns(None, Some(2)));
  }

  #[test]
  fn raw_finalize_targets_bottom_turn_when_split_overlay_is_active() {
    let target = raw_finalize_target_turn_id_for_state(Some(5), Some(7), 7, "live text");
    assert_eq!(target, Some(7));
  }

  #[test]
  fn raw_finalize_targets_finalizing_turn_when_not_split() {
    let target = raw_finalize_target_turn_id_for_state(Some(5), Some(5), 5, "live text");
    assert_eq!(target, Some(5));
  }

  #[test]
  fn raw_finalize_targets_finalizing_turn_when_split_turn_has_no_text() {
    let target = raw_finalize_target_turn_id_for_state(Some(5), Some(7), 7, "");
    assert_eq!(target, Some(5));
  }

  #[test]
  fn current_turn_id_is_monotonic_for_out_of_order_events() {
    let current = Some(next_current_turn_id(None, 5));
    let current = Some(next_current_turn_id(current, 6));
    let current = Some(next_current_turn_id(current, 5));
    assert_eq!(current, Some(6));
  }

  #[test]
  fn stale_finalizing_event_does_not_disable_split_overlay() {
    let current = Some(next_current_turn_id(None, 5));
    let current = Some(next_current_turn_id(current, 6));
    assert!(split_overlay_active_for_turns(Some(5), current));

    // Replayed/late finalizing update for older turn must not steal current turn id.
    let current = Some(next_current_turn_id(current, 5));
    assert!(split_overlay_active_for_turns(Some(5), current));
  }

  #[test]
  fn split_overlay_is_hidden_until_live_text_exists() {
    assert!(!split_overlay_visible_for_state(Some(5), Some(6), ""));
    assert!(!split_overlay_visible_for_state(Some(5), Some(6), "   "));
    assert!(split_overlay_visible_for_state(Some(5), Some(6), "hello"));
  }

  #[test]
  fn split_overlay_hold_shows_only_after_live_text_appears() {
    assert!(!split_overlay_visible_with_hold_for_state(Some(5), Some(5), "", true));
    assert!(split_overlay_visible_with_hold_for_state(Some(5), Some(5), "new words", true));
  }

  #[test]
  fn split_overlay_vad_hint_keeps_second_lane_visible_when_turn_id_lags() {
    assert!(split_overlay_visible_with_vad_hint_for_state(
      Some(5),
      Some(5),
      "new words",
      false,
      true,
    ));
  }

  #[test]
  fn split_overlay_without_hint_stays_hidden_when_turn_id_has_not_advanced() {
    assert!(!split_overlay_visible_with_vad_hint_for_state(
      Some(5),
      Some(5),
      "new words",
      false,
      false,
    ));
  }

  #[test]
  fn split_overlay_vad_hint_requires_finalizing_lane() {
    assert!(!split_overlay_visible_with_vad_hint_for_state(
      None,
      Some(5),
      "new words",
      false,
      true,
    ));
  }

  #[test]
  fn split_overlay_live_divergence_shows_when_turn_id_lags() {
    assert!(split_overlay_visible_with_live_divergence_for_state(
      Some(5),
      "new sentence starts in next thought",
      "previous finalized sentence is done",
    ));
  }

  #[test]
  fn split_overlay_live_divergence_ignores_same_lane_rewrites() {
    assert!(!split_overlay_visible_with_live_divergence_for_state(
      Some(5),
      "this is still the same lane text",
      "this is still the same lane text with punctuation",
    ));
  }

  #[test]
  fn draft_match_treats_near_identical_token_prefix_as_same_turn() {
    assert!(draft_matches_finalized_text(
      "this feels heavy and i think this got cut",
      "this feels heavy and i think this got cut off at the end",
    ));
  }

  #[test]
  fn draft_match_rejects_distinct_turn_content() {
    assert!(!draft_matches_finalized_text(
      "new thought starts here",
      "previous paragraph is now done",
    ));
  }

  #[test]
  fn split_top_completion_ignores_vad_hint_when_live_matches_finalized_turn() {
    assert!(!split_top_completion_for_state(
      Some(10),
      Some(10),
      "this is still the same turn text",
      false,
      true,
      10,
      "this is still the same turn text with punctuation.",
    ));
  }

  #[test]
  fn split_top_completion_allows_vad_hint_when_live_lane_is_distinct() {
    assert!(split_top_completion_for_state(
      Some(10),
      Some(10),
      "brand new sentence in next lane",
      false,
      true,
      10,
      "previous lane text that just finished",
    ));
  }

  #[test]
  fn split_top_completion_allows_live_divergence_without_vad_hint() {
    let completion = split_top_completion_for_state(
      Some(10),
      Some(10),
      "brand new sentence in next lane",
      false,
      false,
      10,
      "previous lane text that just finished",
    ) || split_overlay_visible_with_live_divergence_for_state(
      Some(10),
      "brand new sentence in next lane",
      "previous lane text that just finished",
    );
    assert!(completion);
  }

  #[test]
  fn hotkey_snapshot_idle_without_draft_is_not_started_turn() {
    assert!(!has_started_turn_for_snapshot(false, false, EngineState::Idle, None, "",));
  }

  #[test]
  fn hotkey_snapshot_speech_marks_started_turn() {
    assert!(has_started_turn_for_snapshot(false, false, EngineState::Speech, None, "",));
  }

  #[test]
  fn hotkey_snapshot_finalizing_or_draft_marks_started_turn() {
    assert!(has_started_turn_for_snapshot(false, false, EngineState::Idle, Some(9), "",));
    assert!(has_started_turn_for_snapshot(false, false, EngineState::Idle, None, "draft text",));
  }

  #[test]
  fn hotkey_snapshot_active_hold_ignores_stale_draft_without_speech() {
    assert!(!has_started_turn_for_snapshot(true, false, EngineState::Idle, None, "stale draft",));
  }

  #[test]
  fn hotkey_snapshot_active_hold_uses_hold_speech_signal() {
    assert!(has_started_turn_for_snapshot(true, true, EngineState::Idle, None, "",));
  }

  #[test]
  fn turn_context_snapshot_ignores_manual_hold_without_turn_signals() {
    assert!(!has_turn_context_for_snapshot(EngineState::Idle, None, None, "",));
  }

  #[test]
  fn actionable_turn_context_ignores_stale_state_when_idle_and_hidden() {
    assert!(!has_actionable_turn_context_for_snapshot(
      EngineState::Idle,
      Some(7),
      None,
      "stale transcript text",
      false,
      false,
    ));
  }

  #[test]
  fn actionable_turn_context_keeps_live_finalizing_state() {
    assert!(has_actionable_turn_context_for_snapshot(
      EngineState::Idle,
      Some(7),
      Some(7),
      "still finalizing",
      false,
      false,
    ));
  }

  #[test]
  fn listen_toggle_notice_uses_listen_wording() {
    let (enabled_title, enabled_segments) = listen_toggle_notice(true);
    assert_eq!(enabled_title, "Listen ENABLED");
    assert!(enabled_segments.is_empty());

    let (disabled_title, disabled_segments) = listen_toggle_notice(false);
    assert_eq!(disabled_title, "Listen DISABLED");
    assert!(disabled_segments.is_empty());
  }

  #[test]
  fn build_paste_text_appends_trailing_space_when_enabled() {
    assert_eq!(build_paste_text("hello", true, &[]), "hello ");
    assert_eq!(build_paste_text("hello ", true, &[]), "hello ");
  }

  #[test]
  fn build_paste_text_preserves_input_when_trailing_space_is_disabled() {
    assert_eq!(build_paste_text("hello", false, &[]), "hello");
    assert_eq!(build_paste_text("hello ", false, &[]), "hello ");
  }

  #[test]
  fn build_paste_text_strips_removed_words() {
    let words = vec!["um".to_string(), "ah".to_string()];
    assert_eq!(
      build_paste_text("um I think ah this is right um", false, &words),
      "I think this is right"
    );
    assert_eq!(build_paste_text("Um hello Ah world", false, &words), "hello world");
  }

  #[test]
  fn build_paste_text_strips_removed_word_at_boundaries() {
    let words = vec!["um".to_string()];
    assert_eq!(build_paste_text("um", false, &words), "");
    assert_eq!(build_paste_text("um hello", false, &words), "hello");
    assert_eq!(build_paste_text("hello um", false, &words), "hello");
    assert_eq!(build_paste_text("yummy", false, &words), "yummy");
  }

  #[test]
  fn build_paste_text_strips_removed_words_with_punctuation() {
    let words = vec!["um".to_string(), "ah".to_string()];
    assert_eq!(
      build_paste_text("Um, I think this is right.", false, &words),
      "I think this is right."
    );
    assert_eq!(build_paste_text("Ah. Hello world.", false, &words), "Hello world.");
    assert_eq!(build_paste_text("um, ah, hello", false, &words), "hello");
  }

  #[test]
  fn stream_fault_classifier_matches_core_audio_failure_signals() {
    assert!(is_stream_fault_message(
      "audio input stream ended after error: The requested device is no longer available"
    ));
    assert!(is_stream_fault_message("failed to open microphone capture: device not found"));
    assert!(!is_stream_fault_message("clipboard write failed"));
  }

  #[test]
  fn recovery_state_progresses_from_recovering_to_degraded() {
    assert_eq!(recovery_state_for_fault_count(0), SessionRecoveryState::Healthy);
    assert_eq!(recovery_state_for_fault_count(1), SessionRecoveryState::Recovering);
    assert_eq!(recovery_state_for_fault_count(2), SessionRecoveryState::Recovering);
    assert_eq!(recovery_state_for_fault_count(3), SessionRecoveryState::Degraded);
  }

  #[test]
  fn immediate_restart_is_bounded_for_repeated_faults() {
    assert!(allow_immediate_restart_for_fault_count(1));
    assert!(allow_immediate_restart_for_fault_count(2));
    assert!(!allow_immediate_restart_for_fault_count(3));
  }

  #[test]
  fn menu_toggle_defers_while_transcription_is_active() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.always_listening_enabled = true;
    controller.engine_state = EngineState::Speech;
    controller.current_turn_id = Some(7);
    controller.latest_draft = "active text".to_string();
    controller.manual_hold_active = true;
    controller.hold_saw_speech = true;

    controller.apply_hotkey_effect(HotkeyEffect::MenuToggleAlwaysListening);
    assert_eq!(controller.pending_always_listening_enabled, Some(false));
    assert!(controller.always_listening_enabled);
  }

  #[test]
  fn menu_toggle_applies_immediately_when_idle() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.always_listening_enabled = false;
    controller.apply_hotkey_effect(HotkeyEffect::MenuToggleAlwaysListening);
    assert!(controller.always_listening_enabled);
    assert_eq!(controller.pending_always_listening_enabled, None);
  }

  #[test]
  fn deferred_menu_toggle_applies_on_turn_boundary() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.always_listening_enabled = true;
    controller.pending_always_listening_enabled = Some(false);
    controller.engine_state = EngineState::Idle;
    controller.current_turn_id = None;
    controller.finalizing_turn_id = None;
    controller.latest_draft.clear();
    controller.manual_hold_active = false;
    controller.hold_saw_speech = false;
    controller.overlay_visible = false;

    controller.on_tick();
    assert!(!controller.always_listening_enabled);
    assert_eq!(controller.pending_always_listening_enabled, None);
  }

  #[test]
  fn deferred_menu_toggle_can_be_reversed_before_turn_boundary() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.always_listening_enabled = true;
    controller.engine_state = EngineState::Speech;
    controller.current_turn_id = Some(9);
    controller.finalizing_turn_id = None;
    controller.latest_draft = "still speaking".to_string();
    controller.manual_hold_active = true;
    controller.hold_saw_speech = true;

    controller.apply_hotkey_effect(HotkeyEffect::MenuToggleAlwaysListening);
    assert_eq!(controller.pending_always_listening_enabled, Some(false));

    controller.apply_hotkey_effect(HotkeyEffect::MenuToggleAlwaysListening);
    assert_eq!(controller.pending_always_listening_enabled, Some(true));
    assert!(controller.always_listening_enabled);
  }

  #[test]
  fn active_transcription_detection_ignores_pre_speech_hold() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.engine_state = EngineState::Idle;
    controller.manual_hold_active = true;
    controller.hold_saw_speech = false;
    controller.overlay_visible = true;
    controller.latest_draft.clear();
    controller.current_turn_id = None;
    controller.finalizing_turn_id = None;

    assert!(!controller.has_active_transcription_turn());
  }

  #[test]
  fn started_turn_snapshot_ignores_stale_engine_speech_during_pre_speech_hold() {
    assert!(!has_started_turn_for_snapshot(true, false, EngineState::Speech, None, "",));
  }

  #[test]
  fn started_turn_snapshot_marks_hold_as_started_after_speech_is_seen() {
    assert!(has_started_turn_for_snapshot(true, true, EngineState::Idle, None, "",));
  }

  #[test]
  fn release_without_started_turn_clears_stale_turn_state() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.manual_hold_active = true;
    controller.overlay_visible = true;
    controller.latest_draft = "stale draft".to_string();
    controller.current_turn_id = Some(5);
    controller.finalizing_turn_id = Some(5);
    controller.turn_accept_floor = 2;

    controller.apply_hotkey_effect(HotkeyEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: false,
    });

    assert!(!controller.overlay_visible);
    assert!(controller.latest_draft.is_empty());
    assert_eq!(controller.current_turn_id, None);
    assert_eq!(controller.finalizing_turn_id, None);
    assert!(controller.turn_accept_floor >= 1);
  }

  #[test]
  fn draft_overlay_shows_when_pending_and_hidden_and_unsuppressed() {
    let action = draft_update_overlay_action(
      /* pending */ true, /* overlay_visible */ false,
      /* cancel_suppression_active */ false,
    );
    assert_eq!(action, DraftOverlayAction::Show);
  }

  #[test]
  fn draft_overlay_keeps_pending_during_cancel_suppression_window() {
    // Regression: the buggy implementation cleared `overlay_pending_vad_text` unconditionally
    // after the show-check, so a turn that began within CANCEL_VAD_SHOW_SUPPRESSION_MS of the
    // user pressing Escape saw its first DraftUpdate get suppressed, lost the pending flag,
    // and then transcribed the rest of the turn with no overlay. The fix is to leave the
    // pending flag intact while suppression is active so a later DraftUpdate past the window
    // can still bring the overlay up.
    let action = draft_update_overlay_action(
      /* pending */ true, /* overlay_visible */ false,
      /* cancel_suppression_active */ true,
    );
    assert_eq!(action, DraftOverlayAction::KeepPendingForLater);
  }

  #[test]
  fn draft_overlay_clears_when_already_visible_and_unsuppressed() {
    // Overlay is up; nothing to show. The pending flag should not linger.
    let action = draft_update_overlay_action(
      /* pending */ true, /* overlay_visible */ true,
      /* cancel_suppression_active */ false,
    );
    assert_eq!(action, DraftOverlayAction::Clear);
  }

  #[test]
  fn draft_overlay_keeps_pending_during_suppression_even_if_visible() {
    // Degenerate state (shouldn't typically occur — cancel hides the overlay as it starts the
    // suppression window). Locking in the rule: while suppression is active, nothing touches
    // the pending flag regardless of visibility.
    let action = draft_update_overlay_action(
      /* pending */ true, /* overlay_visible */ true,
      /* cancel_suppression_active */ true,
    );
    assert_eq!(action, DraftOverlayAction::KeepPendingForLater);
  }

  #[test]
  fn draft_overlay_clears_when_not_pending_and_unsuppressed() {
    for visible in [false, true] {
      let action = draft_update_overlay_action(
        /* pending */ false, visible, /* cancel_suppression_active */ false,
      );
      assert_eq!(action, DraftOverlayAction::Clear, "visible={visible}");
    }
  }

  #[test]
  fn draft_overlay_holds_state_during_suppression_when_not_pending() {
    for visible in [false, true] {
      let action = draft_update_overlay_action(
        /* pending */ false, visible, /* cancel_suppression_active */ true,
      );
      assert_eq!(action, DraftOverlayAction::KeepPendingForLater, "visible={visible}");
    }
  }
}
