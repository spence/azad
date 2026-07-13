use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use asr::devices::DeviceStateSnapshot;
use asr::pipeline::{DebugStatsEvent, EngineState};
use azad_text::{DisplayTextOptions, PasteTextOptions, build_display_text, build_paste_text};

use crate::apple_lm::{
  self, AvailabilityReport, AvailabilityState, AzadIntent, RemovedWordAction, TextSettingId,
  TextSettingsSnapshot,
};
use crate::config::AzadConfig;
use crate::connectors;
use crate::device::{DeviceController, DeviceEvent};
use crate::gateway::{self, ConvStatus, GatewayCommand, GatewayEvent};
use crate::hotkey_sm::{HotkeyEffect, HotkeyInput, HotkeyState, RuntimeSnapshot};
use crate::input_log::{self, InputLogEntry, InputLogEvent, StateSnapshot};
use crate::metrics_log::{self, MetricsLogEvent, MetricsLogRecord, TranscriptMode};
use crate::model_download::DownloadHandle;
use crate::models::{self, PackStatus};
use crate::platform;
use crate::platform::{DeviceMenuModel, DeviceMenuRow, PasteResult, SettingsTab};
use crate::preferred_store;
use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod, StartupListenMode};
use crate::speech::{SpeechEvent, SpeechSession, spawn_speech_session};
use crate::spotify_client;
use crate::spotify_cmd::{self, SpotifyIntent};
use crate::transcript_history::TranscriptIndex;

mod history;
mod policy;
mod settings_ui;

use policy::{
  DraftOverlayAction, ListenToggleNotice, ManualHoldReleaseAction, SessionRecoveryState,
  allow_immediate_restart_for_fault_count, draft_update_overlay_action,
  final_text_has_user_visible_context, has_actionable_turn_context_for_snapshot,
  has_started_turn_for_snapshot, is_stream_fault_message, listen_toggle_notice,
  manual_hold_release_plan, next_current_turn_id, raw_finalize_target_turn_id_for_state,
  raw_finalize_ui_plan, recovery_state_for_fault_count, should_ignore_finalizing_event,
  should_latch_raw_on_hold_release, split_overlay_active_for_turns,
  split_overlay_visible_with_live_divergence_for_state,
  split_overlay_visible_with_vad_hint_for_state, split_top_completion_for_state,
  turn_started_should_arm_pending,
};
const DEVICE_SWITCH_RESTART_DEBOUNCE_MS: u64 = 250;
const OVERLAY_ACTIVITY_HISTORY_LEN: usize = 96;
const OVERLAY_ACTIVITY_IDLE_TIMEOUT_MS: u64 = 220;
const OVERLAY_ACTIVITY_DECAY_PER_TICK: f32 = 0.88;
const OVERLAY_BUSY_PHASE_STEP: f32 = 0.24;
const LISTEN_TOGGLE_NOTICE_DURATION_MS: u64 = 600;
const LISTEN_RECOVERING_NOTICE_DURATION_MS: u64 = 1200;
const CANCEL_VAD_SHOW_SUPPRESSION_MS: u64 = 500;
const HISTORY_MANUAL_HOLD_RELEASE_GRACE_MS: u64 = 500;
const SESSION_FAULT_WINDOW_MS: u64 = 30_000;
const SESSION_IMMEDIATE_RETRY_LIMIT: usize = 2;
const SESSION_DEGRADED_THRESHOLD: usize = 3;
/// Fast fail: a `runs.create` that the daemon never acknowledges (no response/event of any
/// kind) within this window means the gateway is wedged, offline, or speaking a protocol it
/// can't parse — fail immediately rather than leaving the overlay on "Thinking…".
const GATEWAY_ACK_TIMEOUT: Duration = Duration::from_secs(5);
/// Once acknowledged, the run may legitimately think for a while; only give up if it then
/// goes fully silent (no delta/activity/completion) for this much longer window. Any inbound
/// event refreshes the deadline, so a live-but-slow answer is never killed.
const GATEWAY_STREAM_TIMEOUT: Duration = Duration::from_secs(25);
const GATEWAY_NO_ACK_MESSAGE: &str =
  "Gateway didn't respond. Is local-agent-gatewayd running and current? — press Esc.";
const GATEWAY_STALL_MESSAGE: &str = "Claude stopped responding — press Esc to dismiss.";
/// One-shot command cards (Hey Azad / Hey Spotify) auto-dismiss after success so
/// the user doesn't need Esc. Claude gateway conversations stay sticky.
const COMMAND_OVERLAY_SUCCESS_HOLD: Duration = Duration::from_secs(1);
const COMMAND_OVERLAY_ERROR_HOLD: Duration = Duration::from_secs(3);
const ACTIVATION_LEVEL_MIN_RMS_DB: f32 = -60.0;
const ACTIVATION_LEVEL_MAX_RMS_DB: f32 = -20.0;

#[derive(Debug, Clone)]
pub enum AppEvent {
  ShutdownRequested,
  HotkeyPressed,
  HotkeyReleased {
    raw_requested: bool,
  },
  FinalizeHotkeyPressed {
    raw_requested: bool,
  },
  MenuToggleAlwaysListening,
  MenuToggleDevices,
  MenuSelectDevice(String),
  MenuOpenSettings,
  MenuOpened,
  MenuClosed,
  SettingsToggleRunOnStartup(bool),
  SettingsSelectStartupListenMode(StartupListenMode),
  SettingsToggleDebugStats(bool),
  SettingsSetActivationLevel(i64),
  SettingsSelectPasteMethod(PasteMethod),
  SettingsSelectAutoSubmit(AutoSubmitMode),
  SettingsSelectOverlayPosition(OverlayPosition),
  SettingsToggleHistory(bool),
  SettingsToggleAppendTrailingSpace(bool),
  SettingsToggleDeduplicateWords(bool),
  SettingsToggleConvertNumberWords(bool),
  SettingsToggleConvertSpokenEmoji(bool),
  SettingsToggleLowercaseExceptUppercaseWords(bool),
  SettingsToggleRemoveHesitations(bool),
  SettingsSetListenModifier {
    bit: u8,
    enabled: bool,
  },
  SettingsToggleConnector {
    index: usize,
    enabled: bool,
  },
  SettingsOpenSystemSettings,
  SettingsRecheckAppleLm,
  SettingsAddRemovedWord(String),
  SettingsRemoveRemovedWord(String),
  SettingsRefresh,
  SettingsDownloadModel(String),
  SettingsSetDownloadPaused(bool),
  RequestPermission(String),
  OnboardingGetStarted,
  OnboardingDownloadModel,
  OnboardingSetListenModifier {
    bit: u8,
    enabled: bool,
  },
  ModelDownloadProgress {
    pack_id: String,
    bytes_done: u64,
    bytes_total: u64,
  },
  ModelDownloadCompleted(String),
  ModelDownloadError {
    pack_id: String,
    message: String,
  },
  OverlayCancel,
  ArrowNavigate(i32),
  /// Right-arrow while history-browse mode is active. Expands the selected
  /// entry inline in the list to show its full text.
  HistoryExpand,
  /// Left-arrow while history-browse mode is active. In expanded mode it
  /// collapses back to the list; in list mode it dismisses the overlay.
  /// Distinct from `OverlayCancel` (Esc) so Esc can always exit fully.
  HistoryCollapse,
  /// User typed into the history search field. Filters the list to entries
  /// containing the term (case-insensitive substring) and highlights the
  /// match. Empty string clears the filter.
  HistorySearchChanged(String),
  /// HID-tap captured a printable keystroke while history mode is active —
  /// append it to the search query.
  HistorySearchAppend(String),
  /// HID-tap captured a backspace — drop the last character of the query.
  HistorySearchBackspace,
  /// HID-tap captured Option+Backspace — drop the trailing word.
  HistorySearchDeleteWord,
  /// HID-tap captured Cmd+Backspace — clear the entire query.
  HistorySearchClear,
  Speech(SpeechEvent),
  Device(DeviceEvent),
  Gateway(crate::gateway::GatewayEvent),
}

static EVENT_TX: OnceLock<Sender<AppEvent>> = OnceLock::new();
static EVENT_RX: OnceLock<Mutex<Receiver<AppEvent>>> = OnceLock::new();
static CONTROLLER: OnceLock<Mutex<AppController>> = OnceLock::new();
static HOTKEY_CLOCK_START: OnceLock<Instant> = OnceLock::new();
static TERMINATION_SIGNAL_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Heartbeat log cadence. Only emits while `AzadDebugStatsEnabled` is set, so it's quiet for
/// normal users. The point is to have a timestamped breadcrumb trail of steady-state flags
/// right up to the moment the app goes silent — so when we get another "it stopped responding
/// and a restart fixed it" report, the tail of the log tells us the last observed values of
/// `capture_enabled`, `always_listening`, `manual_hold_active`, etc.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const FAST_TICK_INTERVAL: Duration = Duration::from_millis(50);
const INTERACTIVE_TICK_INTERVAL: Duration = Duration::from_millis(250);
const IDLE_TICK_INTERVAL: Duration = Duration::from_secs(1);

pub fn run() {
  let (tx, rx) = mpsc::channel::<AppEvent>();
  let _ = EVENT_TX.set(tx);
  let _ = EVENT_RX.set(Mutex::new(rx));
  install_termination_signal_handlers();

  let mut controller = AppController::new(AzadConfig::default());
  controller.bootstrap();
  platform::set_status_item_visible(
    !controller.pending_onboarding && !controller.onboarding_active,
  );
  let _ = CONTROLLER.set(Mutex::new(controller));

  spawn_heartbeat_thread();

  platform::run_app();
}

extern "C" fn handle_termination_signal(_: libc::c_int) {
  TERMINATION_SIGNAL_RECEIVED.store(true, Ordering::SeqCst);
}

fn install_termination_signal_handlers() {
  unsafe {
    for signal in [libc::SIGTERM, libc::SIGINT] {
      let handler = handle_termination_signal as *const () as libc::sighandler_t;
      if libc::signal(signal, handler) == libc::SIG_ERR {
        eprintln!("Azad: failed to install termination signal handler for signal {signal}");
      }
    }
  }
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
            let capture_enabled =
              ctrl.session.as_ref().map(|s| s.capture_enabled()).unwrap_or(false);
            eprintln!(
              "AZAD_HEARTBEAT session_present={} always_listening={} manual_hold_active={} \
             engine_state={:?} overlay_visible={} current_turn={:?} latest_seen_turn={} \
             last_pasted_turn={:?} cancelled={} hold_saw_speech={} pending_recovery={} \
             history_browsing={} capture_enabled={} onboarding_complete={}",
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
              ctrl.history_browsing,
              capture_enabled,
              ctrl.onboarding_complete,
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
    if tx.send(event).is_ok() {
      platform::wake_event_loop();
    }
  }
}

pub fn request_shutdown() {
  send_event(AppEvent::ShutdownRequested);
}

pub fn set_download_paused_immediate(paused: bool) -> bool {
  let Some(controller_mutex) = CONTROLLER.get() else {
    return false;
  };
  let Ok(mut controller) = controller_mutex.lock() else {
    return false;
  };
  controller.handle_settings_set_download_paused(paused);
  true
}

pub fn drain_events() -> Duration {
  let Some(rx) = EVENT_RX.get() else {
    return IDLE_TICK_INTERVAL;
  };
  let Some(controller_mutex) = CONTROLLER.get() else {
    return IDLE_TICK_INTERVAL;
  };

  let mut pending = Vec::new();
  if TERMINATION_SIGNAL_RECEIVED.swap(false, Ordering::SeqCst) {
    pending.push(AppEvent::ShutdownRequested);
  }
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
  controller.next_tick_interval(platform::settings_window_is_open())
}

struct AppController {
  cfg: AzadConfig,
  session: Option<SpeechSession>,
  session_device_id: Option<String>,
  next_session_id: u64,
  shutdown_started: bool,

  device_controller: Option<DeviceController>,
  device_snapshot: Option<DeviceStateSnapshot>,
  device_menu_expanded: bool,
  always_listening_enabled: bool,
  pending_always_listening_enabled: Option<bool>,

  manual_hold_active: bool,
  manual_hold_history_grace_until: Option<Instant>,
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
  pending_hold_release_raw_requested: bool,
  run_on_startup_enabled: bool,
  startup_listen_mode: StartupListenMode,
  activation_level: i64,
  history_enabled: bool,
  paste_method: PasteMethod,
  auto_submit_mode: AutoSubmitMode,
  append_trailing_space_on_paste: bool,
  deduplicate_words_on_paste: bool,
  convert_number_words_on_paste: bool,
  convert_spoken_emoji_on_paste: bool,
  lowercase_except_uppercase_words_on_paste: bool,
  remove_hesitations_on_paste: bool,
  overlay_position: OverlayPosition,
  debug_stats_enabled: bool,
  turn_started_at: HashMap<u64, Instant>,
  session_recovery_state: SessionRecoveryState,
  session_fault_window: Vec<Instant>,
  last_session_error_was_stream_fault: bool,
  pending_recovery_restart: bool,
  cancel_vad_show_suppressed_until: Option<Instant>,
  active_pack_id: String,
  models_ready: bool,
  // Gate on first-run onboarding: the app must not spawn a capture session
  // (and grab the mic) until the user has finished setup. Seeded true on
  // bootstrap for users who predate the welcome flow (see `bootstrap`).
  onboarding_complete: bool,
  pending_onboarding: bool,
  onboarding_active: bool,
  last_onboarding_view_model: Option<platform::OnboardingViewModel>,
  pending_first_launch_settings: bool,
  download_handle: Option<DownloadHandle>,
  download_progress: (u64, u64),
  download_progress_dirty: bool,
  transcript_index: Option<TranscriptIndex>,
  history_browsing: bool,
  history_browse_index: usize,
  // Top-of-window entry index for the visible 5-row slice. Stateful so
  // scrolling DOWN out of a top-pinned position lets the selection drop a
  // couple of slots inside the visible list before the window itself starts
  // sliding — rather than pinning the selection at the visual top.
  history_visible_start: usize,
  // True when the user has pressed Right while history-browsing to view the
  // selected entry's full text. Reset on enter/exit and on Up/Down navigation.
  history_expanded: bool,
  // Live filter for the history list, driven by the search field at the
  // bottom of the overlay. Empty string == no filter. Cleared on enter and
  // exit. The underlying transcript_index never changes — filtering happens
  // at render time and on paste.
  history_search_query: String,
  removed_words: Vec<String>,
  // Built-in connectors (only `enabled` is mutable/persisted) and the connector
  // latched for the current turn, if any. Detection runs on the streaming draft
  // (see `SpeechEvent::DraftUpdated`); the latch resets per turn.
  connectors: Vec<connectors::Connector>,
  active_connector: Option<ActiveConnector>,
  // Sticky gateway conversation. Unlike `active_connector` (which resets every turn),
  // this survives turn boundaries: once "hey claude" opens a thread, every later
  // utterance is a follow-up in the same thread until Escape/close clears it.
  gateway_conn: GatewayConnState,
  gateway_conv: Option<GatewayConversation>,
  // Set from `server.ready` when the daemon announces a protocol this build wasn't written
  // against (e.g. a stale daemon binary). Holds the user-facing error; submits fail closed.
  gateway_protocol_mismatch: Option<String>,
  /// One-shot Hey Azad turn (overlay + apply). Separate from `gateway_conv`.
  azad_turn: Option<AzadTurn>,
  /// One-shot Hey Spotify turn (overlay + control). Separate from azad/gateway.
  spotify_turn: Option<SpotifyTurn>,
  /// Cached Apple Intelligence / helper availability for the Connectors pane.
  apple_lm_availability: AvailabilityReport,
  /// Last time we probed availability (throttle background rechecks).
  apple_lm_last_probe: Option<Instant>,
  /// Clean query waiting for Apple Intelligence to become available.
  azad_pending_query: Option<String>,
  /// Whether Spotify.app is installed (refreshed for settings gate).
  spotify_app_installed: bool,
}

/// One-shot Hey Azad conversation card (not sticky multi-turn).
#[derive(Debug, Clone)]
struct AzadTurn {
  tag_label: &'static str,
  tag_icon: &'static str,
  query: String,
  status: ConvStatus,
  reply: String,
  error_msg: String,
  /// When set, `on_tick` clears this card and may hide the overlay.
  dismiss_at: Option<Instant>,
}

