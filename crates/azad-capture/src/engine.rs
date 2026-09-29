//! Report-level normalization between seized keyboards and the virtual output keyboard.
//!
//! IOKit delivers one value per changed element; every value from one input report shares a
//! timestamp. Classifying element by element would see `Space` before `Option` when both change
//! in one report, so the engine applies a whole batch at once and orders the resulting edges:
//! modifier presses, key releases, key presses, then modifier releases.

use std::collections::{BTreeMap, HashMap};

use crate::policy::{KeyAction, KeyContext, KeyPolicy, modifier_bit};

pub const PAGE_GENERIC_DESKTOP: u32 = 0x01;
pub const PAGE_KEYBOARD: u32 = 0x07;
pub const PAGE_BUTTON: u32 = 0x09;
pub const PAGE_CONSUMER: u32 = 0x0C;
pub const PAGE_APPLE_TOP_CASE: u32 = 0x00FF;
pub const PAGE_APPLE_KEYBOARD: u32 = 0xFF01;

/// The virtual keyboard report holds at most this many simultaneous keys per page.
pub const MAX_REPORT_KEYS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Usage {
  pub page: u32,
  pub usage: u32,
}

impl Usage {
  pub const fn new(page: u32, usage: u32) -> Self {
    Self { page, usage }
  }

  /// Buttons of a pointer collection on a seized keyboard (e.g. mouse keys).
  pub fn is_pointer_button(self) -> bool {
    self.page == PAGE_BUTTON && (1..=32).contains(&self.usage)
  }

  /// Relative motion and scroll of a pointer collection on a seized keyboard.
  pub fn is_pointer_axis(self) -> bool {
    matches!(
      (self.page, self.usage),
      (PAGE_GENERIC_DESKTOP, 0x30 | 0x31 | 0x38) | (PAGE_CONSUMER, 0x238)
    )
  }

  /// True for key usages the helper tracks and forwards; pointer usages are handled
  /// separately and everything else on a seized keyboard is dropped (LED outputs, error
  /// roll-over, vendor diagnostics).
  pub fn is_forwardable(self) -> bool {
    match self.page {
      PAGE_KEYBOARD => (0x04..=0xE7).contains(&self.usage),
      // System controls: power, sleep, wake, Do Not Disturb and related keys.
      PAGE_GENERIC_DESKTOP => (0x81..=0xB7).contains(&self.usage),
      PAGE_CONSUMER | PAGE_APPLE_TOP_CASE | PAGE_APPLE_KEYBOARD => {
        self.usage != 0 && self.usage <= u16::MAX as u32
      }
      _ => false,
    }
  }
}

/// Apple top case "keyboard fn" (the fn/Globe key on Apple keyboards).
pub const APPLE_FN: Usage = Usage::new(PAGE_APPLE_TOP_CASE, 0x03);

/// Per-device key translations the OS would have applied to that keyboard. Apple keyboards
/// publish `FnFunctionUsageMap` (F-row to media/brightness keys) and `FnKeyboardUsageMap`
/// (fn+arrow to Home/End and similar) on their HID service; the OS applies them per device, so
/// once the device is seized the helper must apply them before forwarding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceTranslation {
  pub fn_function: HashMap<Usage, Usage>,
  pub fn_keyboard: HashMap<Usage, Usage>,
  /// The "Use F1, F2, etc. keys as standard function keys" setting for this keyboard.
  pub standard_function_keys: bool,
}

impl DeviceTranslation {
  /// Parses a usage map property: comma-separated `0xPPPPUUUU` pairs, from then to.
  pub fn parse_map(text: &str) -> HashMap<Usage, Usage> {
    let values: Vec<u32> = text
      .split(',')
      .filter_map(|item| u32::from_str_radix(item.trim().trim_start_matches("0x"), 16).ok())
      .collect();
    values
      .chunks_exact(2)
      .map(|pair| (usage_from_packed(pair[0]), usage_from_packed(pair[1])))
      .collect()
  }

