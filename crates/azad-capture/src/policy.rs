//! Azad's shortcut policy over HID keyboard-page usages.
//!
//! The helper evaluates this policy synchronously for every key edge so claimed input never
//! reaches the foreground application. A claimed press owns its release: the matching key-up is
//! suppressed even when the overlay context or modifiers changed while the key was held.

use serde::{Deserialize, Serialize};

pub const MOD_SHIFT: u8 = 1;
pub const MOD_CONTROL: u8 = 2;
pub const MOD_OPTION: u8 = 4;
pub const MOD_COMMAND: u8 = 8;

/// HID keyboard/keypad page (0x07) usages the policy names.
pub mod usage {
  pub const A: u16 = 0x04;
  pub const RETURN: u16 = 0x28;
  pub const ESCAPE: u16 = 0x29;
  pub const DELETE: u16 = 0x2A;
  pub const TAB: u16 = 0x2B;
  pub const SPACE: u16 = 0x2C;
  pub const CAPS_LOCK: u16 = 0x39;
  pub const RIGHT_ARROW: u16 = 0x4F;
  pub const LEFT_ARROW: u16 = 0x50;
  pub const DOWN_ARROW: u16 = 0x51;
  pub const UP_ARROW: u16 = 0x52;
  pub const KEYPAD_ENTER: u16 = 0x58;
  pub const LEFT_CONTROL: u16 = 0xE0;
  pub const LEFT_SHIFT: u16 = 0xE1;
  pub const LEFT_OPTION: u16 = 0xE2;
  pub const LEFT_COMMAND: u16 = 0xE3;
  pub const RIGHT_CONTROL: u16 = 0xE4;
  pub const RIGHT_SHIFT: u16 = 0xE5;
  pub const RIGHT_OPTION: u16 = 0xE6;
  pub const RIGHT_COMMAND: u16 = 0xE7;
}

/// Maps a keyboard-page modifier usage to its `MOD_*` bit.
pub fn modifier_bit(key: u16) -> Option<u8> {
  match key {
    usage::LEFT_CONTROL | usage::RIGHT_CONTROL => Some(MOD_CONTROL),
    usage::LEFT_SHIFT | usage::RIGHT_SHIFT => Some(MOD_SHIFT),
    usage::LEFT_OPTION | usage::RIGHT_OPTION => Some(MOD_OPTION),
    usage::LEFT_COMMAND | usage::RIGHT_COMMAND => Some(MOD_COMMAND),
    _ => None,
  }
}

/// Keys whose press produces layout-dependent text in the history search field.
pub fn is_text_key(key: u16) -> bool {
  matches!(key, 0x04..=0x27 | 0x2C..=0x38 | 0x54..=0x57 | 0x59..=0x64 | 0x67 | 0x85 | 0x87..=0x89)
}

/// Overlay state that decides which keys Azad claims. The app publishes it; the helper applies
/// it to the next key edge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyContext {
  pub listen_modifiers: u8,
  pub escape: bool,
  pub enter: bool,
  pub arrows: bool,
  pub arrow_left: bool,
  pub arrow_right: bool,
  pub search_input: bool,
}

/// An Azad command produced by a claimed key edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeyAction {
  HotkeyPressed,
  HotkeyReleased {
    raw_requested: bool,
  },
  Finalize {
    raw_requested: bool,
  },
  Navigate {
    direction: i32,
  },
  Cancel,
  HistoryCollapse,
  HistoryExpand,
  SearchBackspace,
  SearchDeleteWord,
  SearchClear,
  /// A text key for the search field; the app resolves it with the active keyboard layout.
  SearchKey {
    usage: u16,
    modifiers: u8,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
  pub claimed: bool,
  pub action: Option<KeyAction>,
}

impl Decision {
  const PASS: Decision = Decision { claimed: false, action: None };
  const CLAIM: Decision = Decision { claimed: true, action: None };

  fn act(action: KeyAction) -> Decision {
    Decision { claimed: true, action: Some(action) }
  }
}

#[derive(Debug, Default, Clone)]
pub struct KeyPolicy {
  claimed: [u64; 4],
  space_hold: bool,
}

impl KeyPolicy {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn space_hold_claimed(&self) -> bool {
    self.space_hold
  }

  pub fn is_claimed(&self, key: u16) -> bool {
    key < 256 && self.claimed[(key / 64) as usize] & (1 << (key % 64)) != 0
  }

