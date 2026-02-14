use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use toon::devices::DeviceStateSnapshot;
use toon::pipeline::EngineState;

use crate::config::AzadConfig;
use crate::device::{DeviceController, DeviceEvent};
use crate::platform;
use crate::platform::{DeviceMenuModel, DeviceMenuRow, PasteResult};
use crate::preferred_store;
use crate::speech::{SpeechEvent, SpeechSession, spawn_speech_session};

const FINALIZING_SPINNER: [char; 4] = ['|', '/', '-', '\\'];
const DEVICE_SWITCH_RESTART_DEBOUNCE_MS: u64 = 250;

#[derive(Debug, Clone)]
pub enum AppEvent {
    HotkeyPressed,
    HotkeyReleased,
    FinalizeHotkeyPressed,
    MenuToggleAlwaysListening,
    MenuToggleDevices,
    MenuSelectDevice(String),
    MenuOpened,
    MenuClosed,
    OverlayCancel,
    Speech(SpeechEvent),
    Device(DeviceEvent),
}

static EVENT_TX: OnceLock<Sender<AppEvent>> = OnceLock::new();
static EVENT_RX: OnceLock<Mutex<Receiver<AppEvent>>> = OnceLock::new();
static CONTROLLER: OnceLock<Mutex<AppController>> = OnceLock::new();

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
    pasted_this_session: bool,
    latest_draft: String,
    latest_final: Option<String>,
    finalizing_deadline: Option<Instant>,
    finalizing_turn_id: Option<u64>,
    deferred_vad_start: bool,
    accessibility_notice_deadline: Option<Instant>,
    latest_seen_turn_id: u64,
    turn_accept_floor: u64,
    current_turn_id: Option<u64>,
    spinner_index: usize,
    pending_device_switch_target: Option<String>,
    pending_device_switch_deadline: Option<Instant>,
}