  fn translate(&self, usage: Usage, fn_held: bool) -> Usage {
    if fn_held && let Some(target) = self.fn_keyboard.get(&usage) {
      return *target;
    }
    match self.fn_function.get(&usage) {
      Some(target) if fn_held == self.standard_function_keys => *target,
      _ => usage,
    }
  }
}

fn usage_from_packed(value: u32) -> Usage {
  Usage::new(value >> 16, value & 0xffff)
}

/// Pointer state to post on the virtual pointing device: held buttons and this batch's motion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PointerReport {
  /// Bit n-1 set while button n is held.
  pub buttons: u32,
  pub x: i32,
  pub y: i32,
  pub wheel: i32,
  pub horizontal_wheel: i32,
}

/// Capture time of a batch, in nanoseconds on the host's monotonic clock.
pub type Timestamp = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedAction {
  pub action: KeyAction,
  pub timestamp: Timestamp,
}

/// The pressed state to present on the virtual keyboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardState {
  pub modifiers: u8,
  pub keyboard: Vec<u16>,
  pub consumer: Vec<u16>,
  pub apple_top_case: Vec<u16>,
  pub apple_keyboard: Vec<u16>,
  pub generic_desktop: Vec<u16>,
}

impl ForwardState {
  pub fn is_empty(&self) -> bool {
    *self == ForwardState::default()
  }
}

#[derive(Debug, Default)]
pub struct BatchOutcome {
  pub actions: Vec<TimedAction>,
  /// Present when the forwarded state changed and a new virtual report must be posted.
  pub forward: Option<ForwardState>,
  /// Present when a pointer collection on a seized keyboard changed.
  pub pointer: Option<PointerReport>,
  pub claimed_edges: u32,
  pub forwarded_edges: u32,
}

#[derive(Debug, Default)]
pub struct Engine {
  /// Per device: each held physical usage and the usage it was forwarded as.
  devices: HashMap<u64, BTreeMap<Usage, Usage>>,
  translations: HashMap<u64, DeviceTranslation>,
  /// Per device: held pointer buttons.
  buttons: HashMap<u64, u32>,
  pressed: BTreeMap<Usage, u32>,
  policy: KeyPolicy,
  last_forward: ForwardState,
}

#[derive(Debug, Clone, Copy)]
struct Edge {
  usage: Usage,
  down: bool,
}

impl Engine {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn forward_state(&self) -> &ForwardState {
    &self.last_forward
  }

  pub fn set_translation(&mut self, device: u64, translation: DeviceTranslation) {
    self.translations.insert(device, translation);
  }

  /// Applies one report's worth of element values from `device`. A key keeps the translation
  /// chosen at its press, so its release matches even if fn changed in between.
  pub fn apply_batch(
    &mut self,
    device: u64,
    values: &[(Usage, i64)],
    timestamp: Timestamp,
    context: &KeyContext,
  ) -> BatchOutcome {
    let pointer = self.apply_pointer(device, values);
    let held = self.devices.entry(device).or_default();
    let fn_released = values.iter().any(|&(usage, value)| usage == APPLE_FN && value == 0);
    let fn_pressed = values.iter().any(|&(usage, value)| usage == APPLE_FN && value != 0);
    let fn_held = fn_pressed || (held.contains_key(&APPLE_FN) && !fn_released);
    let translation = self.translations.get(&device);
    let mut edges = Vec::new();
    for &(physical, value) in values {
      if !physical.is_forwardable() {
        continue;
      }
      let down = value != 0;
      let usage = if down {
        if held.contains_key(&physical) {
          continue;
        }
        let usage = translation.map_or(physical, |t| t.translate(physical, fn_held));
        held.insert(physical, usage);
        usage
      } else {
        match held.remove(&physical) {
          Some(usage) => usage,
          None => continue,
        }
      };
      if !usage.is_forwardable() {
        continue;
      }
      let count = self.pressed.entry(usage).or_insert(0);
      if down {
        *count += 1;
        if *count == 1 {
          edges.push(Edge { usage, down: true });
        }
      } else {
        *count = count.saturating_sub(1);
        if *count == 0 {
          self.pressed.remove(&usage);
          edges.push(Edge { usage, down: false });
        }
      }
    }
    let mut outcome = self.run_edges(edges, timestamp, context);
    outcome.pointer = pointer;
    outcome
  }