/// One-shot Hey Spotify conversation card.
#[derive(Debug, Clone)]
struct SpotifyTurn {
  tag_label: &'static str,
  tag_icon: &'static str,
  query: String,
  status: ConvStatus,
  reply: String,
  error_msg: String,
  /// When set, `on_tick` clears this card and may hide the overlay.
  dismiss_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownSnapshotOutcome {
  Saved { turn_id: u64, char_count: usize },
  HistoryDisabled,
  NoTranscriptIndex,
  NoActiveDraft,
  AlreadyHandled,
}

/// The connector latched for the current turn. `clean_query` is the transcription
/// with the trigger phrase stripped — held for the deferred routing follow-up; the
/// paste path does not consume it yet. `matched_trigger` is the exact phrase that
/// latched (primary or ASR alias) so strip removes the right token count.
#[derive(Debug, Clone)]
struct ActiveConnector {
  id: &'static str,
  tag_label: &'static str,
  tag_icon: &'static str,
  matched_trigger: &'static str,
  #[allow(dead_code)]
  clean_query: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayConnState {
  Disconnected,
  Connecting,
  Connected,
}

/// State of the live conversation with the gateway agent. Created on the first "hey
/// claude" turn that finalizes with a non-empty query and torn down only on
/// Escape/close/fatal error.
#[derive(Debug, Clone)]
struct GatewayConversation {
  /// `None` until the daemon's `runs.create` response delivers it; then reused for
  /// follow-ups.
  thread_id: Option<String>,
  tag_label: &'static str,
  tag_icon: &'static str,
  last_query: String,
  reply: String,
  status: ConvStatus,
  error_msg: String,
  activity_label: Option<String>,
  awaiting_run_id: Option<String>,
  /// The live (stripped) draft of a follow-up the user is currently speaking, before it
  /// finalizes. When set, the overlay shows it as the forming query with an empty reply so
  /// the new utterance gets its own space instead of cramping under the prior reply.
  composing_query: Option<String>,
  /// Time of the last sign of life (query sent or any inbound event). Drives the
  /// no-response timeout in `on_tick`; `None` while idle/done.
  last_activity: Option<Instant>,
  /// True once the daemon has acknowledged the current run (any run-level event). Selects
  /// the fast ack deadline vs. the longer post-ack stall deadline; reset on each submit.
  acknowledged: bool,
}

impl GatewayConversation {
  fn new(tag_label: &'static str, tag_icon: &'static str) -> Self {
    GatewayConversation {
      thread_id: None,
      tag_label,
      tag_icon,
      last_query: String::new(),
      reply: String::new(),
      status: ConvStatus::Thinking,
      error_msg: String::new(),
      activity_label: None,
      awaiting_run_id: None,
      composing_query: None,
      last_activity: None,
      acknowledged: false,
    }
  }
}

fn friendly_gateway_error(raw: &str) -> String {
  let lower = raw.to_ascii_lowercase();
  if lower.contains("agent_unavailable") {
    "Claude is unavailable — is the browser adapter connected?".to_string()
  } else if lower.contains("refused") || lower.contains("os error 61") {
    "Gateway unavailable — is local-agent-gatewayd running?".to_string()
  } else if lower.contains("closed") || lower.contains("send failed") {
    "Connection to the gateway was lost.".to_string()
  } else if lower.contains("user_approval_required") {
    "Gateway rejected the request (approval required).".to_string()
  } else {
    format!("Gateway error: {raw}")
  }
}

/// One-line description of a gateway event for the `AZAD_GATEWAY event=` log.
fn gateway_event_summary(event: &GatewayEvent) -> String {
  match event {
    GatewayEvent::Connected => "connected".to_string(),
    GatewayEvent::ServerReady { protocol, version } => {
      format!("server_ready protocol={protocol:?} version={version:?}")
    }
    GatewayEvent::Disconnected { reason } => format!("disconnected reason={reason:?}"),
    GatewayEvent::RunAccepted { thread_id, run_id } => {
      format!("run_accepted thread_id={thread_id:?} run_id={run_id:?}")
    }
    GatewayEvent::Delta { content, delta, replace, .. } => format!(
      "delta content_chars={} delta_chars={} replace={replace}",
      content.as_ref().map_or(0, |s| s.chars().count()),
      delta.as_ref().map_or(0, |s| s.chars().count()),
    ),
    GatewayEvent::Completed { content, .. } => {
      format!("completed reply_chars={}", content.chars().count())
    }
    GatewayEvent::Activity { phase, label } => format!("activity phase={phase:?} label={label:?}"),
    GatewayEvent::Failed { error } => format!("failed error={error:?}"),
    GatewayEvent::RequestError { error } => format!("request_error error={error:?}"),
  }
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

impl AppController {
  fn new(cfg: AzadConfig) -> Self {
    let startup_listen_mode = preferred_store::load_startup_listen_mode();
    let always_listening_enabled =
      startup_listen_mode.initial_listen_enabled(preferred_store::load_always_listening_enabled());
    let run_on_startup_enabled = effective_run_on_startup_enabled(
      preferred_store::load_run_on_startup_enabled(),
      platform::launch_agent_plist_exists(),
    );
    let activation_level = preferred_store::load_activation_level();
    let history_enabled = preferred_store::load_history_enabled();
    let paste_method = preferred_store::load_paste_method();
    let auto_submit_mode = preferred_store::load_auto_submit_mode();
    let append_trailing_space_on_paste = preferred_store::load_append_trailing_space_on_paste();
    let deduplicate_words_on_paste = preferred_store::load_deduplicate_words_on_paste();
    let convert_number_words_on_paste = preferred_store::load_convert_number_words_on_paste();
    let convert_spoken_emoji_on_paste = preferred_store::load_convert_spoken_emoji_on_paste();
    let lowercase_except_uppercase_words_on_paste =
      preferred_store::load_lowercase_except_uppercase_words_on_paste();
    let remove_hesitations_on_paste = preferred_store::load_remove_hesitations_on_paste();
    let overlay_position = preferred_store::load_overlay_position();
    let debug_stats_enabled = preferred_store::load_debug_stats_enabled();
    platform::set_overlay_debug_logs_enabled(debug_stats_enabled);
    let active_pack_id = preferred_store::load_active_model_pack()
      .filter(|id| models::pack_by_id(id).is_some())
      .unwrap_or_else(|| models::default_pack().id.to_string());
    let transcript_index = TranscriptIndex::load();
    let removed_words = preferred_store::migrate_hesitations_out_of_removed_words(
      preferred_store::load_removed_words(),
    );
    let mut connectors = connectors::builtin_connectors();
    if let Some(enabled_ids) = preferred_store::load_enabled_connector_ids() {
      for c in &mut connectors {
        c.enabled = enabled_ids.iter().any(|id| id == c.id);
      }
    }
    Self {
      cfg,
      session: None,
      session_device_id: None,
      next_session_id: 1,
      shutdown_started: false,
      device_controller: None,
      device_snapshot: None,
      device_menu_expanded: false,
      always_listening_enabled,
      pending_always_listening_enabled: None,
      manual_hold_active: false,
      manual_hold_history_grace_until: None,
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
      pending_hold_release_raw_requested: false,
      run_on_startup_enabled,
      startup_listen_mode,
      activation_level,
      history_enabled,
      paste_method,
      auto_submit_mode,
      append_trailing_space_on_paste,
      deduplicate_words_on_paste,
      convert_number_words_on_paste,
      convert_spoken_emoji_on_paste,
      lowercase_except_uppercase_words_on_paste,
      remove_hesitations_on_paste,
      overlay_position,
      debug_stats_enabled,
      turn_started_at: HashMap::new(),
      session_recovery_state: SessionRecoveryState::Healthy,
      session_fault_window: Vec::new(),
      last_session_error_was_stream_fault: false,
      pending_recovery_restart: false,
      cancel_vad_show_suppressed_until: None,
      active_pack_id,
      models_ready: false,
      onboarding_complete: preferred_store::load_onboarding_complete().unwrap_or(false),
      pending_onboarding: false,
      onboarding_active: false,
      last_onboarding_view_model: None,
      pending_first_launch_settings: false,
      download_handle: None,
      download_progress: (0, 0),
      download_progress_dirty: false,
      transcript_index,
      history_browsing: false,
      history_browse_index: 0,
      history_visible_start: 0,
      history_expanded: false,
      history_search_query: String::new(),
      removed_words,
      connectors,
      active_connector: None,
      gateway_conn: GatewayConnState::Disconnected,
      gateway_conv: None,
      gateway_protocol_mismatch: None,
      azad_turn: None,
      spotify_turn: None,
      apple_lm_availability: AvailabilityReport {
        state: AvailabilityState::Unavailable,
        detail: None,
      },
      apple_lm_last_probe: None,
      azad_pending_query: None,
      spotify_app_installed: spotify_client::spotify_app_installed(),
    }
  }

  fn bootstrap(&mut self) {
    self.refresh_models_ready();
    platform::set_overlay_position(self.overlay_position);
    eprintln!(
      "AZAD_PERMISSIONS accessibility={:?} microphone={:?} input_monitoring={:?}",
      platform::accessibility_authorization(),
      platform::microphone_authorization(),
      platform::input_monitoring_authorization(),
    );
    // Seed onboarding for users who predate the welcome flow: an unset flag plus
    // an already-downloaded model means a returning user — mark them onboarded so
    // they're never sent through first-run setup. A genuinely fresh profile (no
    // model yet) keeps the flag unset and goes through onboarding.
    if preferred_store::load_onboarding_complete().is_none() && self.models_ready {
      self.onboarding_complete = true;
      preferred_store::save_onboarding_complete(true);
    }
    // A fresh profile goes through the welcome flow; a returning user whose
    // model is somehow missing still gets the model setup view.
    if !self.onboarding_complete {
      self.pending_onboarding = true;
    } else if !self.models_ready {
      self.pending_first_launch_settings = true;
    }
    self.start_device_controller();
    self.render_device_menu();
    self.ensure_session_if_capture_should_be_live();
  }

  fn refresh_models_ready(&mut self) {
    let pack = models::pack_by_id(&self.active_pack_id).unwrap_or_else(models::default_pack);
    self.models_ready = models::check_pack_status(pack) == PackStatus::Ready;
    if self.models_ready {
      self.cfg.rebuild_pipeline_paths(pack);
    }
  }

  /// The app may spawn a capture session only after setup is complete and both
  /// required permissions are already granted.
  fn ready_to_run(&self) -> bool {
    self.models_ready
      && self.onboarding_complete
      && platform::microphone_authorization() == platform::PermissionStatus::Granted
      && platform::accessibility_authorization() == platform::PermissionStatus::Granted
  }

  /// Record a finalized turn to the transcript history, unless the user has
  /// turned history off. Existing entries stay browsable; only new writes stop.
  fn record_history(&mut self, turn_id: u64, cleaned: &str) {
    if !self.history_enabled {
      return;
    }
    let cleaned = self.strip_active_trigger(cleaned);
    if let Some(index) = &mut self.transcript_index {
      index.append(turn_id, &self.finalizing_draft, &cleaned);
    }
  }

  fn record_shutdown_snapshot(&mut self) -> ShutdownSnapshotOutcome {
    if !self.history_enabled {
      return ShutdownSnapshotOutcome::HistoryDisabled;
    }
    if self.transcript_index.is_none() {
      return ShutdownSnapshotOutcome::NoTranscriptIndex;
    }
    let Some((turn_id, text)) = self.shutdown_snapshot_candidate() else {
      return ShutdownSnapshotOutcome::NoActiveDraft;
    };
    if self.last_pasted_turn_id == Some(turn_id) || self.raw_handled_turn_id == Some(turn_id) {
      return ShutdownSnapshotOutcome::AlreadyHandled;
    }
    let char_count = text.chars().count();
    if let Some(index) = &mut self.transcript_index {
      index.append(turn_id, &text, &text);
    }
    ShutdownSnapshotOutcome::Saved { turn_id, char_count }
  }

  fn shutdown_snapshot_candidate(&self) -> Option<(u64, String)> {
    if self.cancelled || !self.has_active_transcription_turn() {
      return None;
    }

    let live_text = self.strip_active_trigger(&self.latest_draft);
    if let Some(turn_id) = self
      .current_turn_id
      .or_else(|| if self.latest_seen_turn_id > 0 { Some(self.latest_seen_turn_id) } else { None })
    {
      let text = live_text.trim();
      if !text.is_empty() {
        return Some((turn_id, text.to_string()));
      }
    }

    let finalizing_text = self.strip_active_trigger(&self.finalizing_draft);
    if let Some(turn_id) = self.finalizing_turn_id {
      let text = finalizing_text.trim();
      if !text.is_empty() {
        return Some((turn_id, text.to_string()));
      }
    }

    if self.held_top_overlay_active() {
      let held_text = self.strip_active_trigger(&self.held_top_draft);
      let text = held_text.trim();
      if !text.is_empty() {
        let turn_id = self
          .finalizing_turn_id
          .or(self.current_turn_id)
          .unwrap_or(self.latest_seen_turn_id);
        if turn_id > 0 {
          return Some((turn_id, text.to_string()));
        }
      }
    }

    None
  }

  fn start_device_controller(&mut self) {
    let has_controller = self.device_controller.is_some();
    if !should_start_device_controller(has_controller, platform::microphone_authorization()) {
      if !has_controller {
        self.device_snapshot = None;
      }
      return;
    }

    let preferred = preferred_store::load_preferred_device_id();

    let emit: Arc<dyn Fn(DeviceEvent) + Send + Sync> =
      Arc::new(|ev| send_event(AppEvent::Device(ev)));

    match DeviceController::start(preferred, emit) {
      Ok(controller) => {
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
      AppEvent::ShutdownRequested => self.handle_shutdown_requested(),
      AppEvent::HotkeyPressed => self.handle_hotkey_pressed(),
      AppEvent::HotkeyReleased { raw_requested } => self.handle_hotkey_released(raw_requested),
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
      AppEvent::SettingsSelectStartupListenMode(mode) => {
        self.handle_settings_select_startup_listen_mode(mode)
      }
      AppEvent::SettingsToggleDebugStats(enabled) => {
        self.handle_settings_toggle_debug_stats(enabled)
      }
      AppEvent::SettingsSetActivationLevel(value) => {
        self.handle_settings_set_activation_level(value)
      }
      AppEvent::SettingsSelectPasteMethod(method) => {
        self.handle_settings_select_paste_method(method)
      }
      AppEvent::SettingsSelectAutoSubmit(mode) => self.handle_settings_select_auto_submit(mode),
      AppEvent::SettingsSelectOverlayPosition(pos) => {
        self.handle_settings_select_overlay_position(pos)
      }
      AppEvent::SettingsToggleHistory(enabled) => self.handle_settings_toggle_history(enabled),
      AppEvent::SettingsToggleAppendTrailingSpace(enabled) => {
        self.handle_settings_toggle_append_trailing_space(enabled)
      }
      AppEvent::SettingsToggleDeduplicateWords(enabled) => {
        self.handle_settings_toggle_deduplicate_words(enabled)
      }
      AppEvent::SettingsToggleConvertNumberWords(enabled) => {
        self.handle_settings_toggle_convert_number_words(enabled)
      }
      AppEvent::SettingsToggleConvertSpokenEmoji(enabled) => {
        self.handle_settings_toggle_convert_spoken_emoji(enabled)
      }
      AppEvent::SettingsToggleLowercaseExceptUppercaseWords(enabled) => {
        self.handle_settings_toggle_lowercase_except_uppercase_words(enabled)
      }
      AppEvent::SettingsToggleRemoveHesitations(enabled) => {
        self.handle_settings_toggle_remove_hesitations(enabled)
      }
      AppEvent::SettingsSetListenModifier { bit, enabled } => {
        self.handle_settings_set_listen_modifier(bit, enabled)
      }
      AppEvent::SettingsToggleConnector { index, enabled } => {
        self.handle_settings_toggle_connector(index, enabled)
      }
      AppEvent::SettingsOpenSystemSettings => {
        platform::open_system_settings();
      }
      AppEvent::SettingsRecheckAppleLm => {
        self.refresh_apple_lm_availability(true);
        platform::update_settings_window(self.settings_view_model());
        self.maybe_resume_pending_azad();
      }
      AppEvent::SettingsAddRemovedWord(word) => self.handle_settings_add_removed_word(word),
      AppEvent::SettingsRemoveRemovedWord(word) => self.handle_settings_remove_removed_word(word),
      AppEvent::SettingsRefresh => self.handle_settings_refresh(),
      AppEvent::SettingsDownloadModel(pack_id) => self.handle_settings_download_model(&pack_id),
      AppEvent::SettingsSetDownloadPaused(paused) => {
        self.handle_settings_set_download_paused(paused)
      }
      AppEvent::RequestPermission(permission) => self.handle_request_permission(&permission),
      AppEvent::OnboardingGetStarted => self.handle_onboarding_get_started(),
      AppEvent::OnboardingDownloadModel => {
        let pack_id = self.active_pack_id.clone();
        self.handle_settings_download_model(&pack_id);
      }
      AppEvent::OnboardingSetListenModifier { bit, enabled } => {
        self.handle_onboarding_set_listen_modifier(bit, enabled)
      }
      AppEvent::ModelDownloadProgress { pack_id, bytes_done, bytes_total } => {
        self.handle_model_download_progress(&pack_id, bytes_done, bytes_total)
      }
      AppEvent::ModelDownloadCompleted(pack_id) => self.handle_model_download_completed(&pack_id),
      AppEvent::ModelDownloadError { pack_id, message } => {
        self.handle_model_download_error(&pack_id, &message)
      }
      AppEvent::OverlayCancel => self.handle_overlay_cancel(),
      AppEvent::ArrowNavigate(direction) => self.handle_arrow_navigate(direction),
      AppEvent::HistoryExpand => self.handle_history_expand(),
      AppEvent::HistoryCollapse => self.handle_history_collapse(),
      AppEvent::HistorySearchChanged(query) => self.handle_history_search_changed(query),
      AppEvent::HistorySearchAppend(s) => self.handle_history_search_append(&s),
      AppEvent::HistorySearchBackspace => self.handle_history_search_backspace(),
      AppEvent::HistorySearchDeleteWord => self.handle_history_search_delete_word(),
      AppEvent::HistorySearchClear => self.handle_history_search_clear(),
      AppEvent::Speech(ev) => self.handle_speech_event(ev),
      AppEvent::Device(ev) => self.handle_device_event(ev),
      AppEvent::Gateway(ev) => self.handle_gateway_event(ev),
    }
  }

  fn handle_shutdown_requested(&mut self) {
    if self.shutdown_started {
      platform::terminate_app();
      return;
    }
    self.shutdown_started = true;
    let outcome = self.record_shutdown_snapshot();
    eprintln!("AZAD_SHUTDOWN requested snapshot={outcome:?}");
    if let Some(session) = &self.session {
      session.cancel();
    }
    platform::terminate_app();
  }

  fn start_session(&mut self) {
    if !self.ready_to_run() {
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
    self.clear_overlay_pending();
    self.latest_seen_turn_id = 0;
    self.turn_accept_floor = 1;
    self.current_turn_id = None;
    self.dispatch_hotkey_input(HotkeyInput::SessionReset);
    self.raw_finalize_requested = false;
    self.pending_hold_release_raw_requested = false;
    self.manual_hold_history_grace_until = None;
    self.reset_activity_history();
    self.busy_border_phase = 0.0;
    self.turn_started_at.clear();

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
        start_min_rms_db_for_activation_level(self.activation_level),
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
    if !self.ready_to_run() {
      return;
    }
    if self.session.is_none() && self.should_keep_capture_for_followups() {
      self.start_session();
    }
  }

  fn ensure_session_if_capture_should_be_live(&mut self) {
    if self.should_keep_capture_for_followups() {
      self.ensure_session();
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

  fn onboarding_blocks_runtime_input(&self) -> bool {
    self.pending_onboarding || self.onboarding_active || !self.onboarding_complete
  }

  fn handle_hotkey_pressed(&mut self) {
    if self.onboarding_blocks_runtime_input() {
      return;
    }
    self.log_input_event(InputLogEvent::HotkeyPressed);
    self.manual_hold_history_grace_until = None;
    // Pressing opt+space while in history mode dismisses history (without
    // pasting) and starts a fresh dictation turn. The user is signalling
    // "I want to talk now" — don't trap them in the list.
    if self.history_browsing {
      self.exit_history_mode();
    }
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

  fn handle_hotkey_released(&mut self, raw_requested: bool) {
    if self.onboarding_blocks_runtime_input() {
      return;
    }
    self.log_input_event(InputLogEvent::HotkeyReleased { raw_requested });
    if self.history_browsing {
      // Once in history mode the user is no longer required to hold opt+space —
      // they navigate with Up/Down and dismiss with Esc/Left or paste with Enter.
      // The release is a no-op so they can let go and keep browsing freely.
      return;
    }
    if !self.models_ready {
      return;
    }
    self.pending_hold_release_raw_requested = raw_requested;
    self.dispatch_hotkey_input(HotkeyInput::HoldReleased { snapshot: self.hotkey_snapshot() });
    self.pending_hold_release_raw_requested = false;
  }

  fn handle_finalize_hotkey_pressed(&mut self, raw_requested: bool) {
    if self.onboarding_blocks_runtime_input() {
      return;
    }
    self.log_input_event(InputLogEvent::FinalizeHotkeyPressed { raw_requested });
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
    // Engine-stuck fallback for plain Enter: when there's a pending finalizing turn
    // with a finalized draft sitting in the overlay AND the engine is `Idle` (i.e.
    // no fresh `FinalText` is going to arrive), fall through to the raw-finalize
    // path so the captured text actually pastes. This catches the post
    // opt+space-during-finalize stuck state where the engine's finalize loop got
    // disrupted and won't emit FinalText for the in-flight turn. The
    // `last_pasted_turn_id != finalizing_turn_id` guard prevents a double-paste if
    // a delayed FinalText eventually does arrive.
    let engine_stuck = self.engine_state == EngineState::Idle
      && self.finalizing_turn_id.is_some()
      && !self.finalizing_draft.trim().is_empty()
      && self.last_pasted_turn_id != self.finalizing_turn_id;
    if raw_requested || engine_stuck {
      if engine_stuck && !raw_requested {
        if let Some(turn_id) = self.finalizing_turn_id {
          self.log_input_event(InputLogEvent::RawFallbackFired { turn_id });
        }
      }
      let turn_id = self.raw_finalize_target_turn_id();
      let _ = self.try_finalize_with_raw_text(turn_id);
    }
  }

  fn handle_menu_toggle_always_listening(&mut self) {
    if self.onboarding_blocks_runtime_input() {
      return;
    }
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
    // Arm the overlay to surface on the first live draft of the turn that
    // follows enabling always-listening. The engine force-starts a
    // `ManualOverride` turn right after this toggle; if we cleared the flag
    // here, that turn's drafts would never lift the overlay, because the
    // "Listen ENABLED" notice holds `overlay_visible=true` and
    // `turn_started_should_arm_pending` no-ops while the overlay looks visible.
    // On disable we clear it as before.
    if enabled {
      self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
    } else {
      self.clear_overlay_pending();
    }
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
    self.clear_overlay_pending();
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
      &self.finalizing_draft,
    ) || split_overlay_visible_with_live_divergence_for_state(
      self.finalizing_turn_id,
      self.current_turn_id,
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
    self.clear_overlay_pending();
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
        self.clear_overlay_pending();
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
        self.manual_hold_history_grace_until =
          Some(Instant::now() + Duration::from_millis(HISTORY_MANUAL_HOLD_RELEASE_GRACE_MS));
        self.hold_saw_speech = false;
        let plan = manual_hold_release_plan(
          self.always_listening_enabled,
          should_finalize,
          has_started_turn,
        );
        if should_latch_raw_on_hold_release(self.pending_hold_release_raw_requested, plan.action) {
          self.raw_finalize_requested = true;
        }
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
    if !should_update_selected_device(self.current_device_id(), &device_id) {
      return;
    }

    preferred_store::save_preferred_device_id(&device_id);

    if let Some(controller) = &self.device_controller {
      let controller = controller.clone();
      let _ = std::thread::Builder::new()
        .name("azad-device-select".to_string())
        .spawn(move || {
          if let Err(err) = controller.set_preferred(Some(device_id)) {
            eprintln!("Azad: failed to set preferred device: {err}");
          }
        });
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
    self.log_input_event(InputLogEvent::OverlayCancel);
    if self.history_browsing {
      // Esc always fully dismisses history regardless of expand state. The
      // list-mode "back" gesture is the Left arrow (HistoryCollapse), which
      // takes a different code path.
      self.exit_history_mode();
      return;
    }
    // Escape ends a Hey Spotify one-shot card.
    if self.spotify_turn.take().is_some() {
      self.cancelled = true;
      self.dispatch_hotkey_input(HotkeyInput::OverlayCancelled);
      self.hide_overlay();
      return;
    }
    // Escape ends a Hey Azad one-shot card before gateway teardown.
    if self.azad_turn.take().is_some() {
      self.azad_pending_query = None;
      self.cancelled = true;
      self.dispatch_hotkey_input(HotkeyInput::OverlayCancelled);
      self.hide_overlay();
      return;
    }
    // Escape ends a gateway conversation: close the thread, tear down the socket, and
    // return capture to the normal rule. Takes precedence over the dictation cancel.
    if let Some(conv) = self.gateway_conv.take() {
      if let Some(thread_id) = conv.thread_id {
        gateway::send_command(GatewayCommand::Close {
          req_id: gateway::make_request_id(),
          thread_id,
        });
      }
      gateway::send_command(GatewayCommand::Shutdown);
      self.gateway_conn = GatewayConnState::Disconnected;
      self.cancelled = true;
      self.manual_hold_active = false;
      self.manual_hold_history_grace_until = None;
      self.hold_saw_speech = false;
      self.raw_finalize_requested = false;
      self.finalizing_deadline = None;
      self.finalizing_turn_id = None;
      self.finalizing_draft.clear();
      self.raw_handled_turn_id = None;
      self.turn_started_at.clear();
      self.dispatch_hotkey_input(HotkeyInput::OverlayCancelled);
      if let Some(session) = &self.session {
        session.release_manual_hold();
        session.cancel_current_turn();
        if !self.always_listening_enabled {
          session.set_capture_enabled(false);
        }
      }
      self.hide_overlay();
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
    self.manual_hold_history_grace_until = None;
    self.hold_saw_speech = false;
    self.dispatch_hotkey_input(HotkeyInput::OverlayCancelled);
    self.raw_finalize_requested = false;
    self.pending_hold_release_raw_requested = false;
    self.clear_overlay_pending();
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

    if self.session.is_none() && self.should_keep_capture_for_followups() {
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
      | SpeechEvent::TurnStarted { session_id, .. }
      | SpeechEvent::DraftUpdated { session_id, .. }
      | SpeechEvent::Finalizing { session_id, .. }
      | SpeechEvent::FinalizingCancelled { session_id, .. }
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
    if self.shutdown_started {
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
        self.log_input_event(InputLogEvent::EngineSpeechStart);
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
        // A live gateway conversation keeps the prior reply on screen while the user
        // speaks the follow-up; only hide when there's no conversation open.
        if self.overlay_visible && self.gateway_conv.is_none() {
          self.hide_overlay();
        }
        // In auto-VAD mode, wait for actual draft text before showing overlay.
        self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
      }
      SpeechEvent::TurnStarted { reason, .. } => {
        // Defensive overlay-arm for engine-side `ManualOverride` turn starts.
        // The VAD path is fully handled by `SpeechStartedByVad` above (with
        // its own side effects); the manual path historically had no engine
        // event at all, so a `start_turn(ManualOverride)` whose hotkey
        // effect didn't run on the renderer side (e.g. state desync) left
        // the live overlay hidden through the entire turn — user reported
        // turn 9 (audit "I'm happy to answer questions" 14 s, 95.5 %).
        //
        // Decision is in `turn_started_should_arm_pending`: manual paths
        // arm only when the overlay is currently hidden. Manual hold's
        // normal happy path opens the overlay synchronously via
        // `HotkeyEffect::ActivateManualHold` -> `show_overlay_listening`
        // BEFORE this event arrives; that path leaves `overlay_visible=true`
        // and we no-op here. The desync case has `overlay_visible=false`
        // when this arrives — we arm the flag so the next non-empty
        // `DraftUpdated` calls `show_overlay_listening`. Crucially do NOT
        // call `reset_turn_state()`, `hide_overlay()`, or clear
        // `latest_draft` — manual hold owns those.
        let armed = turn_started_should_arm_pending(reason, self.overlay_visible);
        if self.debug_stats_enabled {
          let cancel_suppress_active =
            self.cancel_vad_show_suppressed_until.is_some_and(|d| Instant::now() < d);
          eprintln!(
            "AZAD_OVERLAY_TURN_STARTED reason={:?} armed={} \
             overlay_visible={} pending_before={} \
             accessibility_notice_deadline_active={} \
             listen_toggle_notice_active={} \
             finalizing_turn_id={:?} manual_hold_active={} \
             cancel_suppress_active={} current_turn={:?}",
            reason,
            armed,
            self.overlay_visible,
            self.overlay_pending_vad_text,
            self.accessibility_notice_deadline.is_some(),
            self.listen_toggle_notice.is_some(),
            self.finalizing_turn_id,
            self.manual_hold_active,
            cancel_suppress_active,
            self.current_turn_id,
          );
        }
        if armed {
          self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
        }
      }
      SpeechEvent::DraftUpdated { turn_id, committed, live, .. } => {
        if !self.accept_turn(turn_id) {
          return;
        }
        // A raw finalize (opt+enter) already pasted and dismissed this turn. The
        // engine keeps streaming drafts for a beat afterward; without this guard
        // those late drafts re-show the overlay (the "flash"/"stuck overlay after
        // raw paste" bug). Mirrors the Finalizing handler's raw-handled guard.
        if should_ignore_finalizing_event(self.raw_handled_turn_id, turn_id) {
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
          self.update_active_connector();
          // In a live conversation, surface the follow-up the user is speaking as the
          // forming query so it has its own space (rendered above the reply slot).
          if self.gateway_conv.is_some() {
            let forming = self.strip_active_trigger(&self.latest_draft);
            if let Some(conv) = self.gateway_conv.as_mut() {
              conv.composing_query = Some(forming);
            }
          }
          // A live draft supersedes the transient "Listen ENABLED" notice: the
          // user is mid-utterance and needs to see their words now, not the
          // notice. Dismiss it and switch straight to the listening overlay,
          // otherwise the notice owns the overlay (`visible=true`) so the
          // action below resolves to `Clear` until the notice's own deadline
          // expires (up to ~600 ms of hidden live text).
          if self.listen_toggle_notice.is_some() {
            self.listen_toggle_notice = None;
            self.accessibility_notice_deadline = None;
            self.show_overlay_listening();
          }
          let cancel_suppression_active = self
            .cancel_vad_show_suppressed_until
            .is_some_and(|deadline| Instant::now() < deadline);
          // Recomputed fresh each draft, independent of the pending latch. A turn we
          // are legitimately capturing (always-listening with overlay-on-start, or
          // manual hold) and not history-browsing should surface its live text even
          // if the latch was dropped between turn-start and now. We have real text
          // here (non-empty merged), so this is not a noise-only flash.
          let eligible_to_show = !self.history_browsing
            && ((self.always_listening_enabled && self.cfg.show_overlay_on_vad_start)
              || self.manual_hold_active);
          let action = draft_update_overlay_action(
            self.overlay_pending_vad_text,
            self.overlay_visible,
            cancel_suppression_active,
            eligible_to_show,
          );
          if self.debug_stats_enabled {
            eprintln!(
              "AZAD_OVERLAY_DRAFT turn_id={} merged_chars={} \
               pending={} visible={} cancel_suppress={} \
               accessibility_notice_deadline_active={} \
               finalizing_turn_id={:?} manual_hold_active={} \
               action={:?}",
              turn_id,
              self.latest_draft.chars().count(),
              self.overlay_pending_vad_text,
              self.overlay_visible,
              cancel_suppression_active,
              self.accessibility_notice_deadline.is_some(),
              self.finalizing_turn_id,
              self.manual_hold_active,
              action,
            );
          }
          match action {
            DraftOverlayAction::Show => {
              self.show_overlay_listening();
              self.clear_overlay_pending();
            }
            DraftOverlayAction::Clear => {
              self.clear_overlay_pending();
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
        self.log_input_event(InputLogEvent::EngineSpeechFinalizing {
          turn_id,
          draft_chars: current_draft.chars().count(),
        });
        if self.debug_stats_enabled {
          let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
          eprintln!(
            "AZAD_FINALIZING_RECV ts_ms={} turn_id={} draft_chars={} overlay_visible={} \
             prior_finalizing_turn_id={:?}",
            ts_ms,
            turn_id,
            current_draft.chars().count(),
            self.overlay_visible,
            self.finalizing_turn_id,
          );
        }
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

        self.clear_overlay_pending();
        self.finalizing_deadline =
          Some(Instant::now() + Duration::from_millis(self.cfg.final_pass_timeout_ms));
        self.render_finalizing_overlay_state();
      }
      SpeechEvent::FinalizingCancelled { turn_id, .. } => {
        self.log_input_event(InputLogEvent::EngineFinalizingCancelled { turn_id });
        if self.debug_stats_enabled {
          eprintln!(
            "AZAD_FINALIZING_CANCELLED_RECV turn_id={} finalizing_turn_id={:?} \
             overlay_visible={}",
            turn_id, self.finalizing_turn_id, self.overlay_visible
          );
        }
        if !self.accept_turn(turn_id) {
          return;
        }
        // Tentative finalize for this turn was undone — clear the finalize state
        // so the pulsing border stops and the overlay returns to live listening.
        // Mirrors the inverse of the `Finalizing` arm above.
        if self.finalizing_turn_id == Some(turn_id) {
          self.finalizing_turn_id = None;
        }
        self.finalizing_deadline = None;
        self.finalizing_draft.clear();
        self.saw_vad_start_during_finalizing = false;
        self.raw_handled_turn_id = None;
        self.raw_finalize_requested = false;
        if self.overlay_visible {
          self.render_listening_overlay();
        }
      }
      SpeechEvent::FinalText { turn_id, text, .. } => {
        self.log_input_event(InputLogEvent::EngineFinalText {
          turn_id,
          text_chars: text.chars().count(),
        });
        if !self.accept_turn(turn_id) {
          return;
        }

        let cleaned = text.trim().to_string();
        let has_user_visible_context = final_text_has_user_visible_context(
          turn_id,
          self.current_turn_id,
          self.finalizing_turn_id,
          self.overlay_visible,
          self.manual_hold_active,
          &self.latest_draft,
          &self.finalizing_draft,
        );
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
          self.current_turn_id,
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
            if self.spotify_should_handle_turn() {
              self.submit_to_spotify(turn_id, &cleaned);
            } else if self.azad_should_handle_turn() {
              self.submit_to_azad(turn_id, &cleaned);
            } else if self.gateway_should_handle_turn() {
              self.submit_to_gateway(turn_id, &cleaned);
            } else if self.try_paste(turn_id, TranscriptMode::Normal, &cleaned) {
              self.last_pasted_turn_id = Some(turn_id);
              self.record_history(turn_id, &cleaned);
            } else {
              eprintln!("Azad: failed to auto-paste transcript (clipboard still contains text)");
            }
          }
          // A live gateway/Azad/Spotify conversation owns the card; only fall back to
          // listening when no conversation is open.
          if self.overlay_visible
            && self.gateway_conv.is_none()
            && self.azad_turn.is_none()
            && self.spotify_turn.is_none()
          {
            self.render_listening_overlay();
          }
          return;
        }

        if cleaned.is_empty() {
          self.clear_held_top_overlay();
          self.turn_started_at.remove(&turn_id);
          self.raw_handled_turn_id = None;
          self.maybe_start_deferred_vad_turn();
          if !self.should_keep_capture_for_followups() {
            if let Some(session) = &self.session {
              session.set_capture_enabled(false);
            }
          }
          // Bare "hey claude" / empty follow-up: drop the (empty) forming query so the
          // prior exchange shows again, and keep the conversation on screen, listening.
          if let Some(conv) = self.gateway_conv.as_mut() {
            conv.composing_query = None;
          }
          if self.gateway_conv.is_some() {
            self.show_conversation_overlay();
          }
          return;
        }
        if self.raw_handled_turn_id == Some(turn_id) {
          self.clear_held_top_overlay();
          self.turn_started_at.remove(&turn_id);
          self.raw_handled_turn_id = None;
          self.maybe_start_deferred_vad_turn();
          if !self.should_keep_capture_for_followups() {
            if let Some(session) = &self.session {
              session.set_capture_enabled(false);
            }
          }
          return;
        }
        if !has_user_visible_context {
          self.log_input_event(InputLogEvent::FinalTextSuppressed {
            turn_id,
            text_chars: cleaned.chars().count(),
            reason: "hidden_without_visible_draft",
          });
          self.clear_held_top_overlay();
          self.turn_started_at.remove(&turn_id);
          self.raw_handled_turn_id = None;
          self.latest_final = None;
          self.maybe_start_deferred_vad_turn();
          if !self.should_keep_capture_for_followups() {
            if let Some(session) = &self.session {
              session.set_capture_enabled(false);
            }
          }
          return;
        }
        self.raw_handled_turn_id = None;
        self.latest_final = Some(cleaned.clone());
        if !self.cancelled && self.last_pasted_turn_id != Some(turn_id) {
          if self.spotify_should_handle_turn() {
            self.submit_to_spotify(turn_id, &cleaned);
          } else if self.azad_should_handle_turn() {
            self.submit_to_azad(turn_id, &cleaned);
          } else if self.gateway_should_handle_turn() {
            // Route to the gateway instead of pasting; the overlay shows the reply.
            self.submit_to_gateway(turn_id, &cleaned);
          } else {
            // Keep the finalizing spinner on screen through the paste window so the visual
            // transition coincides with the paste appearing in the target app. Hiding first
            // and then doing a ~100 ms blocking paste creates a perceptible "overlay gone /
            // nothing happening / paste appears" gap; hiding after leaves the overlay
            // responsible for "still working" state right up until the moment the text lands.
            let should_hide_overlay = !self.manual_hold_active && !hold_top_for_next_turn;
            if self.try_paste(turn_id, TranscriptMode::Normal, &cleaned) {
              self.last_pasted_turn_id = Some(turn_id);
              self.record_history(turn_id, &cleaned);
            } else {
              eprintln!("Azad: failed to auto-paste transcript (clipboard still contains text)");
            }
            if should_hide_overlay {
              self.hide_overlay();
            }
          }
        }
        self.maybe_start_deferred_vad_turn();
        if !self.should_keep_capture_for_followups() {
          if let Some(session) = &self.session {
            session.set_capture_enabled(false);
          }
        }
      }
      SpeechEvent::SessionEnded { session_id } => {
        self.log_input_event(InputLogEvent::EngineSessionEnded);
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
        // A live gateway/Azad/Spotify conversation owns the overlay across session recycles.
        if self.gateway_conv.is_none() && self.azad_turn.is_none() && self.spotify_turn.is_none() {
          if !self.cancelled
            && self.latest_seen_turn_id > 0
            && self.last_pasted_turn_id != Some(self.latest_seen_turn_id)
          {
            // Paste-then-hide: the overlay's "still working" state stays on screen until the
            // paste actually lands, so dismissal and paste appear on the same frame.
            if let Some(final_text) = self.latest_final.as_ref() {
              let cleaned = final_text.trim().to_string();
              if !cleaned.is_empty()
                && self.try_paste(self.latest_seen_turn_id, TranscriptMode::Normal, &cleaned)
              {
                self.last_pasted_turn_id = Some(self.latest_seen_turn_id);
              }
            }
            self.hide_overlay();
          }

          self.hide_overlay();
        }
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
        self.clear_overlay_pending();
        self.cancelled = false;
        self.last_pasted_turn_id = None;
        self.session_device_id = None;
        self.dispatch_hotkey_input(HotkeyInput::SessionReset);
        self.reset_activity_history();
        self.busy_border_phase = 0.0;
        self.turn_started_at.clear();

        if should_restart && self.should_keep_capture_for_followups() {
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
        }
      }
    }
  }

  fn on_tick(&mut self) {
    if self.pending_onboarding {
      self.pending_onboarding = false;
      self.onboarding_active = true;
      eprintln!("AZAD_ONBOARDING showing welcome window");
      platform::set_status_item_visible(false);
      let model = self.onboarding_view_model();
      platform::show_onboarding_window(model.clone());
      self.last_onboarding_view_model = Some(model);
    }
    if self.onboarding_active {
      platform::ensure_hotkey_event_tap_if_accessibility_granted();
      self.start_device_controller();
      // Push the dynamic state (download status, the "Get started" gate, and
      // permission indicators) so the welcome window updates live as the
      // download progresses and the user grants access in System Settings.
      let model = self.onboarding_view_model();
      if onboarding_view_model_changed(&self.last_onboarding_view_model, &model) {
        platform::update_onboarding_window(model.clone());
        self.last_onboarding_view_model = Some(model);
      }
    }
    if platform::settings_window_is_open() {
      platform::ensure_hotkey_event_tap_if_accessibility_granted();
      self.start_device_controller();
      platform::refresh_settings_permissions(
        platform::accessibility_authorization(),
        platform::microphone_authorization(),
      );
    }
    if self.pending_first_launch_settings {
      self.pending_first_launch_settings = false;
      let mut vm = self.settings_view_model();
      vm.selected_tab = SettingsTab::Models;
      platform::show_settings_window(vm);
    }

    self.advance_activity_timeline();
    self.maybe_apply_pending_always_listening_toggle();
    self.maybe_auto_dismiss_command_overlays();

    // A live gateway run animates its own busy glow (its turn has finalized, so the
    // finalize-spinner branch below no longer ticks the phase) and fails closed to an
    // error if the daemon goes silent — otherwise a wedged/offline gateway leaves the
    // overlay stuck on "Thinking…" forever. The conversation re-renders in the overlay
    // block below.
    let conv_busy = self
      .gateway_conv
      .as_ref()
      .is_some_and(|c| matches!(c.status, ConvStatus::Thinking | ConvStatus::Streaming));
    if conv_busy {
      self.busy_border_phase =
        (self.busy_border_phase + OVERLAY_BUSY_PHASE_STEP).rem_euclid(std::f32::consts::TAU);
      // Fast fail on a daemon that never acknowledges the run; a longer window only after
      // it has, so a legitimately slow answer isn't killed mid-think.
      if let Some((acknowledged, Some(last))) =
        self.gateway_conv.as_ref().map(|c| (c.acknowledged, c.last_activity))
      {
        let window = if acknowledged { GATEWAY_STREAM_TIMEOUT } else { GATEWAY_ACK_TIMEOUT };
        if last.elapsed() >= window {
          let message = if acknowledged { GATEWAY_STALL_MESSAGE } else { GATEWAY_NO_ACK_MESSAGE };
          eprintln!(
            "AZAD_GATEWAY event=timeout acknowledged={acknowledged} secs={}",
            window.as_secs()
          );
          if let Some(conv) = self.gateway_conv.as_mut() {
            conv.status = ConvStatus::Error;
            conv.error_msg = message.to_string();
            conv.last_activity = None;
          }
        }
      }
    }

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

    // Click outside the overlay dismisses history. `on_tick` runs every 50 ms,
    // so polling `pressedMouseButtons` here gives us a global "did the user
    // just click somewhere else" signal without needing AppKit's NSEvent
    // monitor (which would require the `block` crate).
    if self.history_browsing && platform::poll_click_outside_overlay() {
      self.exit_history_mode();
      return;
    }

    // History-browse mode owns the overlay outright — neither the finalize-
    // animation tick nor the listening tick should re-render speech-mode
    // widgets over it. Without this guard, on_tick fires every 50 ms and the
    // listening branch below calls `render_overlay_text`, which hides every
    // `autocomplete_label` (the history list's row labels) and resizes the
    // window back to the speech footprint. That's what produces the user-
    // reported "blank box that doesn't grow" symptom: the history render
    // runs once on entry, then gets clobbered ~50 ms later by the next tick.
    if !self.history_browsing {
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
          // Preserve an armed pending flag across the notice teardown. The
          // notice expires ~600 ms in — typically just before the first live
          // draft of the force-start turn that enabling always-listening
          // kicks off. `hide_overlay()` clears the flag, which would strand
          // the overlay hidden for the whole turn (the recurring "no overlay,
          // flash at the end" bug). Re-arm after hiding so the next non-empty
          // draft calls `show_overlay_listening`.
          let preserve_pending = self.overlay_pending_vad_text;
          self.hide_overlay();
          self.overlay_pending_vad_text = preserve_pending;
        }
      }
    }

    if self.download_progress_dirty {
      self.download_progress_dirty = false;
      platform::update_settings_window(self.settings_view_model());
    }
  }

  #[track_caller]
  fn clear_overlay_pending(&mut self) {
    if self.debug_stats_enabled && self.overlay_pending_vad_text {
      let loc = std::panic::Location::caller();
      eprintln!(
        "AZAD_OVERLAY_PENDING_CLEAR at {}:{} overlay_visible={} current_turn={:?}",
        loc.file(),
        loc.line(),
        self.overlay_visible,
        self.current_turn_id,
      );
    }
    self.overlay_pending_vad_text = false;
  }

  #[track_caller]
  fn show_overlay_listening(&mut self) {
    self.clear_overlay_pending();
    if !self.overlay_visible {
      if self.debug_stats_enabled {
        let loc = std::panic::Location::caller();
        eprintln!(
          "AZAD_OVERLAY_SHOW kind=listening at {}:{} current_turn={:?} finalizing_turn={:?}",
          loc.file(),
          loc.line(),
          self.current_turn_id,
          self.finalizing_turn_id,
        );
      }
      platform::show_overlay();
      self.overlay_visible = true;
    }
    self.render_listening_overlay();
  }

  fn active_connector_tag(&self) -> &str {
    self.active_connector.as_ref().map(|a| a.tag_label).unwrap_or("")
  }

  fn active_connector_icon(&self) -> &str {
    self.active_connector.as_ref().map(|a| a.tag_icon).unwrap_or("")
  }

  /// `text` with the latched connector's trigger phrase removed, so the matched
  /// lead-in (e.g. "hey claude" / "hey azad") is dropped from the surfaced
  /// transcription and only the connector chip carries the brand. Returns `text`
  /// unchanged when no connector is latched. Applied at the user-facing surfaces
  /// (display, paste, history); `latest_draft` and the finalize state machine keep
  /// the full text.
  fn strip_active_trigger(&self, text: &str) -> String {
    let Some(active) = &self.active_connector else {
      return text.to_string();
    };
    connectors::strip_trigger(text, active.matched_trigger)
  }

  fn stream_display_text(&self, text: &str) -> String {
    let text = self.strip_active_trigger(text);
    self.stream_display_text_without_trigger_strip(&text)
  }

  fn stream_display_text_without_trigger_strip(&self, text: &str) -> String {
    let removed_words =
      effective_removed_words(&self.removed_words, self.remove_hesitations_on_paste);
    build_display_text(
      text,
      DisplayTextOptions {
        removed_words: &removed_words,
        deduplicate_words: self.deduplicate_words_on_paste,
        convert_number_words: self.convert_number_words_on_paste,
        convert_spoken_emoji: self.convert_spoken_emoji_on_paste,
        lowercase_except_uppercase_words: self.lowercase_except_uppercase_words_on_paste,
      },
    )
  }

  fn next_tick_interval(&self, settings_open: bool) -> Duration {
    if self.needs_fast_tick() {
      return FAST_TICK_INTERVAL;
    }
    if self.needs_interactive_tick(settings_open) {
      return INTERACTIVE_TICK_INTERVAL;
    }
    IDLE_TICK_INTERVAL
  }

  fn needs_fast_tick(&self) -> bool {
    let gateway_busy = self
      .gateway_conv
      .as_ref()
      .is_some_and(|c| matches!(c.status, ConvStatus::Thinking | ConvStatus::Streaming));
    let azad_busy = self
      .azad_turn
      .as_ref()
      .is_some_and(|t| matches!(t.status, ConvStatus::Thinking | ConvStatus::Streaming));
    let command_auto_dismiss = self.azad_turn.as_ref().and_then(|t| t.dismiss_at).is_some()
      || self.spotify_turn.as_ref().and_then(|t| t.dismiss_at).is_some();
    self.manual_hold_active
      || self.overlay_visible
      || self.history_browsing
      || self.finalizing_deadline.is_some()
      || self.accessibility_notice_deadline.is_some()
      || self.pending_always_listening_enabled.is_some()
      || self.pending_device_switch_deadline.is_some()
      || gateway_busy
      || azad_busy
      || command_auto_dismiss
  }

  fn needs_interactive_tick(&self, settings_open: bool) -> bool {
    self.pending_onboarding
      || self.onboarding_active
      || settings_open
      || self.pending_first_launch_settings
      || self.download_handle.is_some()
      || self.download_progress_dirty
  }

  #[track_caller]
  /// Body text for the single-lane finalizing overlay. It follows the *live* draft so a
  /// tentative finalize (a brief mid-sentence pause) keeps the caption tracking the speaker
  /// instead of freezing on the `finalizing_draft` snapshot. `finalizing_draft` is only the
  /// commit candidate and the frozen top lane of a genuine two-turn split (handled above).
  fn finalizing_single_lane_body(&self) -> &str {
    &self.latest_draft
  }

  fn render_finalizing_overlay_state(&mut self) {
    if self.accessibility_notice_deadline.is_some() {
      return;
    }
    if !self.overlay_visible {
      if self.debug_stats_enabled {
        let loc = std::panic::Location::caller();
        eprintln!(
          "AZAD_OVERLAY_SHOW kind=finalizing at {}:{} current_turn={:?} finalizing_turn={:?}",
          loc.file(),
          loc.line(),
          self.current_turn_id,
          self.finalizing_turn_id,
        );
      }
      platform::show_overlay();
      self.overlay_visible = true;
    }

    // A live gateway/Azad conversation owns the whole card; the finalize spinner/split
    // lanes are suppressed in favor of the reply.
    if self.gateway_conv.is_some() {
      platform::hide_overlay_top();
      self.render_conversation_overlay();
      return;
    }
    if self.azad_turn.is_some() {
      platform::hide_overlay_top();
      self.render_azad_overlay();
      return;
    }
    if self.spotify_turn.is_some() {
      platform::hide_overlay_top();
      self.render_spotify_overlay();
      return;
    }

    if self.split_overlay_visible() {
      let top_text = self.stream_display_text(&self.finalizing_draft);
      let body_text = self.stream_display_text(&self.latest_draft);
      platform::show_overlay_top();
      platform::set_overlay_top_stream_content(
        &top_text,
        &self.finalizing_activity_history,
        Some(self.busy_border_phase),
      );
      platform::set_overlay_stream_content(
        &body_text,
        &self.activity_history,
        None,
        self.raw_badge_visible(),
        self.hold_badge_visible(),
        "",
        self.active_connector_tag(),
        self.active_connector_icon(),
      );
      return;
    }

    let body_text = self.stream_display_text(self.finalizing_single_lane_body());
    platform::hide_overlay_top();
    platform::set_overlay_stream_content(
      &body_text,
      &self.finalizing_activity_history,
      Some(self.busy_border_phase),
      self.raw_badge_visible(),
      self.hold_badge_visible(),
      "",
      self.active_connector_tag(),
      self.active_connector_icon(),
    );
  }

  fn render_listening_overlay(&self) {
    if self.accessibility_notice_deadline.is_some() {
      return;
    }
    // During a gateway/Azad conversation, keep the prior exchange on screen rather than
    // swapping in the plain draft view.
    if self.gateway_conv.is_some() {
      self.render_conversation_overlay();
      return;
    }
    if self.azad_turn.is_some() {
      self.render_azad_overlay();
      return;
    }
    if self.spotify_turn.is_some() {
      self.render_spotify_overlay();
      return;
    }
    let held_active = self.held_top_overlay_active();
    let live_has_text = !self.latest_draft.trim().is_empty();
    if held_active && live_has_text {
      let top_text = self.stream_display_text_without_trigger_strip(&self.held_top_draft);
      platform::show_overlay_top();
      platform::set_overlay_top_stream_content(&top_text, &self.finalizing_activity_history, None);
    } else {
      platform::hide_overlay_top();
    }
    let body_text = if held_active && !live_has_text {
      self.stream_display_text_without_trigger_strip(&self.held_top_draft)
    } else {
      self.stream_display_text(&self.latest_draft)
    };
    platform::set_overlay_stream_content(
      &body_text,
      &self.activity_history,
      None,
      self.raw_badge_visible(),
      self.hold_badge_visible(),
      "",
      self.active_connector_tag(),
      self.active_connector_icon(),
    );
  }

  /// True when this finalized turn must go to Hey Spotify (no paste, no gateway).
  fn spotify_should_handle_turn(&self) -> bool {
    self.active_connector.as_ref().map(|a| a.id) == Some(connectors::SPOTIFY_CONNECTOR_ID)
  }

  /// True when this finalized turn must go to Hey Azad (no paste, no gateway).
  fn azad_should_handle_turn(&self) -> bool {
    self.active_connector.as_ref().map(|a| a.id) == Some(connectors::AZAD_CONNECTOR_ID)
  }

  /// True when this finalized turn must go to the gateway instead of being pasted —
  /// either a sticky conversation is open, or this is the first "hey claude" turn.
  fn gateway_should_handle_turn(&self) -> bool {
    self.gateway_conv.is_some()
      || self.active_connector.as_ref().map(|a| a.id) == Some(gateway::GATEWAY_AGENT)
  }

  /// Capture must stay on between turns while a conversation is open so follow-up speech
  /// is heard without re-triggering, on top of the normal always-listening / hold rules.
  fn should_keep_capture_for_followups(&self) -> bool {
    should_keep_capture_live(
      self.always_listening_enabled,
      self.manual_hold_active,
      self.gateway_conv.is_some(),
    )
  }

  /// Tag label/icon for a new conversation: the latched connector's, else the built-in
  /// gateway connector's.
  fn gateway_connector_tags(&self) -> (&'static str, &'static str) {
    if let Some(active) = &self.active_connector {
      return (active.tag_label, active.tag_icon);
    }
    self
      .connectors
      .iter()
      .find(|c| c.id == gateway::GATEWAY_AGENT)
      .map(|c| (c.tag_label, c.tag_icon))
      .unwrap_or(("Claude", "claude.svg"))
  }

  /// Warm the WebSocket while the user is still speaking, so it's ready by finalize.
  fn maybe_begin_gateway_connect(&mut self) {
    if self.gateway_conv.is_some() {
      return;
    }
    // Don't downgrade an already-warm socket to `Connecting` — the worker stays alive
    // between turns and won't re-emit `Connected`, which would leave the state stuck.
    if self.gateway_conn == GatewayConnState::Disconnected {
      self.gateway_conn = GatewayConnState::Connecting;
    }
    gateway::ensure_worker();
  }

  /// Submit a finalized turn's query to the gateway: opens a thread on the first turn,
  /// then sends follow-ups in the same thread. Never pastes into the focused app.
  fn submit_to_gateway(&mut self, turn_id: u64, cleaned: &str) {
    let query = self.strip_active_trigger(cleaned).trim().to_string();
    if query.is_empty() {
      // Bare "hey claude" or an empty follow-up: send nothing, keep listening.
      if self.gateway_conv.is_some() {
        self.show_conversation_overlay();
      }
      return;
    }
    gateway::ensure_worker();
    let (tag_label, tag_icon) = self.gateway_connector_tags();
    // A daemon that already announced an incompatible protocol can't service the run; show
    // the error in the user's query bubble instead of sending into a void.
    let protocol_error = self.gateway_protocol_mismatch.clone();
    {
      let conv = self
        .gateway_conv
        .get_or_insert_with(|| GatewayConversation::new(tag_label, tag_icon));
      conv.last_query = query.clone();
      conv.reply.clear();
      conv.error_msg.clear();
      conv.activity_label = None;
      conv.composing_query = None;
      conv.status = ConvStatus::Thinking;
      conv.last_activity = Some(Instant::now());
      conv.acknowledged = false;
      let kind = if conv.thread_id.is_none() { "new_thread" } else { "followup" };
      eprintln!(
        "AZAD_GATEWAY submit turn_id={turn_id} kind={kind} conn={:?} thread_id={:?} chars={} query={:?}",
        self.gateway_conn,
        conv.thread_id,
        query.chars().count(),
        query
      );
      if let Some(message) = protocol_error {
        conv.status = ConvStatus::Error;
        conv.error_msg = message;
        conv.last_activity = None;
      } else {
        // Send unconditionally: the worker's command channel buffers until the socket is
        // connected, so the send never depends on `gateway_conn` (which could be stale, e.g.
        // a warm socket the controller still thinks is "Connecting"). If the connect fails,
        // the buffered command is dropped and a `Disconnected` error surfaces instead.
        let req_id = gateway::make_request_id();
        match conv.thread_id.clone() {
          None => gateway::send_command(GatewayCommand::SendNewThread { req_id, query }),
          Some(thread_id) => {
            gateway::send_command(GatewayCommand::SendFollowup { req_id, thread_id, query })
          }
        };
      }
    }
    // Never paste the query, and stop the SessionEnded fallback from re-pasting it.
    self.last_pasted_turn_id = Some(turn_id);
    self.latest_final = None;
    if let Some(session) = &self.session {
      session.set_capture_enabled(true);
    }
    self.show_conversation_overlay();
  }

  fn show_conversation_overlay(&mut self) {
    if self.gateway_conv.is_none() {
      return;
    }
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    self.render_conversation_overlay();
  }

  /// Auto-dismiss finished Hey Azad / Hey Spotify cards. Claude stays until Esc.
  fn maybe_auto_dismiss_command_overlays(&mut self) {
    let now = Instant::now();
    let mut dismissed = false;
    if self
      .spotify_turn
      .as_ref()
      .and_then(|t| t.dismiss_at)
      .is_some_and(|at| now >= at)
    {
      self.spotify_turn = None;
      dismissed = true;
    }
    if self.azad_turn.as_ref().and_then(|t| t.dismiss_at).is_some_and(|at| now >= at) {
      self.azad_turn = None;
      self.azad_pending_query = None;
      dismissed = true;
    }
    if !dismissed {
      return;
    }
    // Don't hide if another sticky surface still owns the overlay.
    if self.gateway_conv.is_some()
      || self.azad_turn.is_some()
      || self.spotify_turn.is_some()
      || self.manual_hold_active
      || self.history_browsing
      || self.finalizing_deadline.is_some()
    {
      return;
    }
    if self.overlay_visible {
      self.hide_overlay();
    }
  }

  /// Submit a finalized Hey Azad turn: interpret → apply allowlisted intent → overlay.
  /// Never pastes and never opens the Claude gateway.
  fn submit_to_azad(&mut self, turn_id: u64, cleaned: &str) {
    let query = self.strip_active_trigger(cleaned).trim().to_string();
    let (tag_label, tag_icon) = self
      .active_connector
      .as_ref()
      .map(|a| (a.tag_label, a.tag_icon))
      .unwrap_or(("Azad", ""));

    self.refresh_apple_lm_availability(false);
    let avail = self.apple_lm_availability.state;

    // Device ineligible: no settings changes and no Settings CTA.
    if matches!(avail, AvailabilityState::DeviceNotEligible) {
      self.azad_turn = Some(AzadTurn {
        tag_label,
        tag_icon,
        query: query.clone(),
        status: ConvStatus::Error,
        reply: String::new(),
        error_msg: avail.setup_overlay_message().to_string(),
        dismiss_at: Some(Instant::now() + COMMAND_OVERLAY_ERROR_HOLD),
      });
      self.last_pasted_turn_id = Some(turn_id);
      self.latest_final = None;
      self.show_azad_overlay();
      return;
    }

    // Apple Intelligence off / still downloading: show setup, keep pending, still try
    // the closed-catalog heuristic so common phrases work while the model installs.
    if matches!(
      avail,
      AvailabilityState::AppleIntelligenceNotEnabled | AvailabilityState::ModelNotReady
    ) {
      if !query.is_empty() {
        self.azad_pending_query = Some(query.clone());
      }
    }

    self.azad_turn = Some(AzadTurn {
      tag_label,
      tag_icon,
      query: if query.is_empty() { "hey azad".to_string() } else { query.clone() },
      status: ConvStatus::Thinking,
      reply: String::new(),
      error_msg: String::new(),
      dismiss_at: None,
    });
    self.last_pasted_turn_id = Some(turn_id);
    self.latest_final = None;
    self.show_azad_overlay();

    let snapshot = self.text_settings_snapshot();
    let intent = apple_lm::interpret_query(&query, &snapshot);
    eprintln!(
      "AZAD_AZAD_LM event=interpret turn_id={turn_id} avail={} query={:?} intent={:?}",
      avail.as_str(),
      query,
      intent
    );

    // Prefer setup guidance when AI isn't ready and the phrase wasn't recognized.
    if !intent.is_actionable()
      && matches!(
        avail,
        AvailabilityState::AppleIntelligenceNotEnabled | AvailabilityState::ModelNotReady
      )
    {
      if let Some(turn) = self.azad_turn.as_mut() {
        turn.status = ConvStatus::Error;
        turn.error_msg = avail.setup_overlay_message().to_string();
        turn.reply.clear();
        turn.dismiss_at = Some(Instant::now() + COMMAND_OVERLAY_ERROR_HOLD);
      }
      self.show_azad_overlay();
      return;
    }

    self.apply_azad_intent(intent);
    self.show_azad_overlay();
  }

  fn text_settings_snapshot(&self) -> TextSettingsSnapshot {
    TextSettingsSnapshot {
      trailing_space: self.append_trailing_space_on_paste,
      deduplicate_words: self.deduplicate_words_on_paste,
      convert_number_words: self.convert_number_words_on_paste,
      convert_spoken_emoji: self.convert_spoken_emoji_on_paste,
      lowercase_except_uppercase: self.lowercase_except_uppercase_words_on_paste,
      remove_hesitations: self.remove_hesitations_on_paste,
      removed_words: self.removed_words.clone(),
    }
  }

  fn apply_azad_intent(&mut self, intent: AzadIntent) {
    match &intent {
      AzadIntent::SetTextSetting { setting, enabled } => {
        let enabled = *enabled;
        match setting {
          TextSettingId::TrailingSpace => {
            self.handle_settings_toggle_append_trailing_space(enabled);
          }
          TextSettingId::DeduplicateWords => {
            self.handle_settings_toggle_deduplicate_words(enabled);
          }
          TextSettingId::ConvertNumberWords => {
            self.handle_settings_toggle_convert_number_words(enabled);
          }
          TextSettingId::ConvertSpokenEmoji => {
            self.handle_settings_toggle_convert_spoken_emoji(enabled);
          }
          TextSettingId::LowercaseExceptUppercase => {
            self.handle_settings_toggle_lowercase_except_uppercase_words(enabled);
          }
          TextSettingId::RemoveHesitations => {
            self.handle_settings_toggle_remove_hesitations(enabled);
          }
        }
        eprintln!(
          "AZAD_AZAD_LM event=applied kind=set_text_setting setting={:?} enabled={enabled}",
          setting
        );
        if let Some(turn) = self.azad_turn.as_mut() {
          turn.status = ConvStatus::Done;
          turn.reply = intent.confirmation_label();
          turn.error_msg.clear();
          turn.dismiss_at = Some(Instant::now() + COMMAND_OVERLAY_SUCCESS_HOLD);
        }
        self.azad_pending_query = None;
      }
      AzadIntent::ManageRemovedWord { action, word } => {
        match action {
          RemovedWordAction::Add => self.handle_settings_add_removed_word(word.clone()),
          RemovedWordAction::Remove => self.handle_settings_remove_removed_word(word.clone()),
        }
        eprintln!(
          "AZAD_AZAD_LM event=applied kind=manage_removed_word action={action:?} word={word:?}"
        );
        if let Some(turn) = self.azad_turn.as_mut() {
          turn.status = ConvStatus::Done;
          turn.reply = intent.confirmation_label();
          turn.error_msg.clear();
          turn.dismiss_at = Some(Instant::now() + COMMAND_OVERLAY_SUCCESS_HOLD);
        }
        self.azad_pending_query = None;
      }
      AzadIntent::Unsupported { message } => {
        if let Some(turn) = self.azad_turn.as_mut() {
          turn.status = ConvStatus::Error;
          turn.error_msg = message.clone();
          turn.reply.clear();
          turn.dismiss_at = Some(Instant::now() + COMMAND_OVERLAY_ERROR_HOLD);
        }
      }
      AzadIntent::Help => {
        if let Some(turn) = self.azad_turn.as_mut() {
          turn.status = ConvStatus::Done;
          turn.reply = intent.confirmation_label();
          turn.error_msg.clear();
          turn.dismiss_at = Some(Instant::now() + COMMAND_OVERLAY_SUCCESS_HOLD);
        }
      }
    }
  }

  fn show_azad_overlay(&mut self) {
    if self.azad_turn.is_none() {
      return;
    }
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    self.render_azad_overlay();
  }

  /// Submit a finalized Hey Spotify turn: heuristic → Spotify.app control → overlay.
  /// Never pastes and never opens the Claude gateway or Azad tools.
  fn submit_to_spotify(&mut self, turn_id: u64, cleaned: &str) {
    let query = self.strip_active_trigger(cleaned).trim().to_string();
    let (tag_label, tag_icon) = self
      .active_connector
      .as_ref()
      .map(|a| (a.tag_label, a.tag_icon))
      .unwrap_or(("Spotify", ""));

    self.spotify_app_installed = spotify_client::spotify_app_installed();
    if !self.spotify_app_installed {
      self.spotify_turn = Some(SpotifyTurn {
        tag_label,
        tag_icon,
        query: if query.is_empty() { "hey spotify".into() } else { query.clone() },
        status: ConvStatus::Error,
        reply: String::new(),
        error_msg:
          "Spotify is not installed. Install Spotify, then enable this connector in Settings."
            .into(),
        dismiss_at: Some(Instant::now() + COMMAND_OVERLAY_ERROR_HOLD),
      });
      self.last_pasted_turn_id = Some(turn_id);
      self.latest_final = None;
      self.show_spotify_overlay();
      return;
    }

    self.spotify_turn = Some(SpotifyTurn {
      tag_label,
      tag_icon,
      query: if query.is_empty() { "hey spotify".into() } else { query.clone() },
      status: ConvStatus::Thinking,
      reply: String::new(),
      error_msg: String::new(),
      dismiss_at: None,
    });
    self.last_pasted_turn_id = Some(turn_id);
    self.latest_final = None;
    self.show_spotify_overlay();

    let intent = spotify_cmd::interpret_spotify_query(&query);
    eprintln!(
      "AZAD_SPOTIFY event=interpret turn_id={turn_id} query={:?} intent={:?}",
      query, intent
    );
    self.apply_spotify_intent(intent);
    self.show_spotify_overlay();
  }

  fn apply_spotify_intent(&mut self, intent: SpotifyIntent) {
    let result: Result<String, String> = match &intent {
      SpotifyIntent::Play => spotify_client::play()
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::Pause => spotify_client::pause()
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::PlayPause => spotify_client::play_pause()
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::Next => spotify_client::next_track()
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::Previous => spotify_client::previous_track()
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::Like => spotify_client::like_current()
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::Current => spotify_client::current_track().map_err(|e| e.to_string()),
      SpotifyIntent::PlayQuery { query } => {
        spotify_client::play_query(query).map_err(|e| e.to_string())
      }
      SpotifyIntent::PlayArtist { artist } => {
        spotify_client::play_artist_this_is(artist).map_err(|e| e.to_string())
      }
      SpotifyIntent::PlayRadio { query } => {
        spotify_client::play_radio(query.as_deref()).map_err(|e| e.to_string())
      }
      SpotifyIntent::PlayGenre { genre } => {
        spotify_client::play_genre(genre).map_err(|e| e.to_string())
      }
      SpotifyIntent::Search { query } => spotify_client::open_search(query)
        .map(|_| format!("Opened Spotify search for “{query}”"))
        .map_err(|e| e.to_string()),
      SpotifyIntent::VolumeUp => spotify_client::volume_delta(10)
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::VolumeDown => spotify_client::volume_delta(-10)
        .map(|_| intent.confirmation_label())
        .map_err(|e| e.to_string()),
      SpotifyIntent::Identify { play } => {
        // Phase 2: ShazamKit helper. v1: clear message.
        let _ = play;
        Err(
          "Song identify (Shazam) is coming next — try “play <song name>” or pause/next for now."
            .into(),
        )
      }
      SpotifyIntent::Help => Ok(intent.confirmation_label()),
      SpotifyIntent::Unsupported { message } => Err(message.clone()),
    };

    match result {
      Ok(reply) => {
        eprintln!("AZAD_SPOTIFY event=ok reply={reply:?}");
        if let Some(turn) = self.spotify_turn.as_mut() {
          turn.status = ConvStatus::Done;
          turn.reply = reply;
          turn.error_msg.clear();
          turn.dismiss_at = Some(Instant::now() + COMMAND_OVERLAY_SUCCESS_HOLD);
        }
      }
      Err(err) => {
        eprintln!("AZAD_SPOTIFY event=error err={err:?}");
        if let Some(turn) = self.spotify_turn.as_mut() {
          turn.status = ConvStatus::Error;
          turn.error_msg = err;
          turn.reply.clear();
          turn.dismiss_at = Some(Instant::now() + COMMAND_OVERLAY_ERROR_HOLD);
        }
      }
    }
  }

  fn show_spotify_overlay(&mut self) {
    if self.spotify_turn.is_none() {
      return;
    }
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    self.render_spotify_overlay();
  }

  fn render_spotify_overlay(&self) {
    let Some(turn) = self.spotify_turn.as_ref() else {
      return;
    };
    let busy_phase = matches!(turn.status, ConvStatus::Thinking | ConvStatus::Streaming)
      .then_some(self.busy_border_phase);
    let query = self.stream_display_text_without_trigger_strip(&turn.query);
    platform::set_overlay_conversation_content(
      turn.tag_label,
      turn.tag_icon,
      &query,
      &turn.reply,
      turn.status,
      &turn.error_msg,
      &self.activity_history,
      busy_phase,
    );
  }

  fn render_azad_overlay(&self) {
    let Some(turn) = self.azad_turn.as_ref() else {
      return;
    };
    let busy_phase = matches!(turn.status, ConvStatus::Thinking | ConvStatus::Streaming)
      .then_some(self.busy_border_phase);
    let query = self.stream_display_text_without_trigger_strip(&turn.query);
    platform::set_overlay_conversation_content(
      turn.tag_label,
      turn.tag_icon,
      &query,
      &turn.reply,
      turn.status,
      &turn.error_msg,
      &self.activity_history,
      busy_phase,
    );
  }

  fn refresh_apple_lm_availability(&mut self, force: bool) {
    const MIN_PROBE_INTERVAL: Duration = Duration::from_secs(5);
    if !force {
      if let Some(last) = self.apple_lm_last_probe {
        if last.elapsed() < MIN_PROBE_INTERVAL {
          return;
        }
      }
    }
    self.apple_lm_availability = apple_lm::probe_availability();
    self.apple_lm_last_probe = Some(Instant::now());
    eprintln!(
      "AZAD_AZAD_LM event=availability state={} detail={:?}",
      self.apple_lm_availability.state.as_str(),
      self.apple_lm_availability.detail
    );
  }

  fn maybe_resume_pending_azad(&mut self) {
    if !matches!(
      self.apple_lm_availability.state,
      AvailabilityState::Available | AvailabilityState::Unavailable
    ) {
      return;
    }
    let Some(query) = self.azad_pending_query.take() else {
      return;
    };
    // Synthetic turn id 0: already marked last_pasted; just re-run interpret/apply.
    let (tag_label, tag_icon) = ("Azad", "");
    self.azad_turn = Some(AzadTurn {
      tag_label,
      tag_icon,
      query: query.clone(),
      status: ConvStatus::Thinking,
      reply: String::new(),
      error_msg: String::new(),
      dismiss_at: None,
    });
    self.show_azad_overlay();
    let snapshot = self.text_settings_snapshot();
    let intent = apple_lm::interpret_query(&query, &snapshot);
    self.apply_azad_intent(intent);
    self.show_azad_overlay();
  }

  fn render_conversation_overlay(&self) {
    let Some(conv) = self.gateway_conv.as_ref() else {
      return;
    };
    // Chip shows the connector plus the model + effort it's routed to.
    let chip = format!(
      "{} · {} · {}",
      conv.tag_label,
      gateway::GATEWAY_MODEL_ID,
      gateway::GATEWAY_MODEL_EFFORT
    );
    // While the user is speaking a follow-up, show the forming query with an empty reply
    // (no thinking spinner) so it has its own space; the wave strip signals listening.
    let (query, reply, status) = match &conv.composing_query {
      Some(forming) => (forming.as_str(), "", ConvStatus::Done),
      None => (conv.last_query.as_str(), conv.reply.as_str(), conv.status),
    };
    let query = self.stream_display_text_without_trigger_strip(query);
    let busy_phase = matches!(status, ConvStatus::Thinking | ConvStatus::Streaming)
      .then_some(self.busy_border_phase);
    platform::set_overlay_conversation_content(
      &chip,
      conv.tag_icon,
      &query,
      reply,
      status,
      &conv.error_msg,
      &self.activity_history,
      busy_phase,
    );
  }

  fn handle_gateway_event(&mut self, event: GatewayEvent) {
    eprintln!("AZAD_GATEWAY event={}", gateway_event_summary(&event));
    // Any inbound frame is a sign of life; refresh the no-response deadline. A run-level
    // event additionally marks the run acknowledged, switching the timeout from the fast ack
    // window to the longer post-ack stall window.
    let run_acknowledged = matches!(
      event,
      GatewayEvent::RunAccepted { .. }
        | GatewayEvent::Delta { .. }
        | GatewayEvent::Completed { .. }
        | GatewayEvent::Activity { .. }
    );
    if let Some(conv) = self.gateway_conv.as_mut() {
      conv.last_activity = Some(Instant::now());
      if run_acknowledged {
        conv.acknowledged = true;
      }
    }
    match event {
      GatewayEvent::Connected => {
        self.gateway_conn = GatewayConnState::Connected;
        // A fresh socket's protocol verdict is unknown until its `server.ready` arrives.
        self.gateway_protocol_mismatch = None;
      }
      GatewayEvent::ServerReady { protocol, version } => {
        if protocol == gateway::GATEWAY_PROTOCOL {
          self.gateway_protocol_mismatch = None;
        } else {
          let message = format!(
            "Gateway speaks {protocol:?} (need {:?}) — daemon may be out of date. Press Esc.",
            gateway::GATEWAY_PROTOCOL
          );
          eprintln!(
            "AZAD_GATEWAY protocol_mismatch got={protocol:?} want={:?} version={version:?}",
            gateway::GATEWAY_PROTOCOL
          );
          self.gateway_protocol_mismatch = Some(message.clone());
          // Fail any in-flight run immediately rather than waiting for the ack timeout.
          if let Some(conv) = self.gateway_conv.as_mut() {
            if matches!(conv.status, ConvStatus::Thinking | ConvStatus::Streaming) {
              conv.status = ConvStatus::Error;
              conv.error_msg = message;
              conv.last_activity = None;
            }
          }
          self.show_conversation_overlay();
        }
      }
      GatewayEvent::Disconnected { reason } => {
        self.gateway_conn = GatewayConnState::Disconnected;
        if let Some(conv) = self.gateway_conv.as_mut() {
          conv.status = ConvStatus::Error;
          conv.error_msg = friendly_gateway_error(&reason);
        }
        self.show_conversation_overlay();
      }
      GatewayEvent::RunAccepted { thread_id, run_id } => {
        if let Some(conv) = self.gateway_conv.as_mut() {
          conv.thread_id.get_or_insert(thread_id);
          conv.awaiting_run_id = Some(run_id);
          if conv.status != ConvStatus::Streaming {
            conv.status = ConvStatus::Thinking;
          }
        }
        self.show_conversation_overlay();
      }
      GatewayEvent::Delta { thread_id, content, delta, replace } => {
        if let Some(conv) = self.gateway_conv.as_mut() {
          if conv.thread_id.as_deref() == Some(thread_id.as_str()) {
            conv.status = ConvStatus::Streaming;
            conv.activity_label = None;
            gateway::apply_delta(&mut conv.reply, content.as_deref(), delta.as_deref(), replace);
          }
        }
        self.show_conversation_overlay();
      }
      GatewayEvent::Completed { thread_id, content } => {
        if let Some(conv) = self.gateway_conv.as_mut() {
          if conv.thread_id.as_deref() == Some(thread_id.as_str()) {
            if !content.is_empty() {
              conv.reply = content;
            }
            conv.status = ConvStatus::Done;
            conv.activity_label = None;
          }
        }
        self.show_conversation_overlay();
      }
      GatewayEvent::Activity { phase, label } => {
        if let Some(conv) = self.gateway_conv.as_mut() {
          if !matches!(conv.status, ConvStatus::Streaming | ConvStatus::Done) {
            conv.status = ConvStatus::Thinking;
          }
          conv.activity_label = if phase == gateway::ConvPhase::Idle { None } else { label };
        }
        self.show_conversation_overlay();
      }
      GatewayEvent::Failed { error } | GatewayEvent::RequestError { error } => {
        if let Some(conv) = self.gateway_conv.as_mut() {
          conv.status = ConvStatus::Error;
          conv.error_msg = friendly_gateway_error(&error);
        }
        self.show_conversation_overlay();
      }
    }
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
    self.clear_overlay_pending();
    self.raw_finalize_requested = false;
    self.pending_hold_release_raw_requested = false;
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
    let text = self.strip_active_trigger(text);
    let text = text.as_str();
    let removed_words =
      effective_removed_words(&self.removed_words, self.remove_hesitations_on_paste);
    let paste_text = build_paste_text(
      text,
      PasteTextOptions {
        append_trailing_space: self.append_trailing_space_on_paste,
        removed_words: &removed_words,
        deduplicate_words: self.deduplicate_words_on_paste,
        convert_number_words: self.convert_number_words_on_paste,
        convert_spoken_emoji: self.convert_spoken_emoji_on_paste,
        lowercase_except_uppercase_words: self.lowercase_except_uppercase_words_on_paste,
      },
    );

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
        text_preview: preview_text_for_metrics(text, 45),
      }));
    }

    let paste_ok = matches!(paste_result, PasteResult::Pasted);
    self.log_input_event(InputLogEvent::PasteAttempt {
      turn_id,
      mode: match mode {
        TranscriptMode::Normal => "normal",
        TranscriptMode::Raw => "raw",
      },
      source: paste_result_label(paste_result),
      paste_ok,
    });
    paste_ok
  }

  #[track_caller]
  fn hide_overlay(&mut self) {
    // A live gateway/Azad/Spotify conversation owns the overlay until Esc tears it down.
    if self.gateway_conv.is_some() || self.azad_turn.is_some() || self.spotify_turn.is_some() {
      return;
    }
    self.clear_overlay_pending();
    self.clear_held_top_overlay();
    self.listen_toggle_notice = None;
    if self.overlay_visible {
      if self.debug_stats_enabled {
        let loc = std::panic::Location::caller();
        eprintln!("AZAD_OVERLAY_HIDE at {}:{}", loc.file(), loc.line());
      }
      platform::hide_overlay();
      self.overlay_visible = false;
    }
    platform::reset_overlay_conversation_views();
  }

  fn reset_turn_state(&mut self) {
    self.dispatch_hotkey_input(HotkeyInput::TurnReset);
    self.reset_turn_state_preserving_hotkey_state();
  }

  /// Detects/refreshes the connector for the current turn from `latest_draft`.
  /// Latches on the first draft whose leading phrase fully matches an enabled
  /// connector (a partial prefix never matches, so the tag can't flicker off);
  /// once latched, keeps the latched connector and only refreshes its stripped
  /// `clean_query` as the draft grows.
  fn update_active_connector(&mut self) {
    if self.active_connector.is_none() {
      if let Some(m) = connectors::detect(&self.latest_draft, &self.connectors) {
        let id = m.id;
        eprintln!(
          "AZAD_CONNECTOR latched id={id} tag={} trigger={:?} clean_query={:?}",
          m.tag_label, m.matched_trigger, m.clean_query
        );
        self.active_connector = Some(ActiveConnector {
          id,
          tag_label: m.tag_label,
          tag_icon: m.tag_icon,
          matched_trigger: m.matched_trigger,
          clean_query: m.clean_query,
        });
        // Warm the Claude gateway while the user is still speaking so it's ready by finalize.
        // Azad does not use the gateway — chip + strip only (same live overlay path as Claude).
        if id == gateway::GATEWAY_AGENT {
          self.maybe_begin_gateway_connect();
        }
      }
      return;
    }
    // Refresh clean_query with the latched phrase's token count (primary or alias).
    if let Some(active) = self.active_connector.as_mut() {
      active.clean_query = connectors::strip_trigger(&self.latest_draft, active.matched_trigger);
    }
  }

  fn reset_turn_state_preserving_hotkey_state(&mut self) {
    self.cancelled = false;
    self.last_pasted_turn_id = None;
    self.hold_saw_speech = false;
    self.latest_draft.clear();
    self.active_connector = None;
    self.latest_final = None;
    self.finalizing_deadline = None;
    self.finalizing_turn_id = None;
    self.raw_handled_turn_id = None;
    self.raw_finalize_requested = false;
    self.pending_hold_release_raw_requested = false;
    self.deferred_vad_start = false;
    self.accessibility_notice_deadline = None;
    self.listen_toggle_notice = None;
    self.clear_overlay_pending();
    self.clear_held_top_overlay();
    self.current_turn_id = None;
    self.turn_accept_floor = self.latest_seen_turn_id.saturating_add(1);
    self.turn_started_at.clear();
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
    if self.debug_stats_enabled {
      eprintln!(
        "AZAD_RAW_FINALIZE turn_id={} targets_finalizing={} hide_overlay={} disable_capture={} \
         deferred_vad_start={} always_listening={} overlay_visible={} finalizing_turn={:?}",
        turn_id,
        raw_targets_finalizing_lane,
        ui_plan.hide_overlay,
        ui_plan.disable_capture,
        self.deferred_vad_start,
        self.always_listening_enabled,
        self.overlay_visible,
        self.finalizing_turn_id,
      );
    }
    if raw_targets_finalizing_lane {
      self.finalizing_turn_id = None;
      self.finalizing_deadline = None;
      self.finalizing_draft.clear();
    }
    self.raw_handled_turn_id = Some(turn_id);
    self.raw_finalize_requested = false;
    self.pending_hold_release_raw_requested = false;
    self.dispatch_hotkey_input(HotkeyInput::SpeechFinalized);
    self.latest_final = Some(raw_text.clone());

    // Opt+Enter while a connector turn is live submits instead of raw-pasting.
    if self.spotify_should_handle_turn() {
      self.submit_to_spotify(turn_id, &raw_text);
      self.maybe_start_deferred_vad_turn();
      return true;
    }
    if self.azad_should_handle_turn() {
      self.submit_to_azad(turn_id, &raw_text);
      self.maybe_start_deferred_vad_turn();
      return true;
    }
    if self.gateway_should_handle_turn() {
      self.submit_to_gateway(turn_id, &raw_text);
      self.maybe_start_deferred_vad_turn();
      return true;
    }

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

    let DebugStatsEvent::PartialAuditResult {
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
    } = event;
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

  fn accept_turn(&self, turn_id: u64) -> bool {
    turn_id >= self.turn_accept_floor
  }

  fn input_log_snapshot(&self) -> StateSnapshot {
    StateSnapshot {
      finalizing_turn_id: self.finalizing_turn_id,
      current_turn_id: self.current_turn_id,
      latest_seen_turn_id: self.latest_seen_turn_id,
      finalizing_draft_chars: self.finalizing_draft.chars().count(),
      latest_draft_chars: self.latest_draft.chars().count(),
      engine_state: match self.engine_state {
        EngineState::Idle => "idle",
        EngineState::Speech => "speech",
      },
      manual_hold_active: self.manual_hold_active,
      overlay_visible: self.overlay_visible,
      saw_vad_start_during_finalizing: self.saw_vad_start_during_finalizing,
      history_browsing: self.history_browsing,
      last_pasted_turn_id: self.last_pasted_turn_id,
      raw_handled_turn_id: self.raw_handled_turn_id,
    }
  }

  fn log_input_event(&self, event: InputLogEvent) {
    let entry = InputLogEntry {
      schema_version: input_log::schema_version(),
      ts_ms: input_log::now_epoch_ms(),
      event,
      state: self.input_log_snapshot(),
    };
    input_log::append(&entry);
  }

  fn observe_turn(&mut self, turn_id: u64) {
    self.latest_seen_turn_id = self.latest_seen_turn_id.max(turn_id);
    self.current_turn_id = Some(next_current_turn_id(self.current_turn_id, turn_id));
    self.turn_started_at.entry(turn_id).or_insert_with(Instant::now);
  }

  fn update_activity_from_meter(&mut self, peak_db: f32, vad_speech: bool, vad_prob: f32) {
    if self.manual_hold_active && vad_speech {
      self.hold_saw_speech = true;
    }
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

fn should_update_selected_device(
  current_device_id: Option<&str>,
  selected_device_id: &str,
) -> bool {
  current_device_id != Some(selected_device_id)
}

fn should_start_device_controller(
  has_controller: bool,
  microphone_status: platform::PermissionStatus,
) -> bool {
  !has_controller && microphone_status == platform::PermissionStatus::Granted
}

pub(super) fn effective_removed_words(
  custom_words: &[String],
  remove_hesitations: bool,
) -> Vec<String> {
  let mut words = Vec::new();
  if remove_hesitations {
    words.extend(preferred_store::BUILT_IN_HESITATIONS.iter().map(|word| word.to_string()));
  }
  for word in custom_words {
    if !words.iter().any(|existing| existing.eq_ignore_ascii_case(word)) {
      words.push(word.clone());
    }
  }
  words
}

pub(super) fn start_min_rms_db_for_activation_level(value: i64) -> f32 {
  let normalized = (value.clamp(0, 100) as f32) / 100.0;
  ACTIVATION_LEVEL_MIN_RMS_DB
    + normalized * (ACTIVATION_LEVEL_MAX_RMS_DB - ACTIVATION_LEVEL_MIN_RMS_DB)
}

fn effective_run_on_startup_enabled(preference_enabled: bool, launch_agent_exists: bool) -> bool {
  preference_enabled && launch_agent_exists
}

fn onboarding_view_model_changed(
  previous: &Option<platform::OnboardingViewModel>,
  next: &platform::OnboardingViewModel,
) -> bool {
  previous.as_ref() != Some(next)
}

fn should_keep_capture_live(
  always_listening_enabled: bool,
  manual_hold_active: bool,
  gateway_conversation_active: bool,
) -> bool {
  always_listening_enabled || manual_hold_active || gateway_conversation_active
}

#[cfg(test)]
mod tests {
  use std::time::{Duration, Instant};

  use super::policy::{
    DraftOverlayAction, ListenToggleNotice, ManualHoldReleaseAction, ManualHoldReleasePlan,
    RawFinalizeUiPlan, SessionRecoveryState, allow_immediate_restart_for_fault_count,
    draft_matches_finalized_text, draft_update_overlay_action, final_text_has_user_visible_context,
    has_actionable_turn_context_for_snapshot, has_started_turn_for_snapshot,
    has_turn_context_for_snapshot, is_stream_fault_message, listen_toggle_notice,
    manual_hold_release_plan, next_current_turn_id, raw_finalize_target_turn_id_for_state,
    raw_finalize_ui_plan, recovery_state_for_fault_count, should_ignore_finalizing_event,
    should_latch_raw_on_hold_release, split_overlay_active_for_turns,
    split_overlay_visible_for_state, split_overlay_visible_with_hold_for_state,
    split_overlay_visible_with_live_divergence_for_state,
    split_overlay_visible_with_vad_hint_for_state, split_top_completion_for_state,
    turn_started_should_arm_pending,
  };
  use super::{
    AppController, AzadConfig, EngineState, FAST_TICK_INTERVAL, HotkeyEffect, IDLE_TICK_INTERVAL,
    INTERACTIVE_TICK_INTERVAL, LISTEN_TOGGLE_NOTICE_DURATION_MS, ShutdownSnapshotOutcome,
    effective_removed_words, effective_run_on_startup_enabled, onboarding_view_model_changed,
    should_keep_capture_live, should_start_device_controller, should_update_selected_device,
    start_min_rms_db_for_activation_level,
  };
  use crate::speech::{SpeechEvent, SpeechSession};
  use crate::transcript_history::TranscriptIndex;
  use crate::ui_model::{OnboardingViewModel, UiModelPack, UiModelStatus, UiPermissionStatus};

  #[test]
  fn idle_tick_interval_slows_when_listen_is_off_and_no_ui_work_is_active() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.always_listening_enabled = false;
    controller.manual_hold_active = false;
    controller.overlay_visible = false;
    controller.history_browsing = false;
    controller.finalizing_deadline = None;
    controller.accessibility_notice_deadline = None;
    controller.pending_always_listening_enabled = None;
    controller.pending_device_switch_deadline = None;
    controller.onboarding_active = false;
    controller.pending_onboarding = false;
    controller.pending_first_launch_settings = false;
    controller.download_progress_dirty = false;

    assert_eq!(controller.next_tick_interval(false), IDLE_TICK_INTERVAL);
  }

  #[test]
  fn tick_interval_stays_fast_for_visible_or_time_sensitive_work() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.always_listening_enabled = false;
    controller.overlay_visible = true;
    assert_eq!(controller.next_tick_interval(false), FAST_TICK_INTERVAL);

    controller.overlay_visible = false;
    controller.manual_hold_active = true;
    assert_eq!(controller.next_tick_interval(false), FAST_TICK_INTERVAL);

    controller.manual_hold_active = false;
    controller.pending_device_switch_deadline = Some(Instant::now());
    assert_eq!(controller.next_tick_interval(false), FAST_TICK_INTERVAL);
  }

  #[test]
  fn tick_interval_uses_interactive_cadence_for_setup_surfaces() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.always_listening_enabled = false;
    controller.onboarding_active = true;
    assert_eq!(controller.next_tick_interval(false), INTERACTIVE_TICK_INTERVAL);

    controller.onboarding_active = false;
    assert_eq!(controller.next_tick_interval(true), INTERACTIVE_TICK_INTERVAL);
  }

  #[test]
  fn onboarding_blocks_hotkey_overlay_and_state_changes() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.onboarding_complete = false;
    controller.onboarding_active = true;
    controller.models_ready = false;
    controller.overlay_visible = false;
    controller.manual_hold_active = false;
    controller.always_listening_enabled = false;
    controller.raw_finalize_requested = false;

    controller.handle_hotkey_pressed();
    controller.handle_hotkey_released(true);
    controller.handle_finalize_hotkey_pressed(true);
    controller.handle_menu_toggle_always_listening();

    assert!(!controller.overlay_visible);
    assert!(!controller.manual_hold_active);
    assert!(!controller.always_listening_enabled);
    assert!(!controller.raw_finalize_requested);
  }

  #[test]
  fn capture_live_policy_stays_off_for_listen_off_idle_startup() {
    assert!(!should_keep_capture_live(false, false, false));
  }

  #[test]
  fn capture_live_policy_turns_on_for_listen_hold_or_gateway() {
    assert!(should_keep_capture_live(true, false, false));
    assert!(should_keep_capture_live(false, true, false));
    assert!(should_keep_capture_live(false, false, true));
  }

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
  fn menu_device_selection_skips_current_device() {
    assert!(!should_update_selected_device(Some("mic-a"), "mic-a"));
  }

  #[test]
  fn menu_device_selection_updates_different_device() {
    assert!(should_update_selected_device(Some("mic-a"), "mic-b"));
  }

  #[test]
  fn menu_device_selection_updates_when_current_device_unknown() {
    assert!(should_update_selected_device(None, "mic-a"));
  }

  #[test]
  fn device_controller_start_is_gated_by_microphone_permission() {
    assert!(should_start_device_controller(false, crate::platform::PermissionStatus::Granted));
    assert!(!should_start_device_controller(false, crate::platform::PermissionStatus::Denied));
    assert!(!should_start_device_controller(
      false,
      crate::platform::PermissionStatus::NotDetermined
    ));
    assert!(!should_start_device_controller(true, crate::platform::PermissionStatus::Granted));
  }

  #[test]
  fn startup_preference_is_effective_only_when_launch_agent_exists() {
    assert!(effective_run_on_startup_enabled(true, true));
    assert!(!effective_run_on_startup_enabled(true, false));
    assert!(!effective_run_on_startup_enabled(false, true));
    assert!(!effective_run_on_startup_enabled(false, false));
  }

  #[test]
  fn effective_removed_words_adds_built_in_hesitations_when_enabled() {
    let custom = vec!["custom".to_string(), "UM".to_string()];
    let words = effective_removed_words(&custom, true);
    assert!(words.iter().any(|word| word == "um"));
    assert!(words.iter().any(|word| word == "uh"));
    assert!(words.iter().any(|word| word == "custom"));
    assert_eq!(words.iter().filter(|word| word.eq_ignore_ascii_case("um")).count(), 1);
  }

  #[test]
  fn effective_removed_words_uses_only_custom_words_when_hesitations_disabled() {
    let custom = vec!["custom".to_string()];
    assert_eq!(effective_removed_words(&custom, false), custom);
  }

  #[test]
  fn stream_display_text_applies_transforms_without_mutating_raw_draft() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.latest_draft =
      "Hey Claude um The the API has Twenty One happy emoji tabs".to_string();
    controller.removed_words.clear();
    controller.remove_hesitations_on_paste = true;
    controller.deduplicate_words_on_paste = true;
    controller.convert_number_words_on_paste = true;
    controller.convert_spoken_emoji_on_paste = true;
    controller.lowercase_except_uppercase_words_on_paste = true;
    controller.append_trailing_space_on_paste = true;

    controller.update_active_connector();
    let display_text = controller.stream_display_text(&controller.latest_draft);

    assert_eq!(display_text, "the API has 21 😊 tabs");
    assert_eq!(
      controller.latest_draft,
      "Hey Claude um The the API has Twenty One happy emoji tabs"
    );
    assert_eq!(controller.active_connector.as_ref().map(|c| c.id), Some("claude"));
  }

  #[test]
  fn stream_display_text_can_skip_trigger_stripping_for_prestripped_queries() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.removed_words = vec!["actually".to_string()];
    controller.remove_hesitations_on_paste = true;
    controller.deduplicate_words_on_paste = true;
    controller.convert_number_words_on_paste = true;
    controller.convert_spoken_emoji_on_paste = true;
    controller.lowercase_except_uppercase_words_on_paste = true;
    controller.append_trailing_space_on_paste = true;

    let display_text = controller.stream_display_text_without_trigger_strip(
      "actually NASA has Twenty One API calls thumbs up emoji",
    );

    assert_eq!(display_text, "NASA has 21 API calls 👍");
  }

  #[test]
  fn activation_level_maps_to_start_rms_gate_range() {
    assert!((start_min_rms_db_for_activation_level(0) - -60.0).abs() < f32::EPSILON);
    assert!((start_min_rms_db_for_activation_level(100) - -20.0).abs() < f32::EPSILON);
    assert_eq!(start_min_rms_db_for_activation_level(-1), start_min_rms_db_for_activation_level(0));
    assert_eq!(
      start_min_rms_db_for_activation_level(101),
      start_min_rms_db_for_activation_level(100)
    );
  }

  fn shutdown_test_controller() -> AppController {
    let mut controller = AppController::new(AzadConfig::default());
    controller.transcript_index = Some(TranscriptIndex::in_memory_for_tests());
    controller.history_enabled = true;
    controller.cancelled = false;
    controller.overlay_visible = true;
    controller.engine_state = EngineState::Speech;
    controller.current_turn_id = Some(42);
    controller.latest_seen_turn_id = 42;
    controller.latest_draft = "Keep This Visible Draft".to_string();
    controller
  }

  fn draft_event(turn_id: u64, text: &str) -> SpeechEvent {
    SpeechEvent::DraftUpdated {
      session_id: 7,
      turn_id,
      committed: text.to_string(),
      live: String::new(),
    }
  }

  #[test]
  fn finalizing_single_lane_tracks_live_draft_not_frozen_snapshot() {
    // Focused guard on the render-source decision: during a tentative finalize the
    // single-lane finalizing body must follow the *live* draft, not the frozen
    // `finalizing_draft` snapshot. (Pre-fix this returned `finalizing_draft`.)
    let mut controller = AppController::new(AzadConfig::default());
    controller.overlay_visible = true;
    controller.engine_state = EngineState::Speech;
    controller.current_turn_id = Some(7);
    controller.latest_seen_turn_id = 7;
    controller.finalizing_turn_id = Some(7);
    controller.finalizing_draft = "hello".to_string();
    controller.latest_draft = "hello world".to_string();
    controller.finalizing_deadline =
      Some(std::time::Instant::now() + std::time::Duration::from_millis(3000));

    assert_eq!(controller.finalizing_single_lane_body(), "hello world");
    assert_ne!(controller.finalizing_single_lane_body(), controller.finalizing_draft);
  }

  #[test]
  fn finalizing_caption_replays_live_speech_through_a_midsentence_pause() {
    // Deterministic replay of the render events one turn produces when the speaker pauses
    // briefly mid-sentence: drafts grow, the engine emits a tentative `Finalizing` snapshot
    // at the pause, then the speaker resumes and drafts keep growing in the SAME turn.
    // Drives the real `handle_speech_event` routing (DraftUpdated -> Finalizing ->
    // DraftUpdated -> render_finalizing_overlay_state) and asserts the overlay body keeps
    // tracking live speech. Pre-fix the single-lane finalizing branch rendered the frozen
    // `finalizing_draft`, so the terminal body would be "the quick brown" and this fails.
    let _ = crate::platform::test_take_overlay_bodies(); // clear any prior-test residue on this thread
    let mut controller = AppController::new(AzadConfig::default());
    controller.session = Some(SpeechSession::test(7));
    controller.turn_accept_floor = 0;
    controller.overlay_visible = true;
    controller.engine_state = EngineState::Speech;

    controller.handle_speech_event(draft_event(1, "the quick"));
    controller.handle_speech_event(draft_event(1, "the quick brown"));
    controller.handle_speech_event(SpeechEvent::Finalizing {
      session_id: 7,
      turn_id: 1,
      current_draft: "the quick brown".to_string(),
    });
    // Speaker resumes — same turn, still inside the finalizing window.
    controller.handle_speech_event(draft_event(1, "the quick brown fox"));
    controller.handle_speech_event(draft_event(1, "the quick brown fox jumps"));

    assert!(controller.finalizing_deadline.is_some(), "still within the finalizing window");
    let bodies = crate::platform::test_take_overlay_bodies();
    let last = bodies.last().expect("the overlay rendered at least once");
    assert_eq!(
      last,
      &controller.stream_display_text("the quick brown fox jumps"),
      "single-lane finalizing caption must track live speech; got render sequence {bodies:?}"
    );
    assert_ne!(
      last,
      &controller.stream_display_text("the quick brown"),
      "caption froze on the tentative-finalize snapshot"
    );
  }

  #[test]
  fn shutdown_snapshot_saves_current_visible_draft_to_history() {
    let mut controller = shutdown_test_controller();

    let outcome = controller.record_shutdown_snapshot();

    assert_eq!(outcome, ShutdownSnapshotOutcome::Saved { turn_id: 42, char_count: 23 });
    let hits = controller.transcript_index.as_ref().unwrap().search("", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].final_text, "Keep This Visible Draft");
  }

