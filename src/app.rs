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
use crate::platform;
use crate::platform::{DeviceMenuModel, DeviceMenuRow, PasteResult, SettingsViewModel};
use crate::preferred_store;
use crate::speech::{SpeechEvent, SpeechSession, spawn_speech_session};

const DEVICE_SWITCH_RESTART_DEBOUNCE_MS: u64 = 250;
const OVERLAY_ACTIVITY_HISTORY_LEN: usize = 96;
const OVERLAY_ACTIVITY_IDLE_TIMEOUT_MS: u64 = 220;
const OVERLAY_ACTIVITY_DECAY_PER_TICK: f32 = 0.88;
const OVERLAY_BUSY_PHASE_STEP: f32 = 0.24;

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
    SettingsToggleDebugStats(bool),
    SettingsRefresh,
    OverlayCancel,
    Speech(SpeechEvent),
    Device(DeviceEvent),
}

static EVENT_TX: OnceLock<Sender<AppEvent>> = OnceLock::new();
static EVENT_RX: OnceLock<Mutex<Receiver<AppEvent>>> = OnceLock::new();
static CONTROLLER: OnceLock<Mutex<AppController>> = OnceLock::new();
static HOTKEY_CLOCK_START: OnceLock<Instant> = OnceLock::new();

pub fn run() {
    platform::check_required_permissions_on_startup();

    let (tx, rx) = mpsc::channel::<AppEvent>();
    let _ = EVENT_TX.set(tx);
    let _ = EVENT_RX.set(Mutex::new(rx));

    let mut controller = AppController::new(AzadConfig::default());
    controller.bootstrap();
    let _ = CONTROLLER.set(Mutex::new(controller));

    platform::run_app();
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

    manual_hold_active: bool,
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
    raw_handled_turn_id: Option<u64>,
    deferred_vad_start: bool,
    accessibility_notice_deadline: Option<Instant>,
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
    debug_stats_enabled: bool,
    turn_started_at: HashMap<u64, Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawFinalizeUiPlan {
    hide_overlay: bool,
    disable_capture: bool,
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

fn should_ignore_finalizing_event(raw_handled_turn_id: Option<u64>, turn_id: u64) -> bool {
    raw_handled_turn_id == Some(turn_id)
}

fn split_overlay_active_for_turns(finalizing_turn_id: Option<u64>, current_turn_id: Option<u64>) -> bool {
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

impl AppController {
    fn new(cfg: AzadConfig) -> Self {
        let always_listening_enabled = preferred_store::load_always_listening_enabled();
        let debug_stats_enabled = preferred_store::load_debug_stats_enabled();
        Self {
            cfg,
            session: None,
            session_device_id: None,
            next_session_id: 1,
            device_controller: None,
            device_snapshot: None,
            device_menu_expanded: false,
            always_listening_enabled,
            manual_hold_active: false,
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
            raw_handled_turn_id: None,
            deferred_vad_start: false,
            accessibility_notice_deadline: None,
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
            debug_stats_enabled,
            turn_started_at: HashMap::new(),
        }
    }

    fn bootstrap(&mut self) {
        self.start_device_controller();
        self.render_device_menu();
        self.ensure_session();
    }

    fn start_device_controller(&mut self) {
        let preferred = preferred_store::load_preferred_device_id();

        let emit: Arc<dyn Fn(DeviceEvent) + Send + Sync> =
            Arc::new(|ev| send_event(AppEvent::Device(ev)));

        match DeviceController::start(preferred, emit) {
            Ok(controller) => {
                if let Ok(snapshot) = controller.snapshot() {
                    self.handle_device_state_changed(snapshot);
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
        self.device_snapshot
            .as_ref()
            .and_then(|s| s.current_id.as_deref())
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
            AppEvent::SettingsToggleDebugStats(enabled) => {
                self.handle_settings_toggle_debug_stats(enabled)
            }
            AppEvent::SettingsRefresh => self.handle_settings_refresh(),
            AppEvent::OverlayCancel => self.handle_overlay_cancel(),
            AppEvent::Speech(ev) => self.handle_speech_event(ev),
            AppEvent::Device(ev) => self.handle_device_event(ev),
        }
    }

    fn start_session(&mut self) {
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
        self.finalizing_activity_history
            .resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
        self.latest_final = None;
        self.finalizing_deadline = None;
        self.finalizing_turn_id = None;
        self.raw_handled_turn_id = None;
        self.deferred_vad_start = false;
        self.accessibility_notice_deadline = None;
        self.last_pasted_turn_id = None;
        self.cancelled = false;
        self.overlay_pending_vad_text = false;
        self.latest_seen_turn_id = 0;
        self.turn_accept_floor = 1;
        self.current_turn_id = None;
        self.dispatch_hotkey_input(HotkeyInput::SessionReset);
        self.raw_finalize_requested = false;
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
        self.dispatch_hotkey_input(HotkeyInput::HoldPressed {
            now_ms: self.hotkey_now_ms(),
            snapshot: self.hotkey_snapshot(),
        });
    }

    fn handle_hotkey_released(&mut self) {
        self.dispatch_hotkey_input(HotkeyInput::HoldReleased {
            snapshot: self.hotkey_snapshot(),
        });
    }

    fn handle_finalize_hotkey_pressed(&mut self, raw_requested: bool) {
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

    fn apply_always_listening_toggle(&mut self) {
        self.always_listening_enabled = !self.always_listening_enabled;
        preferred_store::save_always_listening_enabled(self.always_listening_enabled);

        self.ensure_session();
        if let Some(session) = &self.session {
            session.set_auto_vad_enabled(self.always_listening_enabled);
            let should_capture = self.always_listening_enabled || self.manual_hold_active;
            session.set_capture_enabled(should_capture);
        }
        self.overlay_pending_vad_text = false;
        self.render_device_menu();
    }

    fn hotkey_now_ms(&self) -> u64 {
        let start = HOTKEY_CLOCK_START.get_or_init(Instant::now);
        start.elapsed().as_millis() as u64
    }

    fn hotkey_snapshot(&self) -> RuntimeSnapshot {
        let has_active_speech_turn =
            self.engine_state == EngineState::Speech && self.finalizing_turn_id.is_none();
        let has_started_turn = self.engine_state == EngineState::Speech
            || self.current_turn_id.is_some()
            || self.finalizing_turn_id.is_some()
            || !self.latest_draft.trim().is_empty();
        RuntimeSnapshot {
            always_listening_enabled: self.always_listening_enabled,
            has_active_speech_turn,
            has_turn_context: has_started_turn,
            has_started_turn,
            overlay_visible: self.overlay_visible,
            manual_hold_active: self.manual_hold_active,
        }
    }

    fn split_overlay_active(&self) -> bool {
        split_overlay_active_for_turns(self.finalizing_turn_id, self.current_turn_id)
    }

    fn split_overlay_visible(&self) -> bool {
        split_overlay_visible_for_state(
            self.finalizing_turn_id,
            self.current_turn_id,
            &self.latest_draft,
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
        self.latest_draft.clear();
        self.finalizing_draft.clear();
        self.finalizing_activity_history
            .resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
        self.latest_final = None;
        self.raw_handled_turn_id = None;
        self.raw_finalize_requested = false;
        self.deferred_vad_start = false;
        self.accessibility_notice_deadline = None;
        self.overlay_pending_vad_text = false;
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
            HotkeyEffect::ToggleAlwaysListening => self.apply_always_listening_toggle(),
            HotkeyEffect::CompletePureToggleGesture => {
                self.manual_hold_active = false;
                if let Some(session) = &self.session {
                    session.release_manual_hold();
                    session.set_capture_enabled(self.always_listening_enabled);
                }
                self.hide_overlay();
            }
            HotkeyEffect::ActivateManualHold {
                reset_turn_state,
                release_should_finalize: _,
            } => {
                self.manual_hold_active = true;
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
            HotkeyEffect::ReleaseManualHold {
                should_finalize,
                has_started_turn,
            } => {
                self.manual_hold_active = false;
                if let Some(session) = &self.session {
                    session.release_manual_hold();
                    if should_finalize {
                        if has_started_turn {
                            session.finalize_current_turn();
                        } else {
                            session.set_capture_enabled(self.always_listening_enabled);
                            self.hide_overlay();
                        }
                    } else {
                        session.set_capture_enabled(self.always_listening_enabled);
                    }
                }
            }
            HotkeyEffect::FinalizeFromHotkey => {
                if !self.overlay_visible {
                    return;
                }
                self.manual_hold_active = false;
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

    fn handle_settings_toggle_debug_stats(&mut self, enabled: bool) {
        self.debug_stats_enabled = enabled;
        preferred_store::save_debug_stats_enabled(enabled);
        if let Some(session) = &self.session {
            session.set_debug_stats_enabled(enabled);
        }
        platform::update_settings_window(self.settings_view_model());
    }

    fn handle_settings_refresh(&mut self) {
        platform::update_settings_window(self.settings_view_model());
    }

    fn settings_view_model(&self) -> SettingsViewModel {
        let metrics_text = match metrics_log::summarize_last_24h() {
            Ok(summary) => metrics_log::render_summary(&summary),
            Err(err) => format!("Failed to load debug metrics: {err}"),
        };

        SettingsViewModel {
            debug_stats_enabled: self.debug_stats_enabled,
            metrics_text,
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
        if !self.overlay_visible {
            return;
        }
        let split_active = self.split_overlay_visible();
        self.cancelled = true;
        self.manual_hold_active = false;
        self.dispatch_hotkey_input(HotkeyInput::OverlayCancelled);
        self.raw_finalize_requested = false;
        self.overlay_pending_vad_text = false;
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

        match event {
            SpeechEvent::SessionStarted { .. } => {}
            SpeechEvent::Listening { .. } => {
                if self.overlay_visible {
                    self.render_listening_overlay();
                }
            }
            SpeechEvent::SpeechStartedByVad { .. } => {
                if self.finalizing_turn_id.is_some() {
                    // Keep finalizing lane visible and prepare a fresh live lane below it.
                    self.latest_draft.clear();
                    self.latest_final = None;
                    self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
                    self.reset_activity_history();
                    return;
                }
                self.reset_turn_state();
                if self.overlay_visible {
                    self.hide_overlay();
                }
                // In auto-VAD mode, wait for actual draft text before showing overlay.
                self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
            }
            SpeechEvent::DraftUpdated {
                turn_id,
                committed,
                live,
                ..
            } => {
                if !self.accept_turn(turn_id) {
                    return;
                }
                self.observe_turn(turn_id);
                let merged = format!("{committed}{live}");
                let merged = merged.trim().to_string();
                if !merged.is_empty() {
                    self.latest_draft = merged;
                    if self.overlay_pending_vad_text && !self.overlay_visible {
                        self.show_overlay_listening();
                    }
                    self.overlay_pending_vad_text = false;
                }
                if self.overlay_visible {
                    if self.finalizing_deadline.is_some() {
                        self.render_finalizing_overlay_state();
                    } else {
                        self.render_listening_overlay();
                    }
                }
            }
            SpeechEvent::Meter {
                peak_db,
                vad_speech,
                vad_prob,
                ..
            } => {
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
            SpeechEvent::Finalizing {
                turn_id,
                current_draft,
                ..
            } => {
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
                    self.finalizing_draft = current_draft.trim().to_string();
                    if self.current_turn_id == Some(turn_id) {
                        self.latest_draft = self.finalizing_draft.clone();
                    }
                } else if self.finalizing_draft.is_empty() {
                    self.finalizing_draft = self.latest_draft.clone();
                }
                self.finalizing_activity_history.clone_from(&self.activity_history);
                self.finalizing_turn_id = Some(turn_id);
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
                let split_top_completion = self
                    .finalizing_turn_id
                    .is_some_and(|finalizing_turn_id| {
                        finalizing_turn_id == turn_id && self.split_overlay_visible()
                    });
                if !split_top_completion {
                    self.observe_turn(turn_id);
                }
                self.finalizing_turn_id = if split_top_completion {
                    None
                } else {
                    self.finalizing_turn_id.and_then(|id| (id != turn_id).then_some(id))
                };
                self.finalizing_deadline = if self.finalizing_turn_id.is_some() {
                    self.finalizing_deadline
                } else {
                    None
                };
                if split_top_completion || self.finalizing_turn_id.is_none() {
                    self.finalizing_draft.clear();
                }
                self.raw_finalize_requested = false;
                if !split_top_completion {
                    self.dispatch_hotkey_input(HotkeyInput::SpeechFinalized);
                }

                if split_top_completion {
                    self.turn_started_at.remove(&turn_id);
                    self.raw_handled_turn_id = None;
                    if !cleaned.is_empty()
                        && !self.cancelled
                        && self.last_pasted_turn_id != Some(turn_id)
                    {
                        if self.try_paste(turn_id, TranscriptMode::Normal, &cleaned) {
                            self.last_pasted_turn_id = Some(turn_id);
                        } else {
                            eprintln!(
                                "Azad: failed to auto-paste transcript (clipboard still contains text)"
                            );
                        }
                    }
                    if self.overlay_visible {
                        self.render_listening_overlay();
                    }
                    return;
                }

                if cleaned.is_empty() {
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
                    if !self.manual_hold_active {
                        self.hide_overlay();
                    }
                    if self.try_paste(turn_id, TranscriptMode::Normal, &cleaned) {
                        self.last_pasted_turn_id = Some(turn_id);
                    } else {
                        eprintln!(
                            "Azad: failed to auto-paste transcript (clipboard still contains text)"
                        );
                    }
                }
                self.maybe_start_deferred_vad_turn();
                if !self.always_listening_enabled && !self.manual_hold_active {
                    if let Some(session) = &self.session {
                        session.set_capture_enabled(false);
                    }
                }
            }
            SpeechEvent::SessionEnded { .. } => {
                self.engine_state = EngineState::Idle;
                if !self.cancelled
                    && self.latest_seen_turn_id > 0
                    && self.last_pasted_turn_id != Some(self.latest_seen_turn_id)
                {
                    self.hide_overlay();
                    if let Some(final_text) = self.latest_final.as_ref() {
                        let cleaned = final_text.trim().to_string();
                        if !cleaned.is_empty() {
                            if self.try_paste(
                                self.latest_seen_turn_id,
                                TranscriptMode::Normal,
                                &cleaned,
                            ) {
                                self.last_pasted_turn_id = Some(self.latest_seen_turn_id);
                            }
                        }
                    }
                }

                self.hide_overlay();
                self.session = None;
                self.latest_draft.clear();
                self.finalizing_draft.clear();
                self.finalizing_activity_history
                    .resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
                self.latest_final = None;
                self.finalizing_deadline = None;
                self.finalizing_turn_id = None;
                self.raw_handled_turn_id = None;
                self.raw_finalize_requested = false;
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
            }
            SpeechEvent::Error { message, .. } => {
                if self.overlay_visible {
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
        self.advance_activity_timeline();

        if let Some(deadline) = self.pending_device_switch_deadline {
            if Instant::now() >= deadline {
                self.pending_device_switch_deadline = None;
                let target = self.pending_device_switch_target.take();
                if let Some(target) = target {
                    let still_current = self.current_device_id() == Some(target.as_str());
                    let needs_restart = self.session.is_some()
                        && self.session_device_id.as_deref() != Some(target.as_str());
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
            if Instant::now() >= deadline {
                self.accessibility_notice_deadline = None;
                if self.overlay_visible
                    && !self.manual_hold_active
                    && self.finalizing_deadline.is_none()
                {
                    self.hide_overlay();
                }
            }
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
        );
    }

    fn render_listening_overlay(&self) {
        if self.accessibility_notice_deadline.is_some() {
            return;
        }
        platform::hide_overlay_top();
        platform::set_overlay_stream_content(
            &self.latest_draft,
            &self.activity_history,
            None,
            self.raw_badge_visible(),
            self.hold_badge_visible(),
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

    fn show_accessibility_overlay_notice(&mut self) {
        if !self.overlay_visible {
            platform::show_overlay();
            self.overlay_visible = true;
        }
        self.accessibility_notice_deadline = Some(Instant::now() + Duration::from_secs(6));
        platform::set_overlay_notice_content(
            "Auto-paste blocked",
            "Enable Azad in System Settings -> Privacy & Security -> Accessibility",
        );
    }

    fn try_paste(&mut self, turn_id: u64, mode: TranscriptMode, text: &str) -> bool {
        let mut paste_text = text.to_string();
        if !paste_text
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            paste_text.push(' ');
        }

        let paste_started = Instant::now();
        let paste_result = platform::paste_text(&paste_text, self.cfg.paste_delay_ms);
        if matches!(paste_result, PasteResult::AccessibilityRequired) {
            self.show_accessibility_overlay_notice();
        }

        if self.debug_stats_enabled {
            let paste_result_label = match paste_result {
                PasteResult::Pasted => "pasted",
                PasteResult::AccessibilityRequired => "accessibility_required",
                PasteResult::EmptyText => "empty_text",
                PasteResult::ClipboardWriteFailed => "clipboard_write_failed",
            };
            let paste_duration_ms =
                u64::try_from(paste_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = metrics_log::append_record(&MetricsLogRecord::new(
                MetricsLogEvent::PasteCompleted {
                    turn_id,
                    mode,
                    paste_duration_ms,
                    result: paste_result_label.to_string(),
                },
            ));

            if let Some(started_at) = self.turn_started_at.remove(&turn_id) {
                let transcription_duration_ms =
                    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
                let _ = metrics_log::append_record(&MetricsLogRecord::new(
                    MetricsLogEvent::TurnCompleted {
                        turn_id,
                        mode,
                        transcription_duration_ms,
                    },
                ));
            }
        }

        matches!(paste_result, PasteResult::Pasted)
    }

    fn hide_overlay(&mut self) {
        self.overlay_pending_vad_text = false;
        if self.overlay_visible {
            platform::hide_overlay();
            self.overlay_visible = false;
        }
    }

    fn reset_turn_state(&mut self) {
        self.dispatch_hotkey_input(HotkeyInput::TurnReset);
        self.reset_turn_state_preserving_hotkey_state();
    }

    fn reset_turn_state_preserving_hotkey_state(&mut self) {
        self.cancelled = false;
        self.last_pasted_turn_id = None;
        self.latest_draft.clear();
        self.latest_final = None;
        self.finalizing_deadline = None;
        self.finalizing_turn_id = None;
        self.raw_handled_turn_id = None;
        self.raw_finalize_requested = false;
        self.deferred_vad_start = false;
        self.accessibility_notice_deadline = None;
        self.overlay_pending_vad_text = false;
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
            let should_hide_overlay =
                ui_plan.hide_overlay && (raw_targets_finalizing_lane || self.finalizing_turn_id.is_none());
            if should_hide_overlay {
                self.hide_overlay();
            }
            if self.try_paste(turn_id, TranscriptMode::Raw, &raw_text) {
                self.last_pasted_turn_id = Some(turn_id);
            } else {
                eprintln!(
                    "Azad: failed to auto-paste raw transcript (clipboard still contains text)"
                );
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
            DebugStatsEvent::PartialFinalizeOutcome {
                turn_id,
                outcome,
                reason,
            } => {
                let _ =
                    metrics_log::append_record(&MetricsLogRecord::new(
                        MetricsLogEvent::PartialFinalizeOutcome {
                            turn_id,
                            outcome,
                            reason,
                        },
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
                let _ = metrics_log::append_record(&MetricsLogRecord::new(
                    MetricsLogEvent::PartialAuditResult {
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
                    },
                ));
            }
            DebugStatsEvent::PartialAuditError {
                turn_id,
                emitted_kind,
                partial_count,
                message,
            } => {
                let _ = metrics_log::append_record(&MetricsLogRecord::new(
                    MetricsLogEvent::PartialAuditError {
                        turn_id,
                        emitted_kind,
                        partial_count,
                        message,
                    },
                ));
            }
        }
    }

    fn accept_turn(&self, turn_id: u64) -> bool {
        turn_id >= self.turn_accept_floor
    }

    fn observe_turn(&mut self, turn_id: u64) {
        self.latest_seen_turn_id = self.latest_seen_turn_id.max(turn_id);
        self.current_turn_id = Some(next_current_turn_id(self.current_turn_id, turn_id));
        self.turn_started_at
            .entry(turn_id)
            .or_insert_with(Instant::now);
    }

    fn update_activity_from_meter(&mut self, peak_db: f32, vad_speech: bool, vad_prob: f32) {
        let normalized_peak = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
        let vad_component = if vad_speech {
            vad_prob.clamp(0.0, 1.0).max(0.15)
        } else {
            0.0
        };
        let mut next = normalized_peak.max(vad_component);
        if !vad_speech {
            next *= 0.7;
        }
        self.latest_activity_level = next.clamp(0.0, 1.0);
        self.last_activity_at = Some(Instant::now());
    }

    fn reset_activity_history(&mut self) {
        self.activity_history.clear();
        self.activity_history
            .resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
        self.latest_activity_level = 0.0;
        self.last_activity_at = None;
    }

    fn advance_activity_timeline(&mut self) {
        if self.activity_history.len() != OVERLAY_ACTIVITY_HISTORY_LEN {
            self.activity_history
                .resize(OVERLAY_ACTIVITY_HISTORY_LEN, 0.0);
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
        RawFinalizeUiPlan, next_current_turn_id, raw_finalize_target_turn_id_for_state,
        raw_finalize_ui_plan, should_ignore_finalizing_event, split_overlay_active_for_turns,
        split_overlay_visible_for_state,
    };

    #[test]
    fn raw_finalize_hotkey_forces_overlay_hide_even_during_manual_hold() {
        let plan = raw_finalize_ui_plan(false, true, true);
        assert_eq!(
            plan,
            RawFinalizeUiPlan {
                hide_overlay: true,
                disable_capture: false,
            }
        );
    }

    #[test]
    fn non_hotkey_raw_finalize_keeps_overlay_when_manual_hold_is_active() {
        let plan = raw_finalize_ui_plan(false, true, false);
        assert_eq!(
            plan,
            RawFinalizeUiPlan {
                hide_overlay: false,
                disable_capture: false,
            }
        );
    }

    #[test]
    fn raw_finalize_without_hold_in_manual_mode_hides_overlay_and_disables_capture() {
        let plan = raw_finalize_ui_plan(false, false, false);
        assert_eq!(
            plan,
            RawFinalizeUiPlan {
                hide_overlay: true,
                disable_capture: true,
            }
        );
    }

    #[test]
    fn raw_finalize_without_hold_in_always_listening_hides_overlay_but_keeps_capture() {
        let plan = raw_finalize_ui_plan(true, false, false);
        assert_eq!(
            plan,
            RawFinalizeUiPlan {
                hide_overlay: true,
                disable_capture: false,
            }
        );
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
}