  fn apply_pointer(&mut self, device: u64, values: &[(Usage, i64)]) -> Option<PointerReport> {
    let mut report = PointerReport::default();
    let mut changed = false;
    for &(usage, value) in values {
      if usage.is_pointer_button() {
        let bit = 1u32 << (usage.usage - 1);
        let buttons = self.buttons.entry(device).or_default();
        if value != 0 {
          *buttons |= bit;
        } else {
          *buttons &= !bit;
        }
        changed = true;
      } else if usage.is_pointer_axis() {
        let delta = value.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        match (usage.page, usage.usage) {
          (PAGE_GENERIC_DESKTOP, 0x30) => report.x = report.x.saturating_add(delta),
          (PAGE_GENERIC_DESKTOP, 0x31) => report.y = report.y.saturating_add(delta),
          (PAGE_GENERIC_DESKTOP, 0x38) => report.wheel = report.wheel.saturating_add(delta),
          _ => report.horizontal_wheel = report.horizontal_wheel.saturating_add(delta),
        }
        changed = true;
      }
    }
    changed.then(|| PointerReport { buttons: self.held_buttons(), ..report })
  }

  fn held_buttons(&self) -> u32 {
    self.buttons.values().fold(0, |held, buttons| held | buttons)
  }

  /// Releases everything `device` held, as if it sent an all-up report.
  pub fn remove_device(
    &mut self,
    device: u64,
    timestamp: Timestamp,
    context: &KeyContext,
  ) -> BatchOutcome {
    self.translations.remove(&device);
    let pointer = self
      .buttons
      .remove(&device)
      .filter(|buttons| *buttons != 0)
      .map(|_| PointerReport { buttons: self.held_buttons(), ..PointerReport::default() });
    let Some(held) = self.devices.remove(&device) else {
      return BatchOutcome { pointer, ..BatchOutcome::default() };
    };
    let mut edges = Vec::new();
    for usage in held.into_values() {
      if let Some(count) = self.pressed.get_mut(&usage) {
        *count -= 1;
        if *count == 0 {
          self.pressed.remove(&usage);
          edges.push(Edge { usage, down: false });
        }
      }
    }
    let mut outcome = self.run_edges(edges, timestamp, context);
    outcome.pointer = pointer;
    outcome
  }

  /// Drops all device state and claims, e.g. when forwarding stops. Returns the actions that
  /// finish any claimed gesture so the app does not stay in a held state.
  pub fn reset(&mut self, timestamp: Timestamp) -> BatchOutcome {
    self.devices.clear();
    self.pressed.clear();
    let had_buttons = self.held_buttons() != 0;
    self.buttons.clear();
    let actions = self
      .policy
      .release_all()
      .into_iter()
      .map(|action| TimedAction { action, timestamp })
      .collect();
    let forward = if self.last_forward.is_empty() {
      None
    } else {
      self.last_forward = ForwardState::default();
      Some(ForwardState::default())
    };
    let pointer = had_buttons.then(PointerReport::default);
    BatchOutcome { actions, forward, pointer, ..BatchOutcome::default() }
  }

