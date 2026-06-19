use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use asr::devices::DeviceStateSnapshot;
use asr::pipeline::{DebugStatsEvent, EngineState};

use crate::config::AzadConfig;
use crate::connectors;
use crate::device::{DeviceController, DeviceEvent};
use crate::gateway::{self, ConvStatus, GatewayCommand, GatewayEvent};
use crate::hotkey_sm::{HotkeyEffect, HotkeyInput, HotkeyState, RuntimeSnapshot};
use crate::input_log::{self, InputLogEntry, InputLogEvent, StateSnapshot};
use crate::metrics_log::{self, MetricsLogEvent, MetricsLogRecord, TranscriptMode};
use crate::model_download::{self, DownloadHandle};
use crate::models::{self, PackStatus};
use crate::platform;
use crate::platform::{
  ConnectorRowVM,
  DeviceMenuModel,
  DeviceMenuRow,
  PasteResult,
  SettingsTab,
  SettingsViewModel,
};
use crate::preferred_store;
use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod};
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
  SettingsToggleDebugStats(bool),
  SettingsSelectPasteMethod(PasteMethod),
  SettingsSelectAutoSubmit(AutoSubmitMode),
  SettingsSelectOverlayPosition(OverlayPosition),
  SettingsToggleAppendTrailingSpace(bool),
  SettingsToggleConnector {
    index: usize,
    enabled: bool,
  },
  SettingsAddRemovedWord(String),
  SettingsRemoveRemovedWord(String),
  SettingsRefresh,
  SettingsDownloadModel(String),
  SettingsCancelDownload,
  OnboardingGetStarted,
  OnboardingSetTrigger(bool),
  OnboardingToggleHistory(bool),
  OnboardingToggleAppendTrailingSpace(bool),
  OnboardingSetOverlayPosition(OverlayPosition),
  OnboardingToggleLogin(bool),
  OnboardingDownloadModel,
  OnboardingSelectDevice(usize),
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
  history_enabled: bool,
  paste_method: PasteMethod,
  auto_submit_mode: AutoSubmitMode,
  append_trailing_space_on_paste: bool,
  overlay_position: OverlayPosition,
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
  // Gate on first-run onboarding: the app must not spawn a capture session
  // (and grab the mic) until the user has finished setup. Seeded true on
  // bootstrap for users who predate the welcome flow (see `bootstrap`).
  onboarding_complete: bool,
  pending_onboarding: bool,
  onboarding_active: bool,
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
}

/// The connector latched for the current turn. `clean_query` is the transcription
/// with the trigger phrase stripped — held for the deferred routing follow-up; the
/// paste path does not consume it yet.
#[derive(Debug, Clone)]
struct ActiveConnector {
  id: &'static str,
  tag_label: &'static str,
  tag_icon: &'static str,
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
  /// A query that finalized before the socket finished connecting; flushed on `Connected`.
  pending_query_until_thread: Option<String>,
  /// The live (stripped) draft of a follow-up the user is currently speaking, before it
  /// finalizes. When set, the overlay shows it as the forming query with an empty reply so
  /// the new utterance gets its own space instead of cramping under the prior reply.
  composing_query: Option<String>,
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
      pending_query_until_thread: None,
      composing_query: None,
    }
  }
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

/// Decision predicate for the `SpeechEvent::TurnStarted` handler arm. Returns
/// true when the renderer should arm `overlay_pending_vad_text` so the next
/// non-empty `DraftUpdated` brings the live overlay up.
///
/// VAD-driven turns are handled fully by `SpeechStartedByVad` (which arms the
/// flag itself with full side effects); this predicate is the narrow defensive
/// branch for `Manual` (engine-side `ManualOverride`) turns. Manual hold's
/// hotkey effect normally opens the overlay synchronously *before* the engine
/// event arrives, leaving `overlay_visible=true` — in that case we no-op so we
/// don't double-arm. The bug case is `overlay_visible=false` at engine-event-
/// arrival time (turn 9 desync): arming here gets the overlay shown on the
/// next `DraftUpdated`.
fn turn_started_should_arm_pending(
  reason: asr::render::TurnStartedReason,
  overlay_visible: bool,
) -> bool {
  matches!(reason, asr::render::TurnStartedReason::Manual) && !overlay_visible
}