  pub fn key_down(&mut self, key: u16, modifiers: u8, context: &KeyContext) -> Decision {
    if modifier_bit(key).is_some() {
      return Decision::PASS;
    }
    if self.is_claimed(key) {
      return Decision::CLAIM;
    }
    let decision = Self::classify_down(self.space_hold, key, modifiers, context);
    if decision.claimed {
      self.set_claimed(key, true);
      if decision.action == Some(KeyAction::HotkeyPressed) {
        self.space_hold = true;
      }
    }
    decision
  }

  pub fn key_up(&mut self, key: u16, modifiers: u8) -> Decision {
    if !self.is_claimed(key) {
      return Decision::PASS;
    }
    self.set_claimed(key, false);
    if key == usage::SPACE && self.space_hold {
      self.space_hold = false;
      return Decision::act(KeyAction::HotkeyReleased {
        raw_requested: modifiers & MOD_OPTION != 0,
      });
    }
    Decision::CLAIM
  }

  /// Releases every claimed key without a physical key-up, e.g. when its device disappears.
  pub fn release_all(&mut self) -> Vec<KeyAction> {
    let mut actions = Vec::new();
    if self.space_hold {
      actions.push(KeyAction::HotkeyReleased { raw_requested: false });
    }
    *self = Self::default();
    actions
  }

  fn classify_down(space_hold: bool, key: u16, modifiers: u8, context: &KeyContext) -> Decision {
    let shift = modifiers & MOD_SHIFT != 0;
    let option = modifiers & MOD_OPTION != 0;

    if key == usage::SPACE {
      let wanted = context.listen_modifiers;
      if wanted != 0 && modifiers & wanted == wanted {
        return Decision::act(KeyAction::HotkeyPressed);
      }
    }
    if space_hold && key == usage::UP_ARROW {
      return Decision::act(KeyAction::Navigate { direction: -1 });
    }
    if context.escape && key == usage::ESCAPE {
      return Decision::act(KeyAction::Cancel);
    }
    let is_enter = key == usage::RETURN || key == usage::KEYPAD_ENTER;
    if (context.enter || context.search_input) && is_enter {
      // Shift+Enter is the soft-return escape hatch for the application underneath.
      if shift {
        return Decision::PASS;
      }
      return Decision::act(KeyAction::Finalize { raw_requested: option });
    }
    if context.arrows && key == usage::UP_ARROW {
      return Decision::act(KeyAction::Navigate { direction: -1 });
    }
    if context.arrows && key == usage::DOWN_ARROW {
      return Decision::act(KeyAction::Navigate { direction: 1 });
    }
    if context.arrow_left && key == usage::LEFT_ARROW {
      return Decision::act(KeyAction::HistoryCollapse);
    }
    if context.arrow_right && key == usage::RIGHT_ARROW {
      return Decision::act(KeyAction::HistoryExpand);
    }
    if context.search_input {
      if key == usage::DELETE {
        let action = if modifiers & MOD_COMMAND != 0 {
          KeyAction::SearchClear
        } else if option {
          KeyAction::SearchDeleteWord
        } else {
          KeyAction::SearchBackspace
        };
        return Decision::act(action);
      }
      // Control chords produce control characters, which never entered the search field.
      if is_text_key(key) && modifiers & MOD_CONTROL == 0 {
        return Decision::act(KeyAction::SearchKey { usage: key, modifiers });
      }
    }
    Decision::PASS
  }

