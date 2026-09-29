//! Report-level normalization between seized keyboards and the virtual output keyboard.
//!
//! IOKit delivers one value per changed element; every value from one input report shares a
//! timestamp. Classifying element by element would see `Space` before `Option` when both change
//! in one report, so the engine applies a whole batch at once and orders the resulting edges:
//! modifier presses, key releases, key presses, then modifier releases.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::policy::{KeyAction, KeyContext, KeyPolicy, modifier_bit};

pub const PAGE_GENERIC_DESKTOP: u32 = 0x01;
pub const PAGE_KEYBOARD: u32 = 0x07;
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

  /// True for usages the helper tracks and forwards; everything else on a seized keyboard is
  /// dropped (LED outputs, error roll-over, vendor diagnostics).
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
  pub claimed_edges: u32,
  pub forwarded_edges: u32,
}

#[derive(Debug, Default)]
pub struct Engine {
  devices: HashMap<u64, BTreeSet<Usage>>,
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

  /// Applies one report's worth of element values from `device`.
  pub fn apply_batch(
    &mut self,
    device: u64,
    values: &[(Usage, bool)],
    timestamp: Timestamp,
    context: &KeyContext,
  ) -> BatchOutcome {
    let held = self.devices.entry(device).or_default();
    let mut edges = Vec::new();
    for &(usage, down) in values {
      if !usage.is_forwardable() {
        continue;
      }
      let changed = if down { held.insert(usage) } else { held.remove(&usage) };
      if !changed {
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
    self.run_edges(edges, timestamp, context)
  }

  /// Releases everything `device` held, as if it sent an all-up report.
  pub fn remove_device(
    &mut self,
    device: u64,
    timestamp: Timestamp,
    context: &KeyContext,
  ) -> BatchOutcome {
    let Some(held) = self.devices.remove(&device) else {
      return BatchOutcome::default();
    };
    let mut edges = Vec::new();
    for usage in held {
      if let Some(count) = self.pressed.get_mut(&usage) {
        *count -= 1;
        if *count == 0 {
          self.pressed.remove(&usage);
          edges.push(Edge { usage, down: false });
        }
      }
    }
    self.run_edges(edges, timestamp, context)
  }

  /// Drops all device state and claims, e.g. when forwarding stops. Returns the actions that
  /// finish any claimed gesture so the app does not stay in a held state.
  pub fn reset(&mut self, timestamp: Timestamp) -> BatchOutcome {
    self.devices.clear();
    self.pressed.clear();
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
    BatchOutcome { actions, forward, ..BatchOutcome::default() }
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
    let outcome = engine.apply_batch(1, &[(SPACE, true), (OPTION, true)], 10, &context());
    assert_eq!(actions(&outcome), vec![KeyAction::HotkeyPressed]);
    assert_eq!(outcome.actions[0].timestamp, 10);
    assert_eq!(outcome.forward.unwrap(), ForwardState { modifiers: 0x04, ..Default::default() });
  }

  #[test]
  fn hold_history_and_release_forward_only_modifiers() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, true)], 1, &ctx);
    engine.apply_batch(1, &[(SPACE, true)], 2, &ctx);
    let up = engine.apply_batch(1, &[(UP, true)], 3, &ctx);
    assert_eq!(actions(&up), vec![KeyAction::Navigate { direction: -1 }]);
    assert!(up.forward.is_none());
    engine.apply_batch(1, &[(UP, false)], 4, &ctx);
    let option_up = engine.apply_batch(1, &[(OPTION, false)], 5, &ctx);
    assert_eq!(option_up.forward.unwrap(), ForwardState::default());
    let release = engine.apply_batch(1, &[(SPACE, false)], 6, &ctx);
    assert_eq!(actions(&release), vec![KeyAction::HotkeyReleased { raw_requested: false }]);
    assert!(release.forward.is_none());
  }

  #[test]
  fn simultaneous_modifier_and_key_release_keeps_the_chord() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, true), (SPACE, true)], 1, &ctx);
    let release = engine.apply_batch(1, &[(OPTION, false), (SPACE, false)], 2, &ctx);
    assert_eq!(actions(&release), vec![KeyAction::HotkeyReleased { raw_requested: true }]);
  }

  #[test]
  fn ordinary_typing_is_forwarded_once() {
    let mut engine = Engine::new();
    let ctx = context();
    let down = engine.apply_batch(1, &[(A, true)], 1, &ctx);
    assert_eq!(down.forward.unwrap().keyboard, vec![usage::A]);
    assert_eq!(down.forwarded_edges, 1);
    let repeat = engine.apply_batch(1, &[(A, true)], 2, &ctx);
    assert!(repeat.forward.is_none());
    let up = engine.apply_batch(1, &[(A, false)], 3, &ctx);
    assert_eq!(up.forward.unwrap(), ForwardState::default());
  }

  #[test]
  fn shift_enter_is_forwarded_with_shift() {
    let mut engine = Engine::new();
    let ctx = KeyContext { enter: true, ..context() };
    let outcome = engine.apply_batch(1, &[(SHIFT, true), (RETURN, true)], 1, &ctx);
    assert!(outcome.actions.is_empty());
    let forward = outcome.forward.unwrap();
    assert_eq!(forward.modifiers, 0x02);
    assert_eq!(forward.keyboard, vec![usage::RETURN]);
  }

  #[test]
  fn consumer_and_apple_pages_pass_through() {
    let mut engine = Engine::new();
    let outcome = engine.apply_batch(1, &[(VOLUME_UP, true), (FN, true)], 1, &context());
    let forward = outcome.forward.unwrap();
    assert_eq!(forward.consumer, vec![0xE9]);
    assert_eq!(forward.apple_top_case, vec![0x03]);
  }

  #[test]
  fn system_control_keys_pass_through() {
    let mut engine = Engine::new();
    let do_not_disturb = Usage::new(PAGE_GENERIC_DESKTOP, 0x9B);
    let outcome = engine.apply_batch(1, &[(do_not_disturb, true)], 1, &context());
    assert_eq!(outcome.forward.unwrap().generic_desktop, vec![0x9B]);
    let pointer_x = Usage::new(PAGE_GENERIC_DESKTOP, 0x30);
    assert!(!pointer_x.is_forwardable());
  }

  #[test]
  fn unknown_usages_are_ignored() {
    let mut engine = Engine::new();
    let rollover = Usage::new(PAGE_KEYBOARD, 0x01);
    let led = Usage::new(0x08, 0x01);
    let outcome = engine.apply_batch(1, &[(rollover, true), (led, true)], 1, &context());
    assert!(outcome.forward.is_none());
    assert_eq!(outcome.forwarded_edges, 0);
  }

  #[test]
  fn keys_held_on_two_keyboards_release_after_the_last_one() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(SHIFT, true)], 1, &ctx);
    assert!(engine.apply_batch(2, &[(SHIFT, true)], 2, &ctx).forward.is_none());
    assert!(engine.apply_batch(1, &[(SHIFT, false)], 3, &ctx).forward.is_none());
    assert_eq!(
      engine.apply_batch(2, &[(SHIFT, false)], 4, &ctx).forward.unwrap(),
      ForwardState::default()
    );
  }

  #[test]
  fn modifier_from_other_keyboard_completes_chord() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, true)], 1, &ctx);
    let outcome = engine.apply_batch(2, &[(SPACE, true)], 2, &ctx);
    assert_eq!(actions(&outcome), vec![KeyAction::HotkeyPressed]);
  }

  #[test]
  fn device_removal_mid_hold_releases_keys_and_finishes_gesture() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, true), (SPACE, true)], 1, &ctx);
    engine.apply_batch(1, &[(A, true)], 2, &ctx);
    let removed = engine.remove_device(1, 3, &ctx);
    assert_eq!(actions(&removed), vec![KeyAction::HotkeyReleased { raw_requested: true }]);
    assert_eq!(removed.forward.unwrap(), ForwardState::default());
  }

  #[test]
  fn reset_clears_forwarded_keys_and_orphaned_hold() {
    let mut engine = Engine::new();
    let ctx = context();
    engine.apply_batch(1, &[(OPTION, true), (SPACE, true)], 1, &ctx);
    let reset = engine.reset(9);
    assert_eq!(actions(&reset), vec![KeyAction::HotkeyReleased { raw_requested: false }]);
    assert_eq!(reset.forward.unwrap(), ForwardState::default());
    let after = engine.apply_batch(1, &[(SPACE, true)], 10, &ctx);
    assert_eq!(after.forward.unwrap().keyboard, vec![usage::SPACE]);
  }

  #[test]
  fn duplicate_right_modifier_keeps_bit_while_left_releases() {
    let mut engine = Engine::new();
    let ctx = context();
    let right_option = Usage::new(PAGE_KEYBOARD, usage::RIGHT_OPTION as u32);
    engine.apply_batch(1, &[(OPTION, true), (right_option, true)], 1, &ctx);
    engine.apply_batch(1, &[(OPTION, false)], 2, &ctx);
    let outcome = engine.apply_batch(1, &[(SPACE, true)], 3, &ctx);
    assert_eq!(actions(&outcome), vec![KeyAction::HotkeyPressed]);
  }
}