impl AppController {
    fn new(cfg: AzadConfig) -> Self {
        let always_listening_enabled = preferred_store::load_always_listening_enabled();
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
            pasted_this_session: false,
            latest_draft: String::new(),
            latest_final: None,
            finalizing_deadline: None,
            finalizing_turn_id: None,
            deferred_vad_start: false,
            accessibility_notice_deadline: None,
            latest_seen_turn_id: 0,
            turn_accept_floor: 1,
            current_turn_id: None,
            spinner_index: 0,
            pending_device_switch_target: None,
            pending_device_switch_deadline: None,
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
            AppEvent::FinalizeHotkeyPressed => self.handle_finalize_hotkey_pressed(),
            AppEvent::MenuToggleAlwaysListening => self.handle_menu_toggle_always_listening(),
            AppEvent::MenuToggleDevices => self.handle_menu_toggle_devices(),
            AppEvent::MenuSelectDevice(device_id) => self.handle_menu_select_device(device_id),
            AppEvent::MenuOpened => self.handle_menu_opened(),
            AppEvent::MenuClosed => self.handle_menu_closed(),
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
        self.latest_final = None;
        self.finalizing_deadline = None;
        self.finalizing_turn_id = None;
        self.deferred_vad_start = false;
        self.accessibility_notice_deadline = None;
        self.pasted_this_session = false;
        self.cancelled = false;
        self.overlay_pending_vad_text = false;
        self.latest_seen_turn_id = 0;
        self.turn_accept_floor = 1;
        self.current_turn_id = None;

        let device_id = self.current_device_id().map(ToOwned::to_owned);
        let emit: Arc<dyn Fn(SpeechEvent) + Send + Sync> =
            Arc::new(|ev| send_event(AppEvent::Speech(ev)));
        match spawn_speech_session(
            session_id,
            self.cfg.to_session_config(
                device_id.clone(),
                self.always_listening_enabled,
                self.always_listening_enabled,
            ),
            emit,
        ) {
            Ok(session) => {
                session.set_auto_vad_enabled(self.always_listening_enabled);
                session.set_capture_enabled(self.always_listening_enabled);
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
        self.manual_hold_active = true;
        self.overlay_pending_vad_text = false;
        self.reset_turn_state();
        self.ensure_session();
        if let Some(session) = &self.session {
            session.set_capture_enabled(true);
            session.start_or_resume_manual_hold();
        }
        self.show_overlay_listening();
    }

    fn handle_hotkey_released(&mut self) {
        self.manual_hold_active = false;
        if let Some(session) = &self.session {
            session.release_manual_hold();
            session.finalize_current_turn();
        }
    }

    fn handle_finalize_hotkey_pressed(&mut self) {
        if !self.overlay_visible {
            return;
        }
        self.manual_hold_active = false;
        if let Some(session) = &self.session {
            session.release_manual_hold();
            session.finalize_current_turn();
        }
    }

    fn handle_menu_toggle_always_listening(&mut self) {
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
        self.cancelled = true;
        self.manual_hold_active = false;
        self.overlay_pending_vad_text = false;
        self.finalizing_deadline = None;
        if let Some(session) = &self.session {
            session.release_manual_hold();
            session.cancel_current_turn();
            if !self.always_listening_enabled {
                session.set_capture_enabled(false);
            }
        }
        self.hide_overlay();
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
            | SpeechEvent::Status { session_id, .. } => *session_id,
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
                    // Keep the current finalizing turn authoritative. We'll open the next
                    // overlay as soon as this turn is fully finalized/pasted.
                    self.deferred_vad_start = true;
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
                if self
                    .finalizing_turn_id
                    .is_some_and(|finalizing_turn_id| turn_id > finalizing_turn_id)
                {
                    // A newer turn started while an older turn is still finalizing.
                    // Defer UI/session rollover until the older turn is fully closed out.
                    self.deferred_vad_start = true;
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
                if self.overlay_visible && self.finalizing_deadline.is_none() {
                    self.render_listening_overlay();
                }
            }
            SpeechEvent::Finalizing {
                turn_id,
                current_draft,
                ..
            } => {
                if !self.accept_turn(turn_id) {
                    return;
                }

                self.observe_turn(turn_id);
                if !current_draft.trim().is_empty() {
                    self.latest_draft = current_draft.trim().to_string();
                }
                self.finalizing_turn_id = Some(turn_id);
                // If we never surfaced any draft text in auto mode, keep overlay hidden.
                // This avoids noise-only VAD turns flashing the overlay.
                let has_visible_text = !self.latest_draft.trim().is_empty();
                if !has_visible_text && !self.overlay_visible && !self.manual_hold_active {
                    self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
                    self.finalizing_deadline = None;
                    return;
                }

                self.overlay_pending_vad_text = false;
                self.finalizing_deadline =
                    Some(Instant::now() + Duration::from_millis(self.cfg.final_pass_timeout_ms));
                self.show_overlay_finalizing();
            }
            SpeechEvent::FinalText { turn_id, text, .. } => {
                if !self.accept_turn(turn_id) {
                    return;
                }
                self.observe_turn(turn_id);

                let cleaned = text.trim().to_string();
                self.finalizing_turn_id = None;
                self.finalizing_deadline = None;
                if cleaned.is_empty() {
                    self.maybe_start_deferred_vad_turn();
                    if !self.always_listening_enabled && !self.manual_hold_active {
                        if let Some(session) = &self.session {
                            session.set_capture_enabled(false);
                        }
                    }
                    return;
                }
                self.latest_final = Some(cleaned.clone());
                if !self.cancelled && !self.pasted_this_session {
                    self.hide_overlay();
                    if self.try_paste(&cleaned) {
                        self.pasted_this_session = true;
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
                if !self.cancelled && !self.pasted_this_session {
                    self.hide_overlay();
                    if let Some(final_text) = self.latest_final.as_ref() {
                        let cleaned = final_text.trim().to_string();
                        if !cleaned.is_empty() {
                            if self.try_paste(&cleaned) {
                                self.pasted_this_session = true;
                            }
                        }
                    }
                }

                self.hide_overlay();
                self.session = None;
                self.latest_draft.clear();
                self.latest_final = None;
                self.finalizing_deadline = None;
                self.finalizing_turn_id = None;
                self.deferred_vad_start = false;
                self.accessibility_notice_deadline = None;
                self.overlay_pending_vad_text = false;
                self.cancelled = false;
                self.pasted_this_session = false;
                self.session_device_id = None;

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
                    platform::set_overlay_content("Error", &message, None);
                }
            }
            SpeechEvent::Status { state, detail, .. } => {
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

                if self.overlay_visible && self.finalizing_deadline.is_none() {
                    let base_status = match state {
                        EngineState::Idle => "Listening",
                        EngineState::Speech => "Capturing speech...",
                    };
                    let status = self.status_with_key_indicator(base_status);
                    let body = if detail.trim().is_empty() {
                        self.latest_draft.as_str()
                    } else {
                        detail.trim()
                    };
                    platform::set_overlay_content(&status, body, None);
                }
            }
        }
    }

    fn on_tick(&mut self) {
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
            let taking_longer = now >= deadline;
            if taking_longer {
                // Keep waiting for the real final-pass completion signal instead of hiding
                // the overlay on a fixed timeout.
                self.finalizing_deadline =
                    Some(now + Duration::from_millis(self.cfg.final_pass_timeout_ms));
            }

            if self.overlay_visible {
                self.spinner_index = (self.spinner_index + 1) % FINALIZING_SPINNER.len();
                let spinner = FINALIZING_SPINNER[self.spinner_index];
                let status = if taking_longer {
                    "Finalizing transcription... (taking longer than usual)"
                } else {
                    "Finalizing transcription..."
                };
                platform::set_overlay_content(status, &self.latest_draft, Some(spinner));
            }
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

    fn show_overlay_finalizing(&mut self) {
        self.overlay_pending_vad_text = false;
        self.accessibility_notice_deadline = None;
        if !self.overlay_visible {
            platform::show_overlay();
            self.overlay_visible = true;
        }
        let spinner = FINALIZING_SPINNER[self.spinner_index % FINALIZING_SPINNER.len()];
        platform::set_overlay_content(
            "Finalizing transcription...",
            &self.latest_draft,
            Some(spinner),
        );
    }

    fn render_listening_overlay(&self) {
        let status = self.status_with_key_indicator("Listening");
        platform::set_overlay_content(&status, &self.latest_draft, None);
    }

    fn show_accessibility_overlay_notice(&mut self) {
        if !self.overlay_visible {
            platform::show_overlay();
            self.overlay_visible = true;
        }
        self.accessibility_notice_deadline = Some(Instant::now() + Duration::from_secs(6));
        platform::set_overlay_content(
            "Auto-paste blocked",
            "Enable Azad in System Settings -> Privacy & Security -> Accessibility",
            None,
        );
    }

    fn try_paste(&mut self, text: &str) -> bool {
        let mut paste_text = text.to_string();
        if !paste_text
            .chars()
            .last()
            .is_some_and(|ch| ch.is_whitespace())
        {
            paste_text.push(' ');
        }

        match platform::paste_text(&paste_text, self.cfg.paste_delay_ms) {
            PasteResult::Pasted => true,
            PasteResult::AccessibilityRequired => {
                self.show_accessibility_overlay_notice();
                false
            }
            PasteResult::EmptyText | PasteResult::ClipboardWriteFailed => false,
        }
    }

    fn hide_overlay(&mut self) {
        self.overlay_pending_vad_text = false;
        if self.overlay_visible {
            platform::hide_overlay();
            self.overlay_visible = false;
        }
    }

    fn status_with_key_indicator(&self, base: &str) -> String {
        if self.manual_hold_active {
            format!("{base}  [Option+Space held]")
        } else {
            base.to_string()
        }
    }

    fn reset_turn_state(&mut self) {
        self.cancelled = false;
        self.pasted_this_session = false;
        self.latest_draft.clear();
        self.latest_final = None;
        self.finalizing_deadline = None;
        self.finalizing_turn_id = None;
        self.deferred_vad_start = false;
        self.accessibility_notice_deadline = None;
        self.overlay_pending_vad_text = false;
        self.current_turn_id = None;
        self.turn_accept_floor = self.latest_seen_turn_id.saturating_add(1);
    }

    fn maybe_start_deferred_vad_turn(&mut self) {
        if !self.deferred_vad_start {
            return;
        }
        self.deferred_vad_start = false;
        self.reset_turn_state();
        self.overlay_pending_vad_text = self.cfg.show_overlay_on_vad_start;
    }

    fn accept_turn(&self, turn_id: u64) -> bool {
        turn_id >= self.turn_accept_floor
    }

    fn observe_turn(&mut self, turn_id: u64) {
        self.latest_seen_turn_id = self.latest_seen_turn_id.max(turn_id);
        self.current_turn_id = Some(turn_id);
    }
}