  fn run_edges(
    &mut self,
    mut edges: Vec<Edge>,
    timestamp: Timestamp,
    context: &KeyContext,
  ) -> BatchOutcome {
    edges.sort_by_key(|edge| {
      let modifier = is_modifier(edge.usage);
      match (modifier, edge.down) {
        (true, true) => 0,
        (false, false) => 1,
        (false, true) => 2,
        (true, false) => 3,
      }
    });
    let mut outcome = BatchOutcome::default();
    let mut modifiers = self.modifiers_before(&edges);
    for edge in edges {
      if is_modifier(edge.usage) {
        let bit = modifier_bit(edge.usage.usage as u16).unwrap_or(0);
        modifiers = if edge.down { modifiers | bit } else { self.live_modifiers() };
        outcome.forwarded_edges += 1;
        continue;
      }
      if edge.usage.page != PAGE_KEYBOARD {
        outcome.forwarded_edges += 1;
        continue;
      }
      let key = edge.usage.usage as u16;
      let decision = if edge.down {
        self.policy.key_down(key, modifiers, context)
      } else {
        self.policy.key_up(key, modifiers)
      };
      if decision.claimed {
        outcome.claimed_edges += 1;
      } else {
        outcome.forwarded_edges += 1;
      }
      if let Some(action) = decision.action {
        outcome.actions.push(TimedAction { action, timestamp });
      }
    }
    let forward = self.compute_forward();
    if forward != self.last_forward {
      self.last_forward = forward.clone();
      outcome.forward = Some(forward);
    }
    outcome
  }

  /// Modifier state before this batch's modifier presses: pressed modifiers minus the ones
  /// this batch presses, plus the ones it releases (they are still held until the last step).
  fn modifiers_before(&self, edges: &[Edge]) -> u8 {
    let mut modifiers = self.live_modifiers();
    for edge in edges.iter().filter(|edge| is_modifier(edge.usage)) {
      let bit = modifier_bit(edge.usage.usage as u16).unwrap_or(0);
      if edge.down {
        modifiers &= !bit;
        // Another held modifier of the same kind keeps the bit set.
        if self.pressed.keys().any(|usage| {
          *usage != edge.usage
            && is_modifier(*usage)
            && modifier_bit(usage.usage as u16) == Some(bit)
        }) {
          modifiers |= bit;
        }
      } else {
        modifiers |= bit;
      }
    }
    modifiers
  }

  fn live_modifiers(&self) -> u8 {
    self
      .pressed
      .keys()
      .filter(|usage| is_modifier(**usage))
      .filter_map(|usage| modifier_bit(usage.usage as u16))
      .fold(0, |acc, bit| acc | bit)
  }

  fn compute_forward(&self) -> ForwardState {
    let mut state = ForwardState::default();
    for usage in self.pressed.keys() {
      let value = usage.usage as u16;
      match usage.page {
        PAGE_KEYBOARD if is_modifier(*usage) => state.modifiers |= 1 << (value - 0xE0),
        PAGE_KEYBOARD if !self.policy.is_claimed(value) => push_key(&mut state.keyboard, value),
        PAGE_KEYBOARD => {}
        PAGE_CONSUMER => push_key(&mut state.consumer, value),
        PAGE_APPLE_TOP_CASE => push_key(&mut state.apple_top_case, value),
        PAGE_APPLE_KEYBOARD => push_key(&mut state.apple_keyboard, value),
        PAGE_GENERIC_DESKTOP => push_key(&mut state.generic_desktop, value),
        _ => {}
      }
    }
    state
  }
}

fn is_modifier(usage: Usage) -> bool {
  usage.page == PAGE_KEYBOARD && (0xE0..=0xE7).contains(&usage.usage)
}