  #[test]
  fn shutdown_snapshot_skips_when_history_is_disabled() {
    let mut controller = shutdown_test_controller();
    controller.history_enabled = false;

    let outcome = controller.record_shutdown_snapshot();

    assert_eq!(outcome, ShutdownSnapshotOutcome::HistoryDisabled);
    assert_eq!(controller.transcript_index.as_ref().unwrap().entry_count(), 0);
  }

  #[test]
  fn shutdown_snapshot_skips_already_handled_turn() {
    let mut controller = shutdown_test_controller();
    controller.last_pasted_turn_id = Some(42);

    let outcome = controller.record_shutdown_snapshot();

    assert_eq!(outcome, ShutdownSnapshotOutcome::AlreadyHandled);
    assert_eq!(controller.transcript_index.as_ref().unwrap().entry_count(), 0);
  }

  #[test]
  fn shutdown_snapshot_uses_finalizing_draft_when_no_live_draft_exists() {
    let mut controller = shutdown_test_controller();
    controller.latest_draft.clear();
    controller.current_turn_id = None;
    controller.engine_state = EngineState::Idle;
    controller.finalizing_turn_id = Some(41);
    controller.finalizing_draft = "finalizing draft only".to_string();

    let outcome = controller.record_shutdown_snapshot();

    assert_eq!(outcome, ShutdownSnapshotOutcome::Saved { turn_id: 41, char_count: 21 });
    let hits = controller.transcript_index.as_ref().unwrap().search("", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].final_text, "finalizing draft only");
  }