/// Decide what to do with the overlay when a non-empty `DraftUpdated` arrives.
///
/// `pending` is the armed `overlay_pending_vad_text` latch. `eligible_to_show` is a
/// *fresh* recomputation of "this turn is one we should surface live text for" —
/// always-listening-with-overlay-on-start, or manual hold, and not history-browsing.
/// We show when the overlay is hidden and EITHER the latch is armed OR the turn is
/// eligible. The `eligible_to_show` arm makes this self-healing: the latch can be lost
/// by any of ~14 clear sites (notice teardown, turn resets, session rebuild, …) between
/// turn-start and the first draft, but as long as we have real transcribed text for an
/// eligible turn we still bring the overlay up. This is what closes the recurring
/// "no overlay during streaming, flash at the end" class of bug rather than chasing each
/// trigger that drops the latch.
///
/// `cancel_suppression_active` still wins: after Escape we suppress the show for
/// `CANCEL_VAD_SHOW_SUPPRESSION_MS` and keep the latch intact (a later DraftUpdate past
/// the window re-evaluates), so a quick Escape-then-talk doesn't bounce the overlay back.
fn draft_update_overlay_action(
  pending: bool,
  overlay_visible: bool,
  cancel_suppression_active: bool,
  eligible_to_show: bool,
) -> DraftOverlayAction {
  if cancel_suppression_active {
    DraftOverlayAction::KeepPendingForLater
  } else if !overlay_visible && (pending || eligible_to_show) {
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
  finalizing_draft: &str,
) -> bool {
  // The VAD-hint branch is purely a carryover from a prior turn — it must NOT fire
  // when the live and finalizing drafts hold the same text, otherwise the renderer
  // paints two overlays with identical content (one busy, one idle). The genuine
  // turn-advance and hold paths still run via `_with_hold_for_state` above, which
  // gates on `current > finalizing` or `manual_hold_active` — neither of which can
  // produce a duplicate-text render.
  split_overlay_visible_with_hold_for_state(
    finalizing_turn_id,
    current_turn_id,
    live_draft,
    hold_active,
  ) || (finalizing_turn_id.is_some()
    && saw_vad_start_during_finalizing
    && !live_draft.trim().is_empty()
    && !draft_matches_finalized_text(live_draft, finalizing_draft))
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

/// Map a raw gateway error / disconnect reason to a short message for the overlay error
/// state, smoothing the common cases while keeping anything unexpected readable.
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
  paste_text = collapse_consecutive_duplicates(&paste_text);
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

/// Collapses consecutive duplicate words in `text`. Pure function. Runs after
/// `strip_removed_words` on the paste path; together they shape the final
/// emitted text before `platform::insert_text` fires.
///
/// Safety net for duplicate-word artifacts the asr-rs stitcher's seam-dedup
/// can't see: model-induced doubles inside a single Parakeet partial (203 of
/// 617 dup-bearing turns in the 2026-05-08 stderr.log analysis) and stable
/// user/model duplicates the full-pass also produces (159 turns).
///
/// Rules — exhaustive, each independently auditable:
/// 1. Tokenize on whitespace (matches `strip_removed_words` style).
/// 2. A token whose last character is non-alphanumeric is a "hard-break"
///    token — the next token is never compared against it. Catches sentence
///    boundaries, comma-separated spelled-out letters, parenthetical groups.
/// 3. To dedup, BOTH the previous and current token must be alphabetic-only
///    (after stripping leading/trailing punctuation) and have an alpha-key
///    of length >= 2. Protects digits, mixed-form tokens (`M3`, `1st`), and
///    single-letter spellings.
/// 4. Comparison is case-insensitive on the alpha key.
/// 5. When collapsing, drop the previous token; keep the current one. This
///    preserves any trailing punctuation that was on the duplicate's later
///    occurrence (e.g. `"the the. cat"` → `"the. cat"`).
///
/// Three-or-more-in-a-row collapses to one by induction — the rule is
/// pairwise but iterates left-to-right.
fn collapse_consecutive_duplicates(text: &str) -> String {
  let tokens: Vec<&str> = text.split_whitespace().collect();
  if tokens.len() < 2 {
    return text.to_string();
  }
  // Short-circuit when no pairwise duplicate exists — preserves the input's
  // exact whitespace (leading/trailing spaces, tab characters, etc.) instead
  // of normalising via `split_whitespace().join(" ")`. Same trick the
  // `removed_words.is_empty()` early return uses in `build_paste_text`.
  if !tokens.windows(2).any(|w| is_consecutive_duplicate(w[0], w[1])) {
    return text.to_string();
  }
  let mut kept: Vec<&str> = Vec::new();
  for tok in tokens {
    let should_collapse = kept.last().is_some_and(|prev| is_consecutive_duplicate(prev, tok));
    if should_collapse {
      kept.pop();
    }
    kept.push(tok);
  }
  kept.join(" ")
}

fn is_consecutive_duplicate(prev: &str, curr: &str) -> bool {
  // Rule 2: trailing punctuation on `prev` is a hard break.
  if prev.chars().last().map(|c| !c.is_alphanumeric()).unwrap_or(true) {
    return false;
  }
  // Rule 3: both must be alphabetic-only (after stripping edge punct) and >= 2 chars.
  if !is_alpha_word(prev) || !is_alpha_word(curr) {
    return false;
  }
  let prev_alpha = alpha_key(prev);
  let curr_alpha = alpha_key(curr);
  if prev_alpha.chars().count() < 2 {
    return false;
  }
  // Rule 4: case-insensitive on the alpha key.
  prev_alpha == curr_alpha
}

fn alpha_key(s: &str) -> String {
  s.chars().filter(|c| c.is_alphabetic()).flat_map(|c| c.to_lowercase()).collect()
}

fn is_alpha_word(s: &str) -> bool {
  let core = s.trim_matches(|c: char| !c.is_alphanumeric());
  !core.is_empty() && core.chars().all(|c| c.is_alphabetic())
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
    let history_enabled = preferred_store::load_history_enabled();
    let paste_method = preferred_store::load_paste_method();
    let auto_submit_mode = preferred_store::load_auto_submit_mode();
    let append_trailing_space_on_paste = preferred_store::load_append_trailing_space_on_paste();
    let overlay_position = preferred_store::load_overlay_position();
    let debug_stats_enabled = preferred_store::load_debug_stats_enabled();
    platform::set_overlay_debug_logs_enabled(debug_stats_enabled);
    let active_pack_id = preferred_store::load_active_model_pack()
      .unwrap_or_else(|| models::default_pack().id.to_string());
    let transcript_index = TranscriptIndex::load();
    let removed_words = preferred_store::load_removed_words();
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
      history_enabled,
      paste_method,
      auto_submit_mode,
      append_trailing_space_on_paste,
      overlay_position,
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
      onboarding_complete: preferred_store::load_onboarding_complete().unwrap_or(false),
      pending_onboarding: false,
      onboarding_active: false,
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
    // A returning/onboarded user with no explicit run-on-startup preference keeps
    // the old on-by-default behavior; a fresh (not-yet-onboarded) user defaults
    // off and onboarding sets it explicitly. Decoupled from the onboarding seed
    // above so it still fires for a user whose onboarding flag was set earlier —
    // it must never silently disable an existing user's auto-start.
    if self.onboarding_complete && preferred_store::load_run_on_startup_enabled_raw().is_none() {
      self.run_on_startup_enabled = true;
      preferred_store::save_run_on_startup_enabled(true);
    }
    self.apply_run_on_startup_preference();
    // A fresh profile goes through the welcome flow; a returning user whose
    // model is somehow missing still gets the legacy first-run Settings popup.
    if !self.onboarding_complete {
      self.pending_onboarding = true;
    } else if !self.models_ready {
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

  /// The app may spawn a capture session only once models are present AND the
  /// user has finished first-run onboarding. Gating session spawning here keeps
  /// the mic from being grabbed before the user has consented (see 38a76a7).
  fn ready_to_run(&self) -> bool {
    self.models_ready && self.onboarding_complete
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
      AppEvent::SettingsSelectOverlayPosition(pos) => {
        self.handle_settings_select_overlay_position(pos)
      }
      AppEvent::SettingsToggleAppendTrailingSpace(enabled) => {
        self.handle_settings_toggle_append_trailing_space(enabled)
      }
      AppEvent::SettingsToggleConnector { index, enabled } => {
        self.handle_settings_toggle_connector(index, enabled)
      }
      AppEvent::SettingsAddRemovedWord(word) => self.handle_settings_add_removed_word(word),
      AppEvent::SettingsRemoveRemovedWord(word) => self.handle_settings_remove_removed_word(word),
      AppEvent::SettingsRefresh => self.handle_settings_refresh(),
      AppEvent::SettingsDownloadModel(pack_id) => self.handle_settings_download_model(&pack_id),
      AppEvent::SettingsCancelDownload => self.handle_settings_cancel_download(),
      AppEvent::OnboardingGetStarted => self.handle_onboarding_get_started(),
      AppEvent::OnboardingSetTrigger(automatic) => self.handle_onboarding_set_trigger(automatic),
      AppEvent::OnboardingToggleHistory(enabled) => self.handle_onboarding_toggle_history(enabled),
      AppEvent::OnboardingToggleAppendTrailingSpace(enabled) => {
        self.handle_onboarding_toggle_append_trailing_space(enabled)
      }
      AppEvent::OnboardingSetOverlayPosition(pos) => {
        self.handle_onboarding_set_overlay_position(pos)
      }
      AppEvent::OnboardingToggleLogin(enabled) => self.handle_onboarding_toggle_login(enabled),
      AppEvent::OnboardingDownloadModel => {
        let pack_id = self.active_pack_id.clone();
        self.handle_settings_download_model(&pack_id);
      }
      AppEvent::OnboardingSelectDevice(index) => self.handle_onboarding_select_device(index),
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
    if !self.ready_to_run() {
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
    self.log_input_event(InputLogEvent::HotkeyPressed);
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

  fn handle_hotkey_released(&mut self) {
    self.log_input_event(InputLogEvent::HotkeyReleased);
    if self.history_browsing {
      // Once in history mode the user is no longer required to hold opt+space —
      // they navigate with Up/Down and dismiss with Esc/Left or paste with Enter.
      // The release is a no-op so they can let go and keep browsing freely.
      return;
    }
    if !self.models_ready {
      return;
    }
    self.dispatch_hotkey_input(HotkeyInput::HoldReleased { snapshot: self.hotkey_snapshot() });
  }

  fn handle_finalize_hotkey_pressed(&mut self, raw_requested: bool) {
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

  fn handle_onboarding_get_started(&mut self) {
    eprintln!("AZAD_ONBOARDING get-started: completing onboarding");
    self.onboarding_complete = true;
    self.onboarding_active = false;
    preferred_store::save_onboarding_complete(true);
    platform::close_onboarding_window();
    // First legitimate session spawn now that onboarding is complete.
    self.ensure_session();
  }

  fn handle_onboarding_set_trigger(&mut self, automatic: bool) {
    self.always_listening_enabled = automatic;
    preferred_store::save_always_listening_enabled(automatic);
  }

  fn handle_onboarding_toggle_history(&mut self, enabled: bool) {
    self.history_enabled = enabled;
    preferred_store::save_history_enabled(enabled);
  }

  fn handle_onboarding_toggle_append_trailing_space(&mut self, enabled: bool) {
    self.append_trailing_space_on_paste = enabled;
    preferred_store::save_append_trailing_space_on_paste(enabled);
  }

  fn handle_onboarding_set_overlay_position(&mut self, pos: OverlayPosition) {
    self.overlay_position = pos;
    preferred_store::save_overlay_position(pos);
    platform::set_overlay_position(pos);
  }

  fn handle_onboarding_set_listen_modifier(&mut self, bit: u8, enabled: bool) {
    let current = platform::listen_modifiers();
    let next = if enabled { current | bit } else { current & !bit };
    // At least one modifier is required (a bare-Space global trigger would
    // consume Space everywhere). A toggle that would clear the last one is
    // rejected; we re-sync the checkboxes to the effective mask either way.
    if next != 0 {
      platform::set_listen_modifiers(next);
      preferred_store::save_listen_modifiers(next);
    }
    platform::sync_onboarding_listen_modifiers(platform::listen_modifiers());
  }

  fn handle_onboarding_select_device(&mut self, index: usize) {
    let device_id = self
      .device_snapshot
      .as_ref()
      .and_then(|s| s.devices.get(index))
      .map(|d| d.id.clone());
    if let Some(device_id) = device_id {
      self.handle_menu_select_device(device_id);
    }
  }

  fn handle_onboarding_toggle_login(&mut self, enabled: bool) {
    // Persist unconditionally and let apply_run_on_startup_preference reconcile
    // the LaunchAgent. The settings handler only saves when an agent plist
    // already exists, which a fresh profile lacks — so the onboarding choice
    // (especially opt-out) would otherwise be silently dropped and later
    // overridden by the returning-user seeding.
    self.run_on_startup_enabled = enabled;
    preferred_store::save_run_on_startup_enabled(enabled);
    self.apply_run_on_startup_preference();
  }

  fn onboarding_view_model(&self) -> platform::OnboardingViewModel {
    let downloading = self.download_handle.is_some();
    let pack = models::pack_by_id(&self.active_pack_id).unwrap_or_else(models::default_pack);
    let header = format!("{} · {}", pack.display_name, models::format_size(pack.total_size_bytes));
    let path = models::pack_dir(&self.active_pack_id)
      .map(|p| {
        let s = p.display().to_string();
        match std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned()) {
          Some(home) => s.strip_prefix(&home).map(|rest| format!("~{rest}")).unwrap_or(s),
          None => s,
        }
      })
      .unwrap_or_default();
    let model_status_text = if downloading {
      let pct = if self.download_progress.1 > 0 {
        ((self.download_progress.0 as f64 / self.download_progress.1 as f64) * 100.0) as u8
      } else {
        0
      };
      format!("{header}\nDownloading… {pct}%")
    } else if self.models_ready {
      format!("{header}\n✓ Installed at {path}")
    } else {
      format!("{header}\nNot downloaded yet")
    };
    let download_enabled = !self.models_ready && !downloading;
    let accessibility_status = platform::accessibility_authorization();
    let microphone_status = platform::microphone_authorization();
    // "Get started" needs the model fetched (downloading or ready) AND both
    // required permissions granted.
    let get_started_enabled = (self.models_ready || downloading)
      && accessibility_status == platform::PermissionStatus::Granted
      && microphone_status == platform::PermissionStatus::Granted;
    let (devices, selected_device_index) = match &self.device_snapshot {
      Some(snapshot) => {
        let devices: Vec<(String, String)> =
          snapshot.devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
        let selected = snapshot
          .current_id
          .as_deref()
          .and_then(|cur| devices.iter().position(|(id, _)| id == cur));
        (devices, selected)
      }
      None => (Vec::new(), None),
    };
    platform::OnboardingViewModel {
      always_listening_enabled: self.always_listening_enabled,
      history_enabled: self.history_enabled,
      paste_method: self.paste_method,
      append_trailing_space_on_paste: self.append_trailing_space_on_paste,
      overlay_position: self.overlay_position,
      run_on_startup_enabled: self.run_on_startup_enabled,
      accessibility_status,
      microphone_status,
      model_status_text,
      download_enabled,
      get_started_enabled,
      devices,
      selected_device_index,
      listen_modifiers: platform::listen_modifiers(),
    }
  }

  fn apply_run_on_startup_preference(&mut self) {
    // Only register a login item when the user has opted in. Never create a
    // LaunchAgent just to disable it — a fresh profile that hasn't consented
    // keeps no login item at all. An existing plist (user opt-in, or a dev
    // `just install`) still gets its RunAtLoad synced to the preference.
    if self.run_on_startup_enabled {
      platform::create_launch_agent_plist_if_missing();
    }
    if platform::launch_agent_plist_exists()
      && !platform::set_launch_agent_startup_enabled(self.run_on_startup_enabled)
    {
      eprintln!(
        "Azad: failed to apply run-on-startup preference (enabled={})",
        self.run_on_startup_enabled
      );
    }
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
    platform::set_overlay_debug_logs_enabled(enabled);
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

  fn handle_settings_select_overlay_position(&mut self, pos: OverlayPosition) {
    self.overlay_position = pos;
    preferred_store::save_overlay_position(pos);
    platform::set_overlay_position(pos);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_toggle_append_trailing_space(&mut self, enabled: bool) {
    self.append_trailing_space_on_paste = enabled;
    preferred_store::save_append_trailing_space_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_settings_toggle_connector(&mut self, index: usize, enabled: bool) {
    let Some(connector) = self.connectors.get_mut(index) else {
      return;
    };
    connector.enabled = enabled;
    let enabled_ids: Vec<String> =
      self.connectors.iter().filter(|c| c.enabled).map(|c| c.id.to_string()).collect();
    preferred_store::save_enabled_connector_ids(&enabled_ids);
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
      // Announce readiness via the overlay for the onboarding flow, where the
      // user finished setup (closing the welcome window) while the download was
      // still running. Suppressed while the welcome window is still open — it
      // shows live progress itself.
      if !self.onboarding_active {
        self.show_overlay_notice("Model ready", "Azad is ready to dictate", Duration::from_secs(4));
      }
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
      accessibility_status: platform::accessibility_authorization(),
      microphone_status: platform::microphone_authorization(),
      run_on_startup_enabled: self.run_on_startup_enabled,
      paste_method: self.paste_method,
      auto_submit_mode: self.auto_submit_mode,
      overlay_position: self.overlay_position,
      append_trailing_space_on_paste: self.append_trailing_space_on_paste,
      debug_stats_enabled: self.debug_stats_enabled,
      metrics_text,
      model_pack_size_label: models::format_size(pack.total_size_bytes),
      model_pack_status: pack_status,
      model_download_bytes_done: self.download_progress.0,
      model_download_bytes_total: self.download_progress.1,
      removed_words: self.removed_words.clone(),
      connectors: self
        .connectors
        .iter()
        .map(|c| ConnectorRowVM { display_name: c.display_name.to_string(), enabled: c.enabled })
        .collect(),
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
    self.hold_saw_speech = false;
    self.dispatch_hotkey_input(HotkeyInput::OverlayCancelled);
    self.raw_finalize_requested = false;
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
            if self.gateway_should_handle_turn() {
              self.submit_to_gateway(turn_id, &cleaned);
            } else if self.try_paste(turn_id, TranscriptMode::Normal, &cleaned) {
              self.last_pasted_turn_id = Some(turn_id);
              self.record_history(turn_id, &cleaned);
            } else {
              eprintln!("Azad: failed to auto-paste transcript (clipboard still contains text)");
            }
          }
          // A live gateway conversation owns the card via `submit_to_gateway`; only fall
          // back to the listening overlay when no conversation is open.
          if self.overlay_visible && self.gateway_conv.is_none() {
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
        self.raw_handled_turn_id = None;
        self.latest_final = Some(cleaned.clone());
        if !self.cancelled && self.last_pasted_turn_id != Some(turn_id) {
          if self.gateway_should_handle_turn() {
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
        self.clear_overlay_pending();
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
    if self.pending_onboarding {
      self.pending_onboarding = false;
      self.onboarding_active = true;
      eprintln!("AZAD_ONBOARDING showing welcome window");
      platform::show_onboarding_window(self.onboarding_view_model());
    }
    if self.onboarding_active {
      // Push the dynamic state (download status, the "Get started" gate, and
      // permission indicators) so the welcome window updates live as the
      // download progresses and the user grants access in System Settings.
      platform::update_onboarding_window(self.onboarding_view_model());
    }
    if platform::settings_window_is_open() {
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
  /// lead-in (e.g. "hey claude") is dropped from the surfaced transcription.
  /// Returns `text` unchanged when no connector is latched. Applied at the
  /// user-facing surfaces (display, paste, history); `latest_draft` and the
  /// finalize state machine keep the full text.
  fn strip_active_trigger(&self, text: &str) -> String {
    let Some(active) = &self.active_connector else {
      return text.to_string();
    };
    match self.connectors.iter().find(|c| c.id == active.id) {
      Some(conn) => connectors::strip_trigger(text, conn.trigger),
      None => text.to_string(),
    }
  }

  #[track_caller]
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

    // A live gateway conversation owns the whole card; the finalize spinner/split lanes
    // are suppressed in favor of the streaming reply.
    if self.gateway_conv.is_some() {
      platform::hide_overlay_top();
      self.render_conversation_overlay();
      return;
    }

    if self.split_overlay_visible() {
      platform::show_overlay_top();
      platform::set_overlay_top_stream_content(
        &self.strip_active_trigger(&self.finalizing_draft),
        &self.finalizing_activity_history,
        Some(self.busy_border_phase),
      );
      platform::set_overlay_stream_content(
        &self.strip_active_trigger(&self.latest_draft),
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

    platform::hide_overlay_top();
    platform::set_overlay_stream_content(
      &self.strip_active_trigger(&self.finalizing_draft),
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
    // During a gateway conversation, follow-up speech keeps the prior exchange on screen
    // (the activity wave signals listening) rather than swapping in the plain draft view.
    if self.gateway_conv.is_some() {
      self.render_conversation_overlay();
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
      self.held_top_draft.clone()
    } else {
      self.strip_active_trigger(&self.latest_draft)
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

  /// True when this finalized turn must go to the gateway instead of being pasted —
  /// either a sticky conversation is open, or this is the first "hey claude" turn.
  fn gateway_should_handle_turn(&self) -> bool {
    self.gateway_conv.is_some()
      || self.active_connector.as_ref().map(|a| a.id) == Some(gateway::GATEWAY_AGENT)
  }

  /// Capture must stay on between turns while a conversation is open so follow-up speech
  /// is heard without re-triggering, on top of the normal always-listening / hold rules.
  fn should_keep_capture_for_followups(&self) -> bool {
    self.always_listening_enabled || self.manual_hold_active || self.gateway_conv.is_some()
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
    self.gateway_conn = GatewayConnState::Connecting;
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
      match conv.thread_id.clone() {
        None => {
          let req_id = gateway::make_request_id();
          if self.gateway_conn == GatewayConnState::Connected {
            gateway::send_command(GatewayCommand::SendNewThread { req_id, query });
          } else {
            // Finalize beat the connect; the query is flushed when `Connected` arrives.
            conv.pending_query_until_thread = Some(query);
          }
        }
        Some(thread_id) => {
          let req_id = gateway::make_request_id();
          gateway::send_command(GatewayCommand::SendFollowup { req_id, thread_id, query });
        }
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
    let busy_phase = matches!(status, ConvStatus::Thinking | ConvStatus::Streaming)
      .then_some(self.busy_border_phase);
    platform::set_overlay_conversation_content(
      &chip,
      conv.tag_icon,
      query,
      reply,
      status,
      &conv.error_msg,
      &self.activity_history,
      busy_phase,
    );
  }

  fn handle_gateway_event(&mut self, event: GatewayEvent) {
    match event {
      GatewayEvent::Connected => {
        self.gateway_conn = GatewayConnState::Connected;
        // Flush a query that finalized before the socket was ready.
        if let Some(conv) = self.gateway_conv.as_mut() {
          if conv.thread_id.is_none() {
            if let Some(query) = conv.pending_query_until_thread.take() {
              let req_id = gateway::make_request_id();
              gateway::send_command(GatewayCommand::SendNewThread { req_id, query });
            }
          }
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

  fn handle_arrow_navigate(&mut self, direction: i32) {
    self.log_input_event(InputLogEvent::ArrowNavigate { direction });
    // Up only pivots into history while opt+space is *actively held*. VAD-only
    // sessions (where the overlay shows because auto-detect picked up speech)
    // must let Up flow through to the focused app underneath — the user uses
    // Up/Down to navigate that app and can't have us hijacking those keys.
    // `enter_history_mode` cleanly cancels the in-flight transcription, so the
    // previous draft-empty / no-speech-yet gates aren't needed.
    if !self.history_browsing && direction == -1 && self.overlay_visible && self.manual_hold_active
    {
      self.enter_history_mode();
      return;
    }
    if !self.history_browsing {
      return;
    }
    // Up/Down inside the expanded view collapses back to the list view AND
    // performs the navigation step — the user gets a single keystroke that
    // returns to browsing while moving the selection.
    if self.history_expanded {
      self.history_expanded = false;
    }
    let count = self.transcript_index.as_ref().map(|i| i.entry_count()).unwrap_or(0);
    if count == 0 {
      return;
    }
    // Visible window count is dynamic — the renderer greedy-fits as many
    // rows as the card budget allows (5 when entries are 2-line, ~7 when
    // they're all 1-line). The last fitted (start, count) lives on the
    // platform side; we read them here to know whether to slide the window.
    //
    // Both directions use a symmetric `LAG = 2` rule so the selected row
    // stays off the window's edges by 2 slots after a scroll fires —
    // pressing Up or Down keeps the highlighted row roughly centred
    // instead of sticking it to the top/bottom edge.
    //
    // Up (older, browse_index += 1): scroll fires when the new selection
    //   is within `LAG` of the top of the window; visible_start advances
    //   so the selection lands `LAG` slots from the top of the new
    //   window. (Was: scroll only fired AFTER the selection had moved
    //   past the top, so the selected row sat at the top edge.)
    // Down (newer, browse_index -= 1): mirror — scroll when within
    //   `LAG` of the bottom; selection lands `LAG` slots from the
    //   bottom of the new window.
    const LAG: usize = 2;
    let last_start = platform::last_history_visible_start();
    let last_count = platform::last_history_visible_count().max(1);
    match direction {
      -1 => {
        if self.history_browse_index + 1 < count {
          self.history_browse_index += 1;
          if self.history_browse_index + LAG >= last_start + last_count {
            // Symmetric to the Down arm below: pin the selection LAG
            // slots from the top of the new window.
            self.history_visible_start =
              (self.history_browse_index + 1 + LAG).saturating_sub(last_count);
          }
        }
      }
      1 => {
        if self.history_browse_index > 0 {
          self.history_browse_index -= 1;
          if self.history_browse_index < last_start + LAG {
            self.history_visible_start = self.history_browse_index.saturating_sub(LAG);
          }
        }
      }
      _ => {}
    }
    self.render_history_overlay();
  }

  fn handle_history_collapse(&mut self) {
    self.log_input_event(InputLogEvent::HistoryCollapse);
    if !self.history_browsing {
      return;
    }
    // Left arrow now collapses an expanded view back to the list, but is a
    // no-op in list mode (was: dismiss the overlay). Esc remains the single
    // way to exit history mode entirely.
    if self.history_expanded {
      self.history_expanded = false;
      self.render_history_overlay();
    }
  }

  fn handle_history_expand(&mut self) {
    self.log_input_event(InputLogEvent::HistoryExpand);
    if !self.history_browsing || self.history_expanded {
      return;
    }
    let count = self.transcript_index.as_ref().map(|i| i.entry_count()).unwrap_or(0);
    if count == 0 {
      return;
    }
    // If the selected entry is fully visible already (rendered without an
    // ellipsis), expanding wouldn't reveal anything — make right-arrow a
    // no-op rather than flipping into a state with no visual difference.
    if !platform::last_history_selected_truncated() {
      return;
    }
    self.history_expanded = true;
    self.render_history_overlay();
  }

  fn render_history_overlay(&self) {
    let Some(index) = &self.transcript_index else {
      if self.debug_stats_enabled {
        eprintln!("AZAD_HISTORY_RENDER action=no_index browse_index={}", self.history_browse_index);
      }
      platform::set_overlay_history_content(&[], 0, 0, false);
      return;
    };
    // FTS5-backed search: empty query returns the cache; non-empty applies
    // tokenized prefix matching with BM25 ranking. Match ranges come back
    // pre-computed from FTS5's highlight().
    const HISTORY_SEARCH_LIMIT: usize = 1000;
    let hits = index.search(&self.history_search_query, HISTORY_SEARCH_LIMIT);
    let entries: Vec<platform::HistoryEntryView<'_>> = hits
      .iter()
      .map(|h| platform::HistoryEntryView {
        text: h.final_text.as_str(),
        match_ranges: h.match_ranges.clone(),
        ts_ms: h.ts_ms,
        char_count: h.final_text.chars().count(),
      })
      .collect();
    let visible = entries.len();
    let selected = self.history_browse_index.min(visible.saturating_sub(1));
    let visible_start = self.history_visible_start.min(visible.saturating_sub(1));
    if self.debug_stats_enabled {
      let preview = entries
        .first()
        .map(|e| &e.text[..e.text.len().min(40)])
        .unwrap_or("(no entries)");
      eprintln!(
        "AZAD_HISTORY_RENDER mode={} filtered={} selected={} \
         visible_start={} query={:?} first_preview={:?}",
        if self.history_expanded { "expanded" } else { "list" },
        entries.len(),
        selected,
        visible_start,
        self.history_search_query,
        preview,
      );
    }
    platform::set_overlay_history_content(&entries, selected, visible_start, self.history_expanded);
  }

  /// Returns the text of the entry at the user's current cursor position
  /// inside the (search-filtered) list. None when no entry matches.
  fn selected_history_entry_text(&self) -> Option<String> {
    let index = self.transcript_index.as_ref()?;
    const HISTORY_SEARCH_LIMIT: usize = 1000;
    let hits = index.search(&self.history_search_query, HISTORY_SEARCH_LIMIT);
    hits.into_iter().nth(self.history_browse_index).map(|h| h.final_text)
  }

  fn paste_from_history(&mut self) {
    if let Some(text) = self.selected_history_entry_text() {
      let paste_text =
        build_paste_text(&text, self.append_trailing_space_on_paste, &self.removed_words);
      // Drop key-input claims BEFORE firing the synthetic Cmd+V. While
      // `OVERLAY_ACCEPTS_KEY_INPUT` is set the HID tap's
      // `claim_tap_search_input` intercepts every printable keydown — and
      // `CGEventKeyboardGetUnicodeString` reports "v" for the V keydown
      // even with Cmd held, so the tap was treating our own paste as a
      // search-bar keystroke (`HistorySearchAppend("v")`) and the focused
      // app never saw the chord. Resigning here lets the synthetic event
      // flow through to the OS routing.
      platform::set_overlay_key_input_enabled(false);
      let _ = platform::insert_text(&paste_text, self.paste_method, self.cfg.paste_delay_ms);
      let _ = platform::send_auto_submit(self.auto_submit_mode);
    }
    // Exit even on an empty-state release so the overlay closes — otherwise
    // the "No transcripts" overlay would linger after opt+space release.
    self.exit_history_mode();
  }

  fn handle_history_search_changed(&mut self, query: String) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "changed",
      chars_appended: Some(query.chars().count()),
    });
    if !self.history_browsing {
      return;
    }
    if self.history_search_query == query {
      return;
    }
    self.history_search_query = query;
    self.after_history_search_change();
  }

  fn handle_history_search_append(&mut self, s: &str) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "append",
      chars_appended: Some(s.chars().count()),
    });
    if !self.history_browsing {
      return;
    }
    self.history_search_query.push_str(s);
    self.after_history_search_change();
  }

  fn handle_history_search_backspace(&mut self) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "backspace",
      chars_appended: None,
    });
    if !self.history_browsing {
      return;
    }
    if self.history_search_query.pop().is_none() {
      return;
    }
    self.after_history_search_change();
  }

  fn handle_history_search_delete_word(&mut self) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "delete_word",
      chars_appended: None,
    });
    if !self.history_browsing || self.history_search_query.is_empty() {
      return;
    }
    // Trim trailing whitespace, then truncate at the next whitespace
    // boundary (or to empty). Matches macOS native Option+Backspace.
    let trimmed = self.history_search_query.trim_end().to_string();
    if let Some(idx) = trimmed.rfind(char::is_whitespace) {
      // Keep up to and including the whitespace char so subsequent
      // Option+Backspace still has a separator to walk past.
      let cut = trimmed[..idx].trim_end().len();
      self.history_search_query.truncate(cut);
    } else {
      self.history_search_query.clear();
    }
    self.after_history_search_change();
  }

  fn handle_history_search_clear(&mut self) {
    self.log_input_event(InputLogEvent::HistorySearchEdit { kind: "clear", chars_appended: None });
    if !self.history_browsing || self.history_search_query.is_empty() {
      return;
    }
    self.history_search_query.clear();
    self.after_history_search_change();
  }

  fn after_history_search_change(&mut self) {
    // Reset selection to the top of the (newly) filtered list so the
    // selection is always valid as long as ≥ 1 result exists.
    self.history_browse_index = 0;
    self.history_visible_start = 0;
    self.history_expanded = false;
    // Mirror the buffer into the visible NSTextField so the user sees what
    // they're typing (the HID tap funnels keystrokes around AppKit, so the
    // field doesn't auto-update).
    platform::set_overlay_search_query(&self.history_search_query);
    self.render_history_overlay();
  }

  fn enter_history_mode(&mut self) {
    // Cancel any in-flight turn AND stop capture. Without `set_capture_enabled(false)`
    // the audio thread keeps producing chunks while the user is browsing — even
    // though `handle_speech_event`'s `history_browsing` guard drops incoming
    // events from the user's perspective, the engine still produces a draft and
    // appends a new entry to `transcript_index` if a turn finalizes. Worse: if a
    // turn finishes RIGHT AS the user dismisses history mode, that turn's
    // FinalText fires after `history_browsing` is cleared and gets pasted in
    // addition to the user's chosen history entry. Setting capture off prevents
    // the engine from doing any of that. Re-enable on exit. The
    // `SessionEnded` event that `set_capture_enabled(false)` may trigger is
    // harmless because the same `history_browsing` guard drops it.
    self.manual_hold_active = false;
    self.hold_saw_speech = false;
    if let Some(session) = &self.session {
      session.release_manual_hold();
      session.cancel_current_turn();
      session.set_capture_enabled(false);
    }
    self.latest_draft.clear();
    self.finalizing_draft.clear();
    self.finalizing_turn_id = None;
    self.finalizing_deadline = None;
    self.history_browsing = true;
    self.history_browse_index = 0;
    self.history_visible_start = 0;
    self.history_expanded = false;
    self.history_search_query.clear();
    platform::set_arrow_left_hotkey_enabled(true);
    platform::set_arrow_right_hotkey_enabled(true);
    platform::reset_click_outside_tracker();
    platform::set_overlay_search_query("");
    platform::set_overlay_key_input_enabled(true);
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
    self.history_visible_start = 0;
    self.history_expanded = false;
    self.history_search_query.clear();
    self.overlay_visible = false;
    platform::set_overlay_key_input_enabled(false);
    platform::set_overlay_search_query("");
    platform::set_arrow_left_hotkey_enabled(false);
    platform::set_arrow_right_hotkey_enabled(false);
    platform::hide_overlay();
    // Restore capture so always-listening continues and the next opt+space hold
    // works. Capture in non-always-listening mode will be turned back off by
    // the engine when the next manual hold ends.
    if let Some(session) = &self.session {
      session.set_capture_enabled(true);
    }
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
        self.active_connector = Some(ActiveConnector {
          id,
          tag_label: m.tag_label,
          tag_icon: m.tag_icon,
          clean_query: m.clean_query,
        });
        // Warm the socket while the user is still speaking so it's ready by finalize.
        if id == gateway::GATEWAY_AGENT {
          self.maybe_begin_gateway_connect();
        }
      }
      return;
    }
    let id = self.active_connector.as_ref().map(|a| a.id);
    let trigger = id.and_then(|id| self.connectors.iter().find(|c| c.id == id)).map(|c| c.trigger);
    if let (Some(trigger), Some(active)) = (trigger, self.active_connector.as_mut()) {
      active.clean_query = connectors::strip_trigger(&self.latest_draft, trigger);
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
    self.deferred_vad_start = false;
    self.accessibility_notice_deadline = None;
    self.listen_toggle_notice = None;
    self.clear_overlay_pending();
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
    self.dispatch_hotkey_input(HotkeyInput::SpeechFinalized);
    self.latest_final = Some(raw_text.clone());

    // Opt+Enter while a gateway turn/conversation is live submits the query instead of
    // raw-pasting; keep the overlay up and capture on.
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
  use std::time::{Duration, Instant};

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
    collapse_consecutive_duplicates,
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
    turn_started_should_arm_pending,
  };
  use super::{LISTEN_TOGGLE_NOTICE_DURATION_MS, ListenToggleNotice};

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
      action: ManualHoldReleaseAction::HideOverlay
    });
  }

  #[test]
  fn manual_hold_release_plan_keeps_live_when_not_finalizing() {
    let plan = manual_hold_release_plan(false, false, true);
    assert_eq!(plan, ManualHoldReleasePlan {
      capture_enabled: false,
      action: ManualHoldReleaseAction::KeepLive
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
  fn collapse_dup_basic_two_in_a_row() {
    assert_eq!(collapse_consecutive_duplicates("the the cat"), "the cat");
  }

  #[test]
  fn collapse_dup_three_or_more_in_a_row() {
    // Pairwise iteration collapses N-in-a-row down to 1.
    assert_eq!(collapse_consecutive_duplicates("that that that idea"), "that idea");
    assert_eq!(collapse_consecutive_duplicates("uh uh uh uh hello"), "uh hello");
  }

  #[test]
  fn collapse_dup_period_acts_as_barrier() {
    // Trailing period on the previous token blocks dedup — it's a sentence boundary.
    assert_eq!(collapse_consecutive_duplicates("the. the cat"), "the. the cat");
    assert_eq!(collapse_consecutive_duplicates("end. End of sentence."), "end. End of sentence.");
  }

  #[test]
  fn collapse_dup_comma_acts_as_barrier_for_letter_spelling() {
    // Spelled-out letter sequences must survive: every comma is a hard break,
    // and the single-letter alpha-key fails the len-≥-2 rule on top of that.
    assert_eq!(collapse_consecutive_duplicates("S, P, E, N, C, E, R"), "S, P, E, N, C, E, R");
  }

  #[test]
  fn collapse_dup_single_letter_no_collapse() {
    // No commas — len-≥-2 alpha-key rule still protects single-letter spellings.
    assert_eq!(collapse_consecutive_duplicates("M M alpha"), "M M alpha");
    assert_eq!(collapse_consecutive_duplicates("A A B B"), "A A B B");
  }

  #[test]
  fn collapse_dup_digits_no_collapse() {
    // `is_alpha_word` rejects digit-only tokens; numeric codes survive.
    assert_eq!(collapse_consecutive_duplicates("2288 2288"), "2288 2288");
    // User's own example — codes read aloud with comma/period pauses.
    assert_eq!(collapse_consecutive_duplicates("2288. Eight, eight."), "2288. Eight, eight.");
  }

  #[test]
  fn collapse_dup_preserves_trailing_punct_on_survivor() {
    // When a duplicate has trailing punctuation on its later occurrence, drop
    // the previous (no-punct) copy and keep the punctuation-bearing one.
    assert_eq!(collapse_consecutive_duplicates("the the. cat"), "the. cat");
    assert_eq!(collapse_consecutive_duplicates("uh uh, hello"), "uh, hello");
  }

  #[test]
  fn collapse_dup_case_insensitive() {
    // Match modulo case; survivor is the LATER token, so its casing wins.
    assert_eq!(collapse_consecutive_duplicates("The the cat"), "the cat");
    assert_eq!(collapse_consecutive_duplicates("the The cat"), "The cat");
  }

  /// Documents the ACCEPTED-trade-off: spelled-out number words spoken
  /// without comma/period pauses ("two two eight eight" as a code) DO get
  /// collapsed today. Speakers naturally pause when reading codes, which
  /// produces the punctuation barriers that protect the case. If real-world
  /// false-positives become a problem, add a small static whitelist of number
  /// words ("one"-"twelve", "twenty"-"ninety", "hundred", "thousand",
  /// "million") to `is_consecutive_duplicate` as a fifth rule. This test
  /// trips when that change lands and forces an explicit decision.
  #[test]
  fn collapse_dup_known_false_positive_unpunctuated_number_words() {
    assert_eq!(collapse_consecutive_duplicates("two two eight eight"), "two eight");
  }

  #[test]
  fn build_paste_text_runs_filler_then_dedup_in_order() {
    // End-to-end: filler removal first, dedup second. The "um the the cat"
    // input first becomes "the the cat" (filler stripped), then "the cat"
    // (dedup'd). Pins ordering inside `build_paste_text`.
    let words = vec!["um".to_string()];
    assert_eq!(build_paste_text("um the the cat", false, &words), "the cat");
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
