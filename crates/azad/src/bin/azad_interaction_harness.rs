//! Process-local interaction runner for Azad shortcut and overlay scenarios.
//!
//! This binary intentionally does not import Azad's AppKit platform module,
//! global-hotkey registration, preferences, microphone, or paste code. Inputs
//! are JSONL messages delivered only to this process. The production
//! interaction reducer is compiled from the same source file used by `azad`.

#![allow(dead_code)]

#[path = "../interaction_sm.rs"]
mod interaction_sm;
#[path = "../platform/hotkeys.rs"]
mod platform_hotkeys;

use anyhow::{Context, Result, bail, ensure};
use interaction_sm::{InteractionEffect, InteractionInput, InteractionState, RuntimeSnapshot};
use platform_hotkeys::{
  ClaimedHoldNavigationAction, SpaceHotkeyAction, claimed_hold_navigation_decision,
  current_mod_mask, space_hotkey_decision,
};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

const KEYCODE_SPACE: u16 = 0x31;
const KEYCODE_ESCAPE: u16 = 0x35;
const KEYCODE_RETURN: u16 = 0x24;
const KEYCODE_ARROW_UP: u16 = 0x7E;
const KEYCODE_ARROW_DOWN: u16 = 0x7D;
const MOD_SHIFT: u8 = 1;
const MOD_CONTROL: u8 = 2;
const MOD_OPTION: u8 = 4;
const MOD_COMMAND: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Key {
  Space,
  ArrowUp,
  ArrowDown,
  Escape,
  Enter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Modifier {
  Option,
  Shift,
  Command,
  Control,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum HarnessInput {
  Initialize {
    #[serde(default)]
    at_ms: u64,
    #[serde(default)]
    always_listening_enabled: bool,
    #[serde(default = "default_listen_modifiers")]
    listen_modifiers: Vec<Modifier>,
    #[serde(default)]
    history_entries: usize,
  },
  KeyDown {
    at_ms: u64,
    key: Key,
    #[serde(default)]
    modifiers: Vec<Modifier>,
    #[serde(default)]
    autorepeat: bool,
  },
  KeyUp {
    at_ms: u64,
    key: Key,
    #[serde(default)]
    modifiers: Vec<Modifier>,
  },
  SpeechStarted {
    at_ms: u64,
  },
  SpeechDraft {
    at_ms: u64,
    text: String,
  },
  SpeechFinalized {
    at_ms: u64,
    text: String,
  },
  SpeechIdle {
    at_ms: u64,
  },
}

impl HarnessInput {
  fn at_ms(&self) -> u64 {
    match self {
      Self::Initialize { at_ms, .. }
      | Self::KeyDown { at_ms, .. }
      | Self::KeyUp { at_ms, .. }
      | Self::SpeechStarted { at_ms }
      | Self::SpeechDraft { at_ms, .. }
      | Self::SpeechFinalized { at_ms, .. }
      | Self::SpeechIdle { at_ms } => *at_ms,
    }
  }
}

fn default_listen_modifiers() -> Vec<Modifier> {
  vec![Modifier::Option]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OverlayKind {
  Hidden,
  Listening,
  History,
  Finalizing,
  ListenEnabled,
  ListenDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum HarnessAction {
  Initialized,
  Ignored,
  ManualHoldStarted,
  ManualHoldReleased,
  CaptureEnabled,
  CaptureDisabled,
  OverlayShown,
  OverlayHidden,
  ListenEnabled,
  ListenDisabled,
  TurnCancelled,
  FinalizeRequested,
  HistoryOpened,
  HistoryClosed,
  HistoryMovedOlder,
  HistoryMovedNewer,
  HistoryPasteRecorded,
  SpeechStarted,
  DraftUpdated,
  FinalTextRecorded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct HarnessState {
  always_listening_enabled: bool,
  capture_enabled: bool,
  manual_hold_active: bool,
  engine_speech_active: bool,
  has_turn_context: bool,
  has_started_turn: bool,
  overlay: OverlayKind,
  draft: String,
  history_browsing: bool,
  history_entries: usize,
  history_selection: usize,
  finalize_requests: u64,
  cancel_requests: u64,
  paste_requests: u64,
}

impl HarnessState {
  fn new(always_listening_enabled: bool, history_entries: usize) -> Self {
    Self {
      always_listening_enabled,
      capture_enabled: always_listening_enabled,
      manual_hold_active: false,
      engine_speech_active: false,
      has_turn_context: false,
      has_started_turn: false,
      overlay: OverlayKind::Hidden,
      draft: String::new(),
      history_browsing: false,
      history_entries,
      history_selection: 0,
      finalize_requests: 0,
      cancel_requests: 0,
      paste_requests: 0,
    }
  }

  fn overlay_visible(&self) -> bool {
    self.overlay != OverlayKind::Hidden
  }

  fn runtime_snapshot(&self) -> RuntimeSnapshot {
    RuntimeSnapshot {
      always_listening_enabled: self.always_listening_enabled,
      has_active_speech_turn: self.engine_speech_active,
      has_turn_context: self.has_turn_context,
      has_started_turn: self.has_started_turn,
      overlay_visible: self.overlay_visible(),
      manual_hold_active: self.manual_hold_active,
    }
  }

  fn clear_turn(&mut self) {
    self.engine_speech_active = false;
    self.has_turn_context = false;
    self.has_started_turn = false;
    self.draft.clear();
  }
}

#[derive(Debug, Clone, Serialize)]
struct HarnessOutput {
  sequence: usize,
  at_ms: u64,
  input: HarnessInput,
  actions: Vec<HarnessAction>,
  state: HarnessState,
}

struct Harness {
  interaction: InteractionState,
  state: HarnessState,
  listen_modifiers: Vec<Modifier>,
  space_claimed: bool,
  last_at_ms: u64,
  sequence: usize,
}

impl Default for Harness {
  fn default() -> Self {
    Self::new(false, default_listen_modifiers(), 0)
  }
}

impl Harness {
  fn new(
    always_listening_enabled: bool,
    listen_modifiers: Vec<Modifier>,
    history_entries: usize,
  ) -> Self {
    Self {
      interaction: InteractionState::default(),
      state: HarnessState::new(always_listening_enabled, history_entries),
      listen_modifiers,
      space_claimed: false,
      last_at_ms: 0,
      sequence: 0,
    }
  }

  fn handle(&mut self, input: HarnessInput) -> Result<HarnessOutput> {
    let at_ms = input.at_ms();
    ensure!(
      at_ms >= self.last_at_ms,
      "event timestamp moved backwards: {} < {}",
      at_ms,
      self.last_at_ms
    );
    self.last_at_ms = at_ms;

    let mut actions = Vec::new();
    match &input {
      HarnessInput::Initialize {
        always_listening_enabled,
        listen_modifiers,
        history_entries,
        ..
      } => {
        ensure!(!listen_modifiers.is_empty(), "listen_modifiers must not be empty");
        let sequence = self.sequence;
        *self = Self::new(*always_listening_enabled, listen_modifiers.clone(), *history_entries);
        self.sequence = sequence;
        self.last_at_ms = at_ms;
        actions.push(HarnessAction::Initialized);
      }
      HarnessInput::KeyDown { key, modifiers, autorepeat, .. } => {
        self.handle_key_down(*key, modifiers, at_ms, *autorepeat, &mut actions)
      }
      HarnessInput::KeyUp { key, modifiers, .. } => {
        self.handle_key_up(*key, modifiers, &mut actions)
      }
      HarnessInput::SpeechStarted { .. } => {
        self.state.engine_speech_active = true;
        self.state.has_turn_context = true;
        actions.push(HarnessAction::SpeechStarted);
      }
      HarnessInput::SpeechDraft { text, .. } => {
        self.state.engine_speech_active = true;
        self.state.has_turn_context = true;
        self.state.draft.clone_from(text);
        if !text.trim().is_empty() {
          self.state.has_started_turn = true;
          if !self.state.overlay_visible() {
            self.state.overlay = OverlayKind::Listening;
            actions.push(HarnessAction::OverlayShown);
          }
        }
        actions.push(HarnessAction::DraftUpdated);
      }
      HarnessInput::SpeechFinalized { text, .. } => {
        let _ = self.interaction.reduce(InteractionInput::SpeechFinalized);
        if !text.trim().is_empty() {
          self.state.paste_requests += 1;
          actions.push(HarnessAction::FinalTextRecorded);
        }
        self.state.manual_hold_active = false;
        self.state.capture_enabled = self.state.always_listening_enabled;
        self.state.clear_turn();
        self.hide_overlay(&mut actions);
      }
      HarnessInput::SpeechIdle { .. } => {
        let _ = self.interaction.reduce(InteractionInput::SpeechIdle {
          manual_hold_active: self.state.manual_hold_active,
        });
        self.state.engine_speech_active = false;
      }
    }

    let output =
      HarnessOutput { sequence: self.sequence, at_ms, input, actions, state: self.state.clone() };
    self.sequence += 1;
    Ok(output)
  }

  fn handle_key_down(
    &mut self,
    key: Key,
    modifiers: &[Modifier],
    at_ms: u64,
    is_autorepeat: bool,
    actions: &mut Vec<HarnessAction>,
  ) {
    if key == Key::Space {
      let decision = space_hotkey_decision(
        modifier_mask(&self.listen_modifiers),
        self.space_claimed,
        modifier_mask(modifiers),
        true,
        is_autorepeat,
      );
      self.space_claimed = decision.claimed_after;
      match decision.action {
        SpaceHotkeyAction::Press => {
          let effects = self.interaction.reduce(InteractionInput::HoldPressed {
            now_ms: at_ms,
            snapshot: self.state.runtime_snapshot(),
          });
          self.apply_effects(effects, actions);
        }
        SpaceHotkeyAction::PassThrough | SpaceHotkeyAction::ClaimOnly => {
          actions.push(HarnessAction::Ignored);
        }
        SpaceHotkeyAction::Release { .. } => unreachable!("keydown produced a release"),
      }
      return;
    }

    match claimed_hold_navigation_decision(self.space_claimed, keycode(key), true) {
      ClaimedHoldNavigationAction::Navigate(-1) => {
        if self.state.manual_hold_active && self.state.overlay_visible() {
          self.enter_history(actions);
        } else {
          actions.push(HarnessAction::Ignored);
        }
        return;
      }
      ClaimedHoldNavigationAction::Navigate(_) => unreachable!("unsupported navigation direction"),
      ClaimedHoldNavigationAction::ClaimOnly => {
        actions.push(HarnessAction::Ignored);
        return;
      }
      ClaimedHoldNavigationAction::PassThrough => {}
    }

    match key {
      Key::ArrowUp if self.state.history_browsing => {
        if self.state.history_selection + 1 < self.state.history_entries {
          self.state.history_selection += 1;
        }
        actions.push(HarnessAction::HistoryMovedOlder);
      }
      Key::ArrowDown if self.state.history_browsing => {
        self.state.history_selection = self.state.history_selection.saturating_sub(1);
        actions.push(HarnessAction::HistoryMovedNewer);
      }
      Key::Escape if self.state.history_browsing => self.exit_history(actions),
      Key::Escape if self.state.overlay_visible() => {
        let _ = self.interaction.reduce(InteractionInput::OverlayCancelled);
        self.state.cancel_requests += 1;
        self.state.manual_hold_active = false;
        self.state.capture_enabled = self.state.always_listening_enabled;
        self.state.clear_turn();
        actions.push(HarnessAction::TurnCancelled);
        self.hide_overlay(actions);
      }
      Key::Enter if self.state.history_browsing => {
        if self.state.history_entries > 0 {
          self.state.paste_requests += 1;
          actions.push(HarnessAction::HistoryPasteRecorded);
        }
        self.exit_history(actions);
      }
      Key::Enter => {
        let effects = self.interaction.reduce(InteractionInput::FinalizePressed {
          overlay_visible: self.state.overlay_visible(),
        });
        self.apply_effects(effects, actions);
      }
      _ => actions.push(HarnessAction::Ignored),
    }
  }

  fn handle_key_up(&mut self, key: Key, modifiers: &[Modifier], actions: &mut Vec<HarnessAction>) {
    if key != Key::Space {
      if matches!(
        claimed_hold_navigation_decision(self.space_claimed, keycode(key), false),
        ClaimedHoldNavigationAction::ClaimOnly
      ) {
        actions.push(HarnessAction::Ignored);
        return;
      }
      actions.push(HarnessAction::Ignored);
      return;
    }

    let decision = space_hotkey_decision(
      modifier_mask(&self.listen_modifiers),
      self.space_claimed,
      modifier_mask(modifiers),
      false,
      false,
    );
    self.space_claimed = decision.claimed_after;
    let SpaceHotkeyAction::Release { .. } = decision.action else {
      actions.push(HarnessAction::Ignored);
      return;
    };
    if self.state.history_browsing {
      actions.push(HarnessAction::Ignored);
      return;
    }
    let effects = self
      .interaction
      .reduce(InteractionInput::HoldReleased { snapshot: self.state.runtime_snapshot() });
    self.apply_effects(effects, actions);
  }

  fn apply_effects(&mut self, effects: Vec<InteractionEffect>, actions: &mut Vec<HarnessAction>) {
    for effect in effects {
      match effect {
        InteractionEffect::InterruptAndToggleAlwaysListening => {
          self.state.cancel_requests += 1;
          self.state.manual_hold_active = false;
          self.state.clear_turn();
          actions.push(HarnessAction::TurnCancelled);
          self.state.always_listening_enabled = !self.state.always_listening_enabled;
          self.state.capture_enabled = self.state.always_listening_enabled;
          if self.state.always_listening_enabled {
            self.state.overlay = OverlayKind::ListenEnabled;
            actions.push(HarnessAction::ListenEnabled);
          } else {
            self.state.overlay = OverlayKind::ListenDisabled;
            actions.push(HarnessAction::ListenDisabled);
          }
          actions.push(HarnessAction::OverlayShown);
        }
        InteractionEffect::MenuToggleAlwaysListening => {
          self.state.always_listening_enabled = !self.state.always_listening_enabled;
          self.state.capture_enabled = self.state.always_listening_enabled;
          if self.state.always_listening_enabled {
            self.state.overlay = OverlayKind::ListenEnabled;
            actions.push(HarnessAction::ListenEnabled);
          } else {
            self.state.overlay = OverlayKind::ListenDisabled;
            actions.push(HarnessAction::ListenDisabled);
          }
          actions.push(HarnessAction::OverlayShown);
        }
        InteractionEffect::ActivateManualHold { reset_turn_state, .. } => {
          if reset_turn_state {
            self.state.clear_turn();
          }
          self.state.manual_hold_active = true;
          if !self.state.capture_enabled {
            self.state.capture_enabled = true;
            actions.push(HarnessAction::CaptureEnabled);
          }
          self.state.overlay = OverlayKind::Listening;
          actions.push(HarnessAction::ManualHoldStarted);
          actions.push(HarnessAction::OverlayShown);
        }
        InteractionEffect::ReleaseManualHold { should_finalize, has_started_turn } => {
          self.state.manual_hold_active = false;
          actions.push(HarnessAction::ManualHoldReleased);
          if should_finalize && has_started_turn {
            self.state.capture_enabled = true;
            self.state.finalize_requests += 1;
            self.state.overlay = OverlayKind::Finalizing;
            actions.push(HarnessAction::FinalizeRequested);
          } else if should_finalize {
            self.state.capture_enabled = self.state.always_listening_enabled;
            self.state.clear_turn();
            if !self.state.capture_enabled {
              actions.push(HarnessAction::CaptureDisabled);
            }
            self.hide_overlay(actions);
          } else {
            self.state.capture_enabled = self.state.always_listening_enabled;
            if !self.state.capture_enabled {
              actions.push(HarnessAction::CaptureDisabled);
            }
          }
        }
        InteractionEffect::FinalizeFromHotkey => {
          self.state.manual_hold_active = false;
          self.state.finalize_requests += 1;
          self.state.overlay = OverlayKind::Finalizing;
          actions.push(HarnessAction::FinalizeRequested);
        }
      }
    }
  }

  fn enter_history(&mut self, actions: &mut Vec<HarnessAction>) {
    self.state.manual_hold_active = false;
    self.state.capture_enabled = false;
    self.state.clear_turn();
    self.state.history_browsing = true;
    self.state.history_selection = 0;
    self.state.overlay = OverlayKind::History;
    actions.push(HarnessAction::HistoryOpened);
    actions.push(HarnessAction::CaptureDisabled);
    actions.push(HarnessAction::OverlayShown);
  }

  fn exit_history(&mut self, actions: &mut Vec<HarnessAction>) {
    self.state.history_browsing = false;
    self.state.history_selection = 0;
    self.state.capture_enabled = self.state.always_listening_enabled;
    actions.push(HarnessAction::HistoryClosed);
    self.hide_overlay(actions);
  }

  fn hide_overlay(&mut self, actions: &mut Vec<HarnessAction>) {
    if self.state.overlay_visible() {
      self.state.overlay = OverlayKind::Hidden;
      actions.push(HarnessAction::OverlayHidden);
    }
  }
}

#[derive(Debug, Serialize)]
struct ScenarioResult<'a> {
  scenario: &'a str,
  passed: bool,
}

fn key_down(at_ms: u64, key: Key, modifiers: Vec<Modifier>) -> HarnessInput {
  HarnessInput::KeyDown { at_ms, key, modifiers, autorepeat: false }
}

fn key_up(at_ms: u64, key: Key, modifiers: Vec<Modifier>) -> HarnessInput {
  HarnessInput::KeyUp { at_ms, key, modifiers }
}

fn option() -> Vec<Modifier> {
  vec![Modifier::Option]
}

fn modifier_mask(modifiers: &[Modifier]) -> u8 {
  current_mod_mask(
    modifiers.contains(&Modifier::Option),
    modifiers.contains(&Modifier::Shift),
    modifiers.contains(&Modifier::Command),
    modifiers.contains(&Modifier::Control),
  )
}

fn keycode(key: Key) -> u16 {
  match key {
    Key::Space => KEYCODE_SPACE,
    Key::ArrowUp => KEYCODE_ARROW_UP,
    Key::ArrowDown => KEYCODE_ARROW_DOWN,
    Key::Escape => KEYCODE_ESCAPE,
    Key::Enter => KEYCODE_RETURN,
  }
}

fn run_self_test() -> Result<()> {
  let stdout = io::stdout();
  let mut out = BufWriter::new(stdout.lock());

  let mut manual = Harness::new(false, option(), 3);
  let pressed = manual.handle(key_down(1_000, Key::Space, option()))?;
  ensure!(pressed.state.manual_hold_active, "manual hold did not activate");
  ensure!(pressed.state.capture_enabled, "manual hold did not enable capture");
  ensure!(
    pressed.state.overlay == OverlayKind::Listening,
    "manual hold did not show the listening overlay before text"
  );
  let released = manual.handle(key_up(1_800, Key::Space, option()))?;
  ensure!(!released.state.manual_hold_active, "manual hold did not release");
  ensure!(released.state.overlay == OverlayKind::Hidden, "empty hold overlay did not hide");
  write_json_line(&mut out, &ScenarioResult { scenario: "manual_hold_overlay", passed: true })?;

  let mut spoken = Harness::new(false, option(), 0);
  let _ = spoken.handle(key_down(2_000, Key::Space, option()))?;
  let _ = spoken.handle(HarnessInput::SpeechDraft {
    at_ms: 2_300,
    text: "hello from the isolated harness".to_string(),
  })?;
  let finalizing = spoken.handle(key_up(3_000, Key::Space, option()))?;
  ensure!(finalizing.state.overlay == OverlayKind::Finalizing, "spoken hold did not finalize");
  ensure!(finalizing.state.finalize_requests == 1, "spoken hold finalized wrong number of times");
  write_json_line(&mut out, &ScenarioResult { scenario: "manual_hold_finalize", passed: true })?;

  let mut toggle = Harness::new(false, option(), 0);
  let _ = toggle.handle(key_down(4_000, Key::Space, option()))?;
  let _ = toggle.handle(key_up(4_040, Key::Space, option()))?;
  let toggled = toggle.handle(key_down(4_120, Key::Space, option()))?;
  let _ = toggle.handle(key_up(4_160, Key::Space, option()))?;
  ensure!(toggled.state.always_listening_enabled, "double tap did not enable listening");
  ensure!(toggled.state.capture_enabled, "double tap did not keep capture enabled");
  ensure!(toggled.state.finalize_requests == 0, "double tap incorrectly finalized a turn");
  write_json_line(&mut out, &ScenarioResult { scenario: "double_tap_toggle", passed: true })?;

  let mut history = Harness::new(false, option(), 4);
  let _ = history.handle(key_down(5_000, Key::Space, option()))?;
  let opened = history.handle(key_down(5_100, Key::ArrowUp, option()))?;
  ensure!(opened.state.history_browsing, "hold+Up did not enter history");
  ensure!(opened.state.overlay == OverlayKind::History, "history overlay was not recorded");
  ensure!(!opened.state.capture_enabled, "history did not stop capture");
  let released = history.handle(key_up(5_200, Key::Space, option()))?;
  ensure!(released.state.history_browsing, "Space release unexpectedly closed history");
  let closed = history.handle(key_down(5_300, Key::Escape, Vec::new()))?;
  ensure!(!closed.state.history_browsing, "Escape did not close history");
  ensure!(closed.state.overlay == OverlayKind::Hidden, "Escape left history overlay visible");
  write_json_line(&mut out, &ScenarioResult { scenario: "history_navigation", passed: true })?;

  let mut cancel = Harness::new(false, option(), 0);
  let _ = cancel.handle(key_down(6_000, Key::Space, option()))?;
  let cancelled = cancel.handle(key_down(6_100, Key::Escape, Vec::new()))?;
  ensure!(cancelled.state.cancel_requests == 1, "Escape did not record cancellation");
  ensure!(cancelled.state.overlay == OverlayKind::Hidden, "Escape left overlay visible");
  ensure!(!cancelled.state.capture_enabled, "Escape left manual capture enabled");
  write_json_line(&mut out, &ScenarioResult { scenario: "cancel_manual_hold", passed: true })?;

  let mut enter = Harness::new(false, option(), 0);
  let _ = enter.handle(key_down(6_500, Key::Space, option()))?;
  let _ = enter
    .handle(HarnessInput::SpeechDraft { at_ms: 6_600, text: "finish this turn".to_string() })?;
  let finalizing = enter.handle(key_down(6_700, Key::Enter, Vec::new()))?;
  ensure!(finalizing.state.overlay == OverlayKind::Finalizing, "Enter did not start finalizing");
  ensure!(finalizing.state.finalize_requests == 1, "Enter finalized the wrong number of times");
  let finalized = enter
    .handle(HarnessInput::SpeechFinalized { at_ms: 6_800, text: "finish this turn".to_string() })?;
  ensure!(finalized.state.overlay == OverlayKind::Hidden, "final text left the overlay visible");
  ensure!(!finalized.state.manual_hold_active, "final text left the manual hold active");
  let released = enter.handle(key_up(6_900, Key::Space, option()))?;
  ensure!(released.state.overlay == OverlayKind::Hidden, "late Space release restored the overlay");
  write_json_line(&mut out, &ScenarioResult { scenario: "enter_finalize_cleanup", passed: true })?;

  let mut assist = Harness::new(true, option(), 0);
  let _ = assist.handle(HarnessInput::SpeechStarted { at_ms: 7_000 })?;
  let _ = assist
    .handle(HarnessInput::SpeechDraft { at_ms: 7_100, text: "ongoing vad turn".to_string() })?;
  let _ = assist.handle(key_down(7_200, Key::Space, option()))?;
  let released = assist.handle(key_up(7_800, Key::Space, option()))?;
  ensure!(released.state.capture_enabled, "VAD assist release disabled capture");
  ensure!(released.state.finalize_requests == 0, "VAD assist release incorrectly finalized");
  write_json_line(&mut out, &ScenarioResult { scenario: "always_listening_assist", passed: true })?;

  write_json_line(&mut out, &serde_json::json!({ "summary": "ok", "scenarios": 7 }))?;
  out.flush()?;
  Ok(())
}

fn run_jsonl(path: Option<&Path>) -> Result<()> {
  let input: Box<dyn BufRead> = match path {
    Some(path) => Box::new(BufReader::new(
      File::open(path).with_context(|| format!("open {}", path.display()))?,
    )),
    None => Box::new(BufReader::new(io::stdin().lock())),
  };
  let stdout = io::stdout();
  let mut out = BufWriter::new(stdout.lock());
  let mut harness = Harness::default();

  for (line_index, line) in input.lines().enumerate() {
    let line = line.with_context(|| format!("read JSONL line {}", line_index + 1))?;
    if line.trim().is_empty() {
      continue;
    }
    let event: HarnessInput = serde_json::from_str(&line)
      .with_context(|| format!("parse JSONL line {}", line_index + 1))?;
    let output = harness
      .handle(event)
      .with_context(|| format!("handle JSONL line {}", line_index + 1))?;
    write_json_line(&mut out, &output)?;
  }
  out.flush()?;
  Ok(())
}

fn write_json_line(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
  serde_json::to_writer(&mut *writer, value)?;
  writer.write_all(b"\n")?;
  Ok(())
}

fn print_description() -> Result<()> {
  let stdout = io::stdout();
  let mut out = BufWriter::new(stdout.lock());
  write_json_line(
    &mut out,
    &serde_json::json!({
      "process_local_events": true,
      "registers_global_hotkeys": false,
      "posts_core_graphics_events": false,
      "opens_appkit_windows": false,
      "reads_or_writes_user_defaults": false,
      "opens_microphone": false,
      "performs_paste": false,
      "input": "jsonl",
      "commands": ["describe", "self-test", "run [path]"]
    }),
  )?;
  out.flush()?;
  Ok(())
}

fn main() -> Result<()> {
  let mut args = std::env::args().skip(1);
  match args.next().as_deref() {
    Some("describe") => print_description(),
    Some("self-test") => run_self_test(),
    Some("run") => {
      let path = args.next();
      if args.next().is_some() {
        bail!("usage: azad-interaction-harness run [jsonl-path]");
      }
      run_jsonl(path.as_deref().map(Path::new))
    }
    _ => bail!("usage: azad-interaction-harness <describe|self-test|run [jsonl-path]>"),
  }
}