  #[test]
  fn shutdown_snapshot_does_not_apply_paste_text_transformations() {
    let mut controller = shutdown_test_controller();
    controller.latest_draft = "NO NO I Said Twenty Three, UM.".to_string();
    controller.append_trailing_space_on_paste = true;
    controller.deduplicate_words_on_paste = true;
    controller.convert_number_words_on_paste = true;
    controller.convert_spoken_emoji_on_paste = true;
    controller.lowercase_except_uppercase_words_on_paste = true;
    controller.remove_hesitations_on_paste = true;

    let outcome = controller.record_shutdown_snapshot();

    assert_eq!(outcome, ShutdownSnapshotOutcome::Saved { turn_id: 42, char_count: 30 });
    let hits = controller.transcript_index.as_ref().unwrap().search("", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].final_text, "NO NO I Said Twenty Three, UM.");
  }

  fn test_onboarding_model() -> OnboardingViewModel {
    OnboardingViewModel {
      accessibility_status: UiPermissionStatus::Granted,
      microphone_status: UiPermissionStatus::Granted,
      model: UiModelPack {
        id: "nemotron-3.5-mlx-bf16-v1".to_string(),
        welcome_name: "Nemotron-3.5 ASR Streaming".to_string(),
        settings_name: "NVIDIA Nemotron-3.5 ASR Streaming 0.6B".to_string(),
        page_url: "https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b"
          .to_string(),
        description: "On-device streaming speech-to-text".to_string(),
        size_label: "1.2 GB".to_string(),
        status: UiModelStatus::Ready,
        download_paused: false,
        progress_pct: 100,
        bytes_done_label: "1.2 GB".to_string(),
        bytes_total_label: "1.2 GB".to_string(),
        error_message: String::new(),
      },
      get_started_enabled: true,
      listen_modifiers: 4,
    }
  }

  #[test]
  fn onboarding_view_model_changed_when_no_previous_model() {
    let model = test_onboarding_model();
    assert!(onboarding_view_model_changed(&None, &model));
  }

  #[test]
  fn onboarding_view_model_unchanged_skips_render() {
    let model = test_onboarding_model();
    assert!(!onboarding_view_model_changed(&Some(model.clone()), &model));
  }

  #[test]
  fn onboarding_view_model_permission_change_triggers_render() {
    let previous = test_onboarding_model();
    let mut next = previous.clone();
    next.microphone_status = UiPermissionStatus::Denied;
    assert!(onboarding_view_model_changed(&Some(previous), &next));
  }

  #[test]
  fn manual_hold_release_plan_keeps_capture_through_finalize_when_listen_is_off() {
    let plan = manual_hold_release_plan(false, true, true);
    assert_eq!(
      plan,
      ManualHoldReleasePlan {
        capture_enabled: true,
        action: ManualHoldReleaseAction::FinalizeTurn,
      }
    );
  }

  #[test]
  fn manual_hold_release_plan_disables_capture_when_nothing_started_and_listen_off() {
    let plan = manual_hold_release_plan(false, true, false);
    assert_eq!(
      plan,
      ManualHoldReleasePlan {
        capture_enabled: false,
        action: ManualHoldReleaseAction::HideOverlay,
      }
    );
  }

  #[test]
  fn manual_hold_release_plan_keeps_capture_when_listen_is_on() {
    let plan = manual_hold_release_plan(true, true, false);
    assert_eq!(
      plan,
      ManualHoldReleasePlan { capture_enabled: true, action: ManualHoldReleaseAction::HideOverlay }
    );
  }

  #[test]
  fn manual_hold_release_plan_keeps_live_when_not_finalizing() {
    let plan = manual_hold_release_plan(false, false, true);
    assert_eq!(
      plan,
      ManualHoldReleasePlan { capture_enabled: false, action: ManualHoldReleaseAction::KeepLive }
    );
  }

  #[test]
  fn manual_hold_meter_vad_marks_short_hold_as_started() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.manual_hold_active = true;
    controller.hold_saw_speech = false;

    controller.update_activity_from_meter(-45.0, true, 0.50);

    assert!(controller.hold_saw_speech);
  }

  #[test]
  fn idle_meter_vad_does_not_mark_hold_as_started() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.manual_hold_active = false;
    controller.hold_saw_speech = false;

    controller.update_activity_from_meter(-45.0, true, 0.50);

    assert!(!controller.hold_saw_speech);
  }

  #[test]
  fn raw_hold_release_latches_only_when_release_finalizes() {
    assert!(should_latch_raw_on_hold_release(true, ManualHoldReleaseAction::FinalizeTurn));
    assert!(!should_latch_raw_on_hold_release(true, ManualHoldReleaseAction::KeepLive));
    assert!(!should_latch_raw_on_hold_release(true, ManualHoldReleaseAction::HideOverlay));
    assert!(!should_latch_raw_on_hold_release(false, ManualHoldReleaseAction::FinalizeTurn));
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
      "previous text",
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
      "previous text",
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
      "previous text",
    ));
  }

  #[test]
  fn enter_falls_back_to_raw_paste_when_engine_is_idle_with_pending_finalize() {
    // Reproduces the user-reported bug from 2026-04-29: after pressing opt+space
    // during a finalizing turn (intent: "pause to think before pasting"), the
    // engine's finalize loop gets disrupted and never emits FinalText for the
    // in-flight turn. The finalizing_draft sits in the overlay, but pressing Enter
    // dispatches FinalizePressed → FinalizeFromHotkey → session.finalize_current_turn()
    // which is a no-op because the engine has nothing fresh to finalize. Only
    // Opt+Enter (raw) pastes — because raw bypasses the engine via
    // `try_finalize_with_raw_text` and uses `finalizing_draft` directly.
    //
    // After the fix, plain Enter also falls through to the raw path when the
    // engine is `Idle` with a non-empty finalizing_draft.
    let mut controller = AppController::new(AzadConfig::default());
    controller.onboarding_complete = true;
    controller.models_ready = true;
    controller.overlay_visible = true;
    controller.engine_state = EngineState::Idle;
    controller.manual_hold_active = false;
    controller.current_turn_id = Some(7);
    controller.finalizing_turn_id = Some(7);
    controller.finalizing_draft = "the long utterance".to_string();
    controller.latest_draft = "the long utterance".to_string();
    controller.finalizing_deadline = Some(Instant::now() + Duration::from_secs(60));
    controller.saw_vad_start_during_finalizing = true;
    controller.latest_seen_turn_id = 7;

    // Sanity: pre-fix this state was the "stuck" shape — overlay actionable, but
    // dispatching FinalizePressed reaches no engine and no paste fires.
    assert!(controller.actionable_overlay_visible());

    controller.handle_finalize_hotkey_pressed(false);

    // The fallback fires: raw-finalize clears the finalizing lane and captures
    // the in-flight text into latest_final, even with no session attached.
    assert_eq!(
      controller.latest_final.as_deref(),
      Some("the long utterance"),
      "raw fallback must capture the finalizing draft as latest_final",
    );
    assert_eq!(
      controller.finalizing_turn_id, None,
      "raw fallback must clear the stuck finalizing lane",
    );
    assert_eq!(
      controller.raw_handled_turn_id,
      Some(7),
      "raw fallback must mark the turn as raw-handled",
    );
    assert!(
      controller.finalizing_draft.is_empty(),
      "raw fallback must clear finalizing_draft (turn cleaned up)",
    );
  }

  #[test]
  fn enter_does_not_fall_back_when_engine_is_active() {
    // Pin the gate: if the engine is in Speech state, the engine's own finalize
    // loop is expected to deliver FinalText. The fallback must NOT preempt that —
    // otherwise plain Enter would always race the engine and produce raw output
    // instead of the polished full-pass text.
    let mut controller = AppController::new(AzadConfig::default());
    controller.onboarding_complete = true;
    controller.models_ready = true;
    controller.overlay_visible = true;
    controller.engine_state = EngineState::Speech;
    controller.current_turn_id = Some(7);
    controller.finalizing_turn_id = Some(7);
    controller.finalizing_draft = "the long utterance".to_string();
    controller.latest_draft = "the long utterance".to_string();
    controller.finalizing_deadline = Some(Instant::now() + Duration::from_secs(60));
    controller.latest_seen_turn_id = 7;

    controller.handle_finalize_hotkey_pressed(false);

    // No raw fallback fired — the engine is responsible for delivering FinalText.
    assert!(
      controller.latest_final.is_none(),
      "raw fallback must not preempt the engine when it is active",
    );
    assert_eq!(
      controller.finalizing_turn_id,
      Some(7),
      "finalizing turn stays in flight; engine will deliver FinalText",
    );
    assert!(controller.raw_handled_turn_id.is_none());
  }

  #[test]
  fn enter_does_not_fall_back_when_finalizing_draft_is_empty() {
    // Pin the gate: no text to fall back on means no fallback. Without this,
    // pressing Enter during pre-speech setup (no draft yet) could leak into the
    // raw path and produce an empty paste.
    let mut controller = AppController::new(AzadConfig::default());
    controller.onboarding_complete = true;
    controller.models_ready = true;
    controller.overlay_visible = true;
    controller.engine_state = EngineState::Idle;
    controller.current_turn_id = Some(7);
    controller.finalizing_turn_id = Some(7);
    controller.finalizing_draft.clear();
    controller.latest_draft.clear();
    controller.latest_seen_turn_id = 7;

    controller.handle_finalize_hotkey_pressed(false);

    assert!(controller.latest_final.is_none());
    assert_eq!(controller.finalizing_turn_id, Some(7));
  }

  #[test]
  fn split_overlay_vad_hint_collapses_when_drafts_match() {
    // Bug case: post opt+space-during-finalize, the engine re-emitted Finalizing for
    // a new turn id but with the SAME content as the prior finalizing_draft. With the
    // VAD-hint branch firing on `saw_vad_start_during_finalizing`, the renderer was
    // showing two overlays with identical text (top busy + bottom idle). Filter the
    // hint-only path on draft divergence so a duplicate finalized text never produces
    // a phantom split lane.
    assert!(!split_overlay_visible_with_vad_hint_for_state(
      Some(5),
      Some(5),
      "the long utterance",
      false,
      true,
      "the long utterance",
    ));
    // Real divergence still surfaces split mode.
    assert!(split_overlay_visible_with_vad_hint_for_state(
      Some(5),
      Some(5),
      "next thought begins",
      false,
      true,
      "previous finalized sentence",
    ));
  }

  #[test]
  fn split_overlay_live_divergence_shows_when_turn_id_lags() {
    assert!(split_overlay_visible_with_live_divergence_for_state(
      Some(5),
      Some(6),
      "new sentence starts in next thought",
      "previous finalized sentence is done",
    ));
  }

  #[test]
  fn split_overlay_live_divergence_ignores_same_lane_rewrites() {
    assert!(!split_overlay_visible_with_live_divergence_for_state(
      Some(5),
      Some(5),
      "this is still the same lane text",
      "this is still the same lane text with punctuation",
    ));
  }

  #[test]
  fn split_overlay_live_divergence_ignores_same_turn_refinement() {
    assert!(!split_overlay_visible_with_live_divergence_for_state(
      Some(7),
      Some(7),
      "same turn final text gained better punctuation",
      "same turn final text gained better punctuation and casing",
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
      Some(11),
      "brand new sentence in next lane",
      false,
      false,
      10,
      "previous lane text that just finished",
    ) || split_overlay_visible_with_live_divergence_for_state(
      Some(10),
      Some(11),
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
  fn final_text_without_visible_turn_context_is_not_pasteable() {
    assert!(!final_text_has_user_visible_context(
      42,
      Some(42),
      None,
      /* overlay_visible */ false,
      /* manual_hold_active */ false,
      "",
      "",
    ));
  }

  #[test]
  fn final_text_with_live_draft_for_same_turn_is_pasteable() {
    assert!(final_text_has_user_visible_context(
      42,
      Some(42),
      None,
      /* overlay_visible */ false,
      /* manual_hold_active */ false,
      "hello",
      "",
    ));
  }

  #[test]
  fn final_text_ignores_stale_draft_from_prior_turn() {
    assert!(!final_text_has_user_visible_context(
      42,
      Some(41),
      None,
      /* overlay_visible */ false,
      /* manual_hold_active */ false,
      "prior text",
      "",
    ));
  }

  #[test]
  fn final_text_with_finalizing_draft_for_same_turn_is_pasteable() {
    assert!(final_text_has_user_visible_context(
      42,
      Some(42),
      Some(42),
      /* overlay_visible */ false,
      /* manual_hold_active */ false,
      "",
      "visible finalizing text",
    ));
  }

  #[test]
  fn final_text_with_overlay_or_manual_hold_context_is_pasteable() {
    assert!(final_text_has_user_visible_context(
      42, None, None, /* overlay_visible */ true, /* manual_hold_active */ false, "", "",
    ));
    assert!(final_text_has_user_visible_context(
      42, None, None, /* overlay_visible */ false, /* manual_hold_active */ true, "", "",
    ));
  }

  #[test]
  fn hidden_final_text_does_not_become_session_fallback_candidate() {
    let mut controller = AppController::new(AzadConfig::default());
    controller.session = Some(SpeechSession::test(7));
    controller.current_turn_id = Some(42);
    controller.latest_seen_turn_id = 42;
    controller.latest_draft.clear();
    controller.finalizing_draft.clear();
    controller.overlay_visible = false;
    controller.manual_hold_active = false;
    controller.latest_final = Some("stale prior text".to_string());

    controller.handle_speech_event(SpeechEvent::FinalText {
      session_id: 7,
      turn_id: 42,
      text: "Hmm.".to_string(),
    });

    assert!(controller.latest_final.is_none());
    assert_eq!(controller.last_pasted_turn_id, None);

    controller.handle_speech_event(SpeechEvent::SessionEnded { session_id: 7 });

    assert_eq!(controller.last_pasted_turn_id, None);
    assert!(controller.latest_final.is_none());
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
      /* cancel_suppression_active */ false, /* eligible_to_show */ false,
    );
    assert_eq!(action, DraftOverlayAction::Show);
  }

  #[test]
  fn draft_overlay_shows_when_eligible_even_without_pending() {
    // Self-heal: the pending latch was dropped (e.g. notice teardown / turn reset
    // cleared it) but we have real transcribed text for a turn we're legitimately
    // capturing. The overlay must still come up. This is the arm that closes the
    // recurring "no overlay during streaming" class of bug independent of which
    // clear-site dropped the latch.
    let action = draft_update_overlay_action(
      /* pending */ false, /* overlay_visible */ false,
      /* cancel_suppression_active */ false, /* eligible_to_show */ true,
    );
    assert_eq!(action, DraftOverlayAction::Show);
  }

  #[test]
  fn draft_overlay_eligible_does_not_override_cancel_suppression() {
    // Escape-then-talk must still suppress: eligibility does not punch through the
    // post-cancel suppression window.
    let action = draft_update_overlay_action(
      /* pending */ false, /* overlay_visible */ false,
      /* cancel_suppression_active */ true, /* eligible_to_show */ true,
    );
    assert_eq!(action, DraftOverlayAction::KeepPendingForLater);
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
      /* cancel_suppression_active */ true, /* eligible_to_show */ false,
    );
    assert_eq!(action, DraftOverlayAction::KeepPendingForLater);
  }

  #[test]
  fn draft_overlay_clears_when_already_visible_and_unsuppressed() {
    // Overlay is up; nothing to show. The pending flag should not linger.
    let action = draft_update_overlay_action(
      /* pending */ true, /* overlay_visible */ true,
      /* cancel_suppression_active */ false, /* eligible_to_show */ true,
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
      /* cancel_suppression_active */ true, /* eligible_to_show */ false,
    );
    assert_eq!(action, DraftOverlayAction::KeepPendingForLater);
  }

  #[test]
  fn draft_overlay_clears_when_not_pending_not_eligible_and_unsuppressed() {
    for visible in [false, true] {
      let action = draft_update_overlay_action(
        /* pending */ false, visible, /* cancel_suppression_active */ false,
        /* eligible_to_show */ false,
      );
      assert_eq!(action, DraftOverlayAction::Clear, "visible={visible}");
    }
  }

  #[test]
  fn notice_expiry_preserves_armed_pending_for_incoming_turn() {
    // Regression for the recurring "no overlay during streaming, flash at the
    // end" bug (turn 958): enabling always-listening force-starts a
    // `ManualOverride` turn while the "Listen ENABLED" notice holds the
    // overlay. The notice expires ~600 ms in — just before that turn's first
    // live draft. The notice-expiry tick used to call `hide_overlay()`, which
    // cleared `overlay_pending_vad_text`, so the first draft hit
    // `draft_update_overlay_action(pending=false, visible=false) == Clear` and
    // the overlay never came up. The fix preserves the armed flag across the
    // teardown so the next non-empty draft still resolves to `Show`.
    let mut controller = AppController::new(AzadConfig::default());
    controller.history_browsing = false;
    controller.manual_hold_active = false;
    controller.finalizing_deadline = None;
    controller.finalizing_turn_id = None;
    controller.overlay_visible = true;
    controller.overlay_pending_vad_text = true;
    controller.listen_toggle_notice = Some(ListenToggleNotice {
      enabled: true,
      started_at: Instant::now() - Duration::from_secs(1),
      duration: Duration::from_millis(LISTEN_TOGGLE_NOTICE_DURATION_MS),
    });
    controller.accessibility_notice_deadline = Some(Instant::now() - Duration::from_millis(1));

    controller.on_tick();

    assert!(controller.listen_toggle_notice.is_none(), "notice should expire");
    assert!(controller.accessibility_notice_deadline.is_none(), "deadline should clear");
    assert!(!controller.overlay_visible, "overlay hidden when notice tears down");
    assert!(
      controller.overlay_pending_vad_text,
      "armed pending must survive notice teardown so the next draft shows the overlay"
    );
    // Confirm the surviving flag actually drives a Show on the next draft.
    assert_eq!(
      draft_update_overlay_action(
        controller.overlay_pending_vad_text,
        controller.overlay_visible,
        /* cancel_suppression_active */ false,
        /* eligible_to_show */ false,
      ),
      DraftOverlayAction::Show,
    );
  }

  #[test]
  fn turn_started_arms_pending_for_manual_when_overlay_hidden() {
    // Reproduces the turn-9 desync: engine fires TurnStarted{Manual} but the
    // hotkey effect never ran on the renderer side, so overlay_visible=false.
    // Predicate must return true so the next DraftUpdated brings the overlay up.
    assert!(turn_started_should_arm_pending(asr::render::TurnStartedReason::Manual, false));
  }

  #[test]
  fn turn_started_no_op_for_manual_when_overlay_already_visible() {
    // Normal manual-hold happy path: HotkeyEffect::ActivateManualHold opened
    // the overlay synchronously before the engine event arrived. Predicate
    // must return false so we don't disturb that flow.
    assert!(!turn_started_should_arm_pending(asr::render::TurnStartedReason::Manual, true));
  }

  #[test]
  fn turn_started_no_op_for_vad_regardless_of_overlay_state() {
    // The VAD path is fully handled by `SpeechStartedByVad`, which has its
    // own side-effect set (reset_turn_state, hide_overlay, latest_draft
    // clear). Reusing this defensive branch for Vad would either double-fire
    // those effects or produce flicker. Always false for Vad.
    for visible in [false, true] {
      assert!(
        !turn_started_should_arm_pending(asr::render::TurnStartedReason::Vad, visible),
        "vad path should be a no-op here regardless of overlay_visible={visible}"
      );
    }
  }

  #[test]
  fn draft_overlay_holds_state_during_suppression_when_not_pending() {
    for visible in [false, true] {
      let action = draft_update_overlay_action(
        /* pending */ false, visible, /* cancel_suppression_active */ true,
        /* eligible_to_show */ false,
      );
      assert_eq!(action, DraftOverlayAction::KeepPendingForLater, "visible={visible}");
    }
  }
}