fn push_key(keys: &mut Vec<u16>, value: u16) {
  if keys.len() < MAX_REPORT_KEYS {
    keys.push(value);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::policy::{MOD_OPTION, usage};

  const OPTION: Usage = Usage::new(PAGE_KEYBOARD, usage::LEFT_OPTION as u32);
  const SHIFT: Usage = Usage::new(PAGE_KEYBOARD, usage::LEFT_SHIFT as u32);
  const SPACE: Usage = Usage::new(PAGE_KEYBOARD, usage::SPACE as u32);
  const UP: Usage = Usage::new(PAGE_KEYBOARD, usage::UP_ARROW as u32);
  const A: Usage = Usage::new(PAGE_KEYBOARD, usage::A as u32);
  const RETURN: Usage = Usage::new(PAGE_KEYBOARD, usage::RETURN as u32);
  const VOLUME_UP: Usage = Usage::new(PAGE_CONSUMER, 0xE9);
  const FN: Usage = Usage::new(PAGE_APPLE_TOP_CASE, 0x03);

  fn context() -> KeyContext {
    KeyContext { listen_modifiers: MOD_OPTION, ..KeyContext::default() }
  }

  fn actions(outcome: &BatchOutcome) -> Vec<KeyAction> {
    outcome.actions.iter().map(|timed| timed.action).collect()
  }

  #[test]
  fn chord_in_one_report_is_classified_with_its_modifier() {
    let mut engine = Engine::new();
    // Element order within the report lists Space before Option.
    let outcome = engine.apply_batch(1, &[(SPACE, 1), (OPTION, 1)], 10, &context());
    assert_eq!(actions(&outcome), vec![KeyAction::HotkeyPressed]);
    assert_eq!(outcome.actions[0].timestamp, 10);
    assert_eq!(outcome.forward.unwrap(), ForwardState { modifiers: 0x04, ..Default::default() });
  }

  #[test]
  fn hold_history_and_release_forward_only_modifiers() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, 1)], 1, &ctx);
    engine.apply_batch(1, &[(SPACE, 1)], 2, &ctx);
    let up = engine.apply_batch(1, &[(UP, 1)], 3, &ctx);
    assert_eq!(actions(&up), vec![KeyAction::Navigate { direction: -1 }]);
    assert!(up.forward.is_none());
    engine.apply_batch(1, &[(UP, 0)], 4, &ctx);
    let option_up = engine.apply_batch(1, &[(OPTION, 0)], 5, &ctx);
    assert_eq!(option_up.forward.unwrap(), ForwardState::default());
    let release = engine.apply_batch(1, &[(SPACE, 0)], 6, &ctx);
    assert_eq!(actions(&release), vec![KeyAction::HotkeyReleased { raw_requested: false }]);
    assert!(release.forward.is_none());
  }

  #[test]
  fn simultaneous_modifier_and_key_release_keeps_the_chord() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, 1), (SPACE, 1)], 1, &ctx);
    let release = engine.apply_batch(1, &[(OPTION, 0), (SPACE, 0)], 2, &ctx);
    assert_eq!(actions(&release), vec![KeyAction::HotkeyReleased { raw_requested: true }]);
  }

  #[test]
  fn ordinary_typing_is_forwarded_once() {
    let mut engine = Engine::new();
    let ctx = context();
    let down = engine.apply_batch(1, &[(A, 1)], 1, &ctx);
    assert_eq!(down.forward.unwrap().keyboard, vec![usage::A]);
    assert_eq!(down.forwarded_edges, 1);
    let repeat = engine.apply_batch(1, &[(A, 1)], 2, &ctx);
    assert!(repeat.forward.is_none());
    let up = engine.apply_batch(1, &[(A, 0)], 3, &ctx);
    assert_eq!(up.forward.unwrap(), ForwardState::default());
  }

  #[test]
  fn shift_enter_is_forwarded_with_shift() {
    let mut engine = Engine::new();
    let ctx = KeyContext { enter: true, ..context() };
    let outcome = engine.apply_batch(1, &[(SHIFT, 1), (RETURN, 1)], 1, &ctx);
    assert!(outcome.actions.is_empty());
    let forward = outcome.forward.unwrap();
    assert_eq!(forward.modifiers, 0x02);
    assert_eq!(forward.keyboard, vec![usage::RETURN]);
  }

  #[test]
  fn consumer_and_apple_pages_pass_through() {
    let mut engine = Engine::new();
    let outcome = engine.apply_batch(1, &[(VOLUME_UP, 1), (FN, 1)], 1, &context());
    let forward = outcome.forward.unwrap();
    assert_eq!(forward.consumer, vec![0xE9]);
    assert_eq!(forward.apple_top_case, vec![0x03]);
  }

  // Read from a MacBook Pro's built-in keyboard service (Apple Internal Keyboard / Trackpad).
  const BUILT_IN_FN_FUNCTION: &str = "0x0007003a,0x00ff0005,0x0007003b,0x00ff0004,0x0007003c,0xff010010,0x0007003d,0x000c0221,0x0007003e,0x000c00cf,0x0007003f,0x0001009b,0x00070040,0x000c00b4,0x00070041,0x000c00cd,0x00070042,0x000c00b3,0x00070043,0x000c00e2,0x00070044,0x000c00ea,0x00070045,0x000c00e9";
  const BUILT_IN_FN_KEYBOARD: &str = "0x00070050,0x0007004a,0x00070052,0x0007004b,0x0007002a,0x0007004c,0x0007004f,0x0007004d,0x00070051,0x0007004e,0x00070028,0x00070058";
  const F1: Usage = Usage::new(PAGE_KEYBOARD, 0x3A);
  const F6: Usage = Usage::new(PAGE_KEYBOARD, 0x3F);
  const F12: Usage = Usage::new(PAGE_KEYBOARD, 0x45);
  const LEFT: Usage = Usage::new(PAGE_KEYBOARD, usage::LEFT_ARROW as u32);

  fn built_in(standard_function_keys: bool) -> DeviceTranslation {
    DeviceTranslation {
      fn_function: DeviceTranslation::parse_map(BUILT_IN_FN_FUNCTION),
      fn_keyboard: DeviceTranslation::parse_map(BUILT_IN_FN_KEYBOARD),
      standard_function_keys,
    }
  }

  #[test]
  fn built_in_f_row_forwards_media_keys_by_default() {
    let mut engine = Engine::new();
    engine.set_translation(1, built_in(false));
    let brightness = engine.apply_batch(1, &[(F1, 1)], 1, &context());
    assert_eq!(brightness.forward.unwrap().apple_top_case, vec![0x05]);
    engine.apply_batch(1, &[(F1, 0)], 2, &context());
    let dnd = engine.apply_batch(1, &[(F6, 1)], 3, &context());
    assert_eq!(dnd.forward.unwrap().generic_desktop, vec![0x9B]);
    engine.apply_batch(1, &[(F6, 0)], 4, &context());
    let volume = engine.apply_batch(1, &[(F12, 1)], 5, &context());
    assert_eq!(volume.forward.unwrap().consumer, vec![0xE9]);
  }

  #[test]
  fn fn_f_key_forwards_the_function_key() {
    let mut engine = Engine::new();
    engine.set_translation(1, built_in(false));
    let outcome = engine.apply_batch(1, &[(FN, 1), (F1, 1)], 1, &context());
    let forward = outcome.forward.unwrap();
    assert_eq!(forward.keyboard, vec![0x3A]);
    assert_eq!(forward.apple_top_case, vec![0x03]);
  }

  #[test]
  fn standard_function_key_mode_inverts_fn() {
    let mut engine = Engine::new();
    engine.set_translation(1, built_in(true));
    assert_eq!(
      engine.apply_batch(1, &[(F1, 1)], 1, &context()).forward.unwrap().keyboard,
      vec![0x3A]
    );
    engine.apply_batch(1, &[(F1, 0)], 2, &context());
    let media = engine.apply_batch(1, &[(FN, 1), (F1, 1)], 3, &context()).forward.unwrap();
    assert_eq!(media.apple_top_case, vec![0x03, 0x05]);
  }

  #[test]
  fn fn_arrow_forwards_home_and_release_matches_after_fn_lifts() {
    let mut engine = Engine::new();
    engine.set_translation(1, built_in(false));
    engine.apply_batch(1, &[(FN, 1)], 1, &context());
    let home = engine.apply_batch(1, &[(LEFT, 1)], 2, &context());
    assert_eq!(home.forward.unwrap().keyboard, vec![0x4A]);
    engine.apply_batch(1, &[(FN, 0)], 3, &context());
    let release = engine.apply_batch(1, &[(LEFT, 0)], 4, &context());
    assert_eq!(release.forward.unwrap(), ForwardState::default());
  }

  #[test]
  fn fn_return_becomes_keypad_enter_for_the_policy() {
    let mut engine = Engine::new();
    engine.set_translation(1, built_in(false));
    let ctx = KeyContext { enter: true, ..context() };
    engine.apply_batch(1, &[(FN, 1)], 1, &ctx);
    let outcome = engine.apply_batch(1, &[(RETURN, 1)], 2, &ctx);
    assert_eq!(actions(&outcome), vec![KeyAction::Finalize { raw_requested: false }]);
  }

  #[test]
  fn keyboards_without_maps_forward_f_keys_unchanged() {
    let mut engine = Engine::new();
    let outcome = engine.apply_batch(2, &[(F1, 1)], 1, &context());
    assert_eq!(outcome.forward.unwrap().keyboard, vec![0x3A]);
  }

  const BUTTON_1: Usage = Usage::new(PAGE_BUTTON, 1);
  const POINTER_X: Usage = Usage::new(PAGE_GENERIC_DESKTOP, 0x30);
  const POINTER_Y: Usage = Usage::new(PAGE_GENERIC_DESKTOP, 0x31);
  const WHEEL: Usage = Usage::new(PAGE_GENERIC_DESKTOP, 0x38);

  #[test]
  fn random_sequences_never_strand_keys_or_claims() {
    let keys = [OPTION, SHIFT, SPACE, UP, A, RETURN, FN, Usage::new(PAGE_KEYBOARD, 0x29)];
    let contexts = [
      context(),
      KeyContext { escape: true, enter: true, arrows: true, ..context() },
      KeyContext { search_input: true, ..context() },
    ];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
      seed ^= seed << 13;
      seed ^= seed >> 7;
      seed ^= seed << 17;
      seed
    };
    for _ in 0..500 {
      let mut engine = Engine::new();
      let mut held: Vec<(u64, Usage)> = Vec::new();
      for step in 0..40u64 {
        let ctx = contexts[(next() % 3) as usize];
        let device = 1 + next() % 2;
        let key = keys[(next() % keys.len() as u64) as usize];
        let down = !held.contains(&(device, key)) && next() % 2 == 0;
        if down {
          held.push((device, key));
        } else if let Some(i) = held.iter().position(|entry| *entry == (device, key)) {
          held.remove(i);
        } else {
          continue;
        }
        engine.apply_batch(device, &[(key, down as i64)], step, &ctx);
      }
      for (device, key) in held.drain(..) {
        engine.apply_batch(device, &[(key, 0)], 99, &context());
      }
      assert!(engine.forward_state().is_empty(), "stranded forward state");
      assert!(!engine.policy.space_hold_claimed(), "stranded hold");
      assert!((0..256u16).all(|usage| !engine.policy.is_claimed(usage)), "stranded claim");
    }
  }

  #[test]
  fn pointer_collection_on_a_keyboard_is_forwarded() {
    let mut engine = Engine::new();
    let moved = engine.apply_batch(1, &[(POINTER_X, -3), (POINTER_Y, 4)], 1, &context());
    assert_eq!(moved.pointer, Some(PointerReport { x: -3, y: 4, ..PointerReport::default() }));
    assert!(moved.forward.is_none());
    let pressed = engine.apply_batch(1, &[(BUTTON_1, 1), (WHEEL, -1)], 2, &context());
    assert_eq!(
      pressed.pointer,
      Some(PointerReport { buttons: 1, wheel: -1, ..PointerReport::default() })
    );
    let released = engine.apply_batch(1, &[(BUTTON_1, 0)], 3, &context());
    assert_eq!(released.pointer, Some(PointerReport::default()));
  }

  #[test]
  fn removing_a_device_releases_its_pointer_buttons() {
    let mut engine = Engine::new();
    engine.apply_batch(1, &[(BUTTON_1, 1)], 1, &context());
    let removed = engine.remove_device(1, 2, &context());
    assert_eq!(removed.pointer, Some(PointerReport::default()));
    engine.apply_batch(2, &[(BUTTON_1, 1)], 3, &context());
    assert_eq!(engine.reset(4).pointer, Some(PointerReport::default()));
  }

  #[test]
  fn keyboards_without_pointers_report_no_pointer_state() {
    let mut engine = Engine::new();
    assert!(engine.apply_batch(1, &[(A, 1)], 1, &context()).pointer.is_none());
    assert!(engine.remove_device(1, 2, &context()).pointer.is_none());
  }

  #[test]
  fn system_control_keys_pass_through() {
    let mut engine = Engine::new();
    let do_not_disturb = Usage::new(PAGE_GENERIC_DESKTOP, 0x9B);
    let outcome = engine.apply_batch(1, &[(do_not_disturb, 1)], 1, &context());
    assert_eq!(outcome.forward.unwrap().generic_desktop, vec![0x9B]);
    let pointer_x = Usage::new(PAGE_GENERIC_DESKTOP, 0x30);
    assert!(!pointer_x.is_forwardable());
  }

  #[test]
  fn unknown_usages_are_ignored() {
    let mut engine = Engine::new();
    let rollover = Usage::new(PAGE_KEYBOARD, 0x01);
    let led = Usage::new(0x08, 0x01);
    let outcome = engine.apply_batch(1, &[(rollover, 1), (led, 1)], 1, &context());
    assert!(outcome.forward.is_none());
    assert_eq!(outcome.forwarded_edges, 0);
  }

  #[test]
  fn keys_held_on_two_keyboards_release_after_the_last_one() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(SHIFT, 1)], 1, &ctx);
    assert!(engine.apply_batch(2, &[(SHIFT, 1)], 2, &ctx).forward.is_none());
    assert!(engine.apply_batch(1, &[(SHIFT, 0)], 3, &ctx).forward.is_none());
    assert_eq!(
      engine.apply_batch(2, &[(SHIFT, 0)], 4, &ctx).forward.unwrap(),
      ForwardState::default()
    );
  }

  #[test]
  fn modifier_from_other_keyboard_completes_chord() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, 1)], 1, &ctx);
    let outcome = engine.apply_batch(2, &[(SPACE, 1)], 2, &ctx);
    assert_eq!(actions(&outcome), vec![KeyAction::HotkeyPressed]);
  }

  #[test]
  fn device_removal_mid_hold_releases_keys_and_finishes_gesture() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, 1), (SPACE, 1)], 1, &ctx);
    engine.apply_batch(1, &[(A, 1)], 2, &ctx);
    let removed = engine.remove_device(1, 3, &ctx);
    assert_eq!(actions(&removed), vec![KeyAction::HotkeyReleased { raw_requested: true }]);
    assert_eq!(removed.forward.unwrap(), ForwardState::default());
  }

  #[test]
  fn reset_clears_forwarded_keys_and_orphaned_hold() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, 1), (SPACE, 1)], 1, &ctx);
    let reset = engine.reset(9);
    assert_eq!(actions(&reset), vec![KeyAction::HotkeyReleased { raw_requested: false }]);
    assert_eq!(reset.forward.unwrap(), ForwardState::default());
    let after = engine.apply_batch(1, &[(SPACE, 1)], 10, &ctx);
    assert_eq!(after.forward.unwrap().keyboard, vec![usage::SPACE]);
  }

  #[test]
  fn duplicate_right_modifier_keeps_bit_while_left_releases() {
    let mut engine = Engine::new();
    let ctx = context();
    let right_option = Usage::new(PAGE_KEYBOARD, usage::RIGHT_OPTION as u32);
    engine.apply_batch(1, &[(OPTION, 1), (right_option, 1)], 1, &ctx);
    engine.apply_batch(1, &[(OPTION, 0)], 2, &ctx);
    let outcome = engine.apply_batch(1, &[(SPACE, 1)], 3, &ctx);
    assert_eq!(actions(&outcome), vec![KeyAction::HotkeyPressed]);
  }
}