  fn set_claimed(&mut self, key: u16, claimed: bool) {
    if key >= 256 {
      return;
    }
    let bit = 1u64 << (key % 64);
    let word = &mut self.claimed[(key / 64) as usize];
    if claimed {
      *word |= bit;
    } else {
      *word &= !bit;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn listen_only() -> KeyContext {
    KeyContext { listen_modifiers: MOD_OPTION, ..KeyContext::default() }
  }

  fn overlay() -> KeyContext {
    KeyContext {
      listen_modifiers: MOD_OPTION,
      escape: true,
      enter: true,
      arrows: true,
      arrow_left: true,
      arrow_right: true,
      search_input: false,
    }
  }

  #[test]
  fn option_space_press_claims_and_releases_non_raw_after_option_release() {
    let mut policy = KeyPolicy::new();
    assert_eq!(
      policy.key_down(usage::SPACE, MOD_OPTION, &listen_only()),
      Decision::act(KeyAction::HotkeyPressed)
    );
    assert!(policy.space_hold_claimed());
    assert_eq!(
      policy.key_up(usage::SPACE, 0),
      Decision::act(KeyAction::HotkeyReleased { raw_requested: false })
    );
    assert!(!policy.space_hold_claimed());
  }

  #[test]
  fn space_release_while_option_held_requests_raw() {
    let mut policy = KeyPolicy::new();
    policy.key_down(usage::SPACE, MOD_OPTION, &listen_only());
    assert_eq!(
      policy.key_up(usage::SPACE, MOD_OPTION),
      Decision::act(KeyAction::HotkeyReleased { raw_requested: true })
    );
  }

  #[test]
  fn modifier_superset_still_matches_listen_chord() {
    let mut policy = KeyPolicy::new();
    assert!(policy.key_down(usage::SPACE, MOD_OPTION | MOD_SHIFT, &listen_only()).claimed);
  }

  #[test]
  fn bare_space_and_empty_mask_pass_through() {
    let mut policy = KeyPolicy::new();
    assert_eq!(policy.key_down(usage::SPACE, 0, &listen_only()), Decision::PASS);
    assert_eq!(policy.key_up(usage::SPACE, 0), Decision::PASS);
    assert_eq!(policy.key_down(usage::SPACE, MOD_OPTION, &KeyContext::default()), Decision::PASS);
  }

  #[test]
  fn up_during_claimed_hold_navigates_history_and_owns_release() {
    let mut policy = KeyPolicy::new();
    policy.key_down(usage::SPACE, MOD_OPTION, &listen_only());
    assert_eq!(
      policy.key_down(usage::UP_ARROW, MOD_OPTION, &listen_only()),
      Decision::act(KeyAction::Navigate { direction: -1 })
    );
    assert_eq!(policy.key_up(usage::UP_ARROW, 0), Decision::CLAIM);
  }

  #[test]
  fn option_up_without_space_hold_is_not_a_command() {
    let mut policy = KeyPolicy::new();
    assert_eq!(policy.key_down(usage::UP_ARROW, MOD_OPTION, &listen_only()), Decision::PASS);
    assert_eq!(policy.key_down(usage::DOWN_ARROW, MOD_OPTION, &listen_only()), Decision::PASS);
  }

  #[test]
  fn overlay_keys_claim_both_edges() {
    let mut policy = KeyPolicy::new();
    let context = overlay();
    let cases = [
      (usage::ESCAPE, KeyAction::Cancel),
      (usage::RETURN, KeyAction::Finalize { raw_requested: false }),
      (usage::KEYPAD_ENTER, KeyAction::Finalize { raw_requested: false }),
      (usage::UP_ARROW, KeyAction::Navigate { direction: -1 }),
      (usage::DOWN_ARROW, KeyAction::Navigate { direction: 1 }),
      (usage::LEFT_ARROW, KeyAction::HistoryCollapse),
      (usage::RIGHT_ARROW, KeyAction::HistoryExpand),
    ];
    for (key, action) in cases {
      assert_eq!(policy.key_down(key, 0, &context), Decision::act(action), "{key:#x}");
      assert_eq!(policy.key_up(key, 0), Decision::CLAIM, "{key:#x}");
    }
  }

  #[test]
  fn option_enter_requests_raw_finalize() {
    let mut policy = KeyPolicy::new();
    assert_eq!(
      policy.key_down(usage::RETURN, MOD_OPTION, &overlay()),
      Decision::act(KeyAction::Finalize { raw_requested: true })
    );
    assert_eq!(
      policy.key_down(usage::KEYPAD_ENTER, MOD_OPTION, &overlay()),
      Decision::act(KeyAction::Finalize { raw_requested: true })
    );
  }

  #[test]
  fn shift_enter_passes_through_in_overlay_and_search() {
    let mut policy = KeyPolicy::new();
    assert_eq!(policy.key_down(usage::RETURN, MOD_SHIFT, &overlay()), Decision::PASS);
    let search = KeyContext { search_input: true, ..KeyContext::default() };
    assert_eq!(policy.key_down(usage::KEYPAD_ENTER, MOD_SHIFT, &search), Decision::PASS);
    assert_eq!(policy.key_up(usage::KEYPAD_ENTER, MOD_SHIFT), Decision::PASS);
  }

  #[test]
  fn overlay_keys_pass_through_when_context_is_hidden() {
    let mut policy = KeyPolicy::new();
    for key in [usage::ESCAPE, usage::RETURN, usage::UP_ARROW, usage::LEFT_ARROW] {
      assert_eq!(policy.key_down(key, 0, &listen_only()), Decision::PASS);
    }
  }

  #[test]
  fn claimed_press_owns_release_after_context_closes() {
    let mut policy = KeyPolicy::new();
    policy.key_down(usage::ESCAPE, 0, &overlay());
    // The overlay hid on Escape; the release must still be suppressed.
    assert_eq!(policy.key_up(usage::ESCAPE, 0), Decision::CLAIM);
  }

  #[test]
  fn unclaimed_press_release_passes_after_context_opens() {
    let mut policy = KeyPolicy::new();
    assert_eq!(policy.key_down(usage::RETURN, 0, &listen_only()), Decision::PASS);
    assert_eq!(policy.key_up(usage::RETURN, 0), Decision::PASS);
  }

  #[test]
  fn search_input_claims_text_and_editing_keys() {
    let mut policy = KeyPolicy::new();
    let search = KeyContext { search_input: true, ..listen_only() };
    assert_eq!(
      policy.key_down(usage::A, MOD_SHIFT, &search),
      Decision::act(KeyAction::SearchKey { usage: usage::A, modifiers: MOD_SHIFT })
    );
    assert_eq!(
      policy.key_down(usage::SPACE, 0, &search),
      Decision::act(KeyAction::SearchKey { usage: usage::SPACE, modifiers: 0 })
    );
    assert_eq!(
      policy.key_down(usage::DELETE, 0, &search),
      Decision::act(KeyAction::SearchBackspace)
    );
    policy.key_up(usage::DELETE, 0);
    assert_eq!(
      policy.key_down(usage::DELETE, MOD_OPTION, &search),
      Decision::act(KeyAction::SearchDeleteWord)
    );
    policy.key_up(usage::DELETE, 0);
    assert_eq!(
      policy.key_down(usage::DELETE, MOD_COMMAND, &search),
      Decision::act(KeyAction::SearchClear)
    );
    assert_eq!(
      policy.key_down(usage::RETURN, 0, &search),
      Decision::act(KeyAction::Finalize { raw_requested: false })
    );
  }

  #[test]
  fn search_input_leaves_control_chords_tab_and_modifiers_alone() {
    let mut policy = KeyPolicy::new();
    let search = KeyContext { search_input: true, ..KeyContext::default() };
    assert_eq!(policy.key_down(usage::A, MOD_CONTROL, &search), Decision::PASS);
    assert_eq!(policy.key_down(usage::TAB, 0, &search), Decision::PASS);
    assert_eq!(policy.key_down(usage::LEFT_SHIFT, MOD_SHIFT, &search), Decision::PASS);
    assert_eq!(policy.key_down(usage::CAPS_LOCK, 0, &search), Decision::PASS);
  }

  #[test]
  fn listen_chord_wins_over_search_text() {
    let mut policy = KeyPolicy::new();
    let search = KeyContext { search_input: true, ..listen_only() };
    assert_eq!(
      policy.key_down(usage::SPACE, MOD_OPTION, &search),
      Decision::act(KeyAction::HotkeyPressed)
    );
  }

  #[test]
  fn release_all_finishes_an_orphaned_space_hold() {
    let mut policy = KeyPolicy::new();
    policy.key_down(usage::SPACE, MOD_OPTION, &listen_only());
    policy.key_down(usage::UP_ARROW, MOD_OPTION, &listen_only());
    assert_eq!(policy.release_all(), vec![KeyAction::HotkeyReleased { raw_requested: false }]);
    assert!(!policy.is_claimed(usage::UP_ARROW));
    assert_eq!(policy.key_up(usage::SPACE, 0), Decision::PASS);
  }

  #[test]
  fn modifiers_are_never_claimed() {
    let mut policy = KeyPolicy::new();
    for key in usage::LEFT_CONTROL..=usage::RIGHT_COMMAND {
      assert_eq!(policy.key_down(key, MOD_OPTION, &overlay()), Decision::PASS);
    }
  }
}
