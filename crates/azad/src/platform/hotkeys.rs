use super::{MOD_COMMAND, MOD_CONTROL, MOD_OPTION, MOD_SHIFT};

/// Build the persisted MOD_* mask from live CGEventFlags booleans.
pub(super) fn current_mod_mask(
  is_option: bool,
  is_shift: bool,
  is_command: bool,
  is_control: bool,
) -> u8 {
  let mut m = 0u8;
  if is_shift {
    m |= MOD_SHIFT;
  }
  if is_control {
    m |= MOD_CONTROL;
  }
  if is_option {
    m |= MOD_OPTION;
  }
  if is_command {
    m |= MOD_COMMAND;
  }
  m
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SpaceHotkeyDecision {
  pub(super) claimed_after: bool,
  pub(super) action: SpaceHotkeyAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpaceHotkeyAction {
  PassThrough,
  ClaimOnly,
  Press,
  Release { raw_requested: bool },
}

pub(super) fn space_hotkey_decision(
  wanted_mods: u8,
  prior_claimed: bool,
  live_mods: u8,
  is_keydown: bool,
  is_autorepeat: bool,
) -> SpaceHotkeyDecision {
  let mods_match = wanted_mods != 0 && (live_mods & wanted_mods) == wanted_mods;
  if is_keydown {
    if mods_match {
      if is_autorepeat {
        return SpaceHotkeyDecision {
          claimed_after: prior_claimed,
          action: SpaceHotkeyAction::ClaimOnly,
        };
      }
      return SpaceHotkeyDecision { claimed_after: true, action: SpaceHotkeyAction::Press };
    }
    if prior_claimed {
      return SpaceHotkeyDecision { claimed_after: true, action: SpaceHotkeyAction::ClaimOnly };
    }
    return SpaceHotkeyDecision { claimed_after: false, action: SpaceHotkeyAction::PassThrough };
  }

  if prior_claimed {
    SpaceHotkeyDecision {
      claimed_after: false,
      action: SpaceHotkeyAction::Release { raw_requested: live_mods & MOD_OPTION != 0 },
    }
  } else {
    SpaceHotkeyDecision { claimed_after: false, action: SpaceHotkeyAction::PassThrough }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClaimedHoldNavigationAction {
  PassThrough,
  ClaimOnly,
  Navigate(i32),
}

pub(super) fn claimed_hold_navigation_decision(
  space_claimed: bool,
  keycode: u16,
  is_keydown: bool,
) -> ClaimedHoldNavigationAction {
  if !space_claimed || keycode != super::KEYCODE_ARROW_UP {
    return ClaimedHoldNavigationAction::PassThrough;
  }
  if is_keydown {
    ClaimedHoldNavigationAction::Navigate(-1)
  } else {
    ClaimedHoldNavigationAction::ClaimOnly
  }
}

#[cfg(test)]
mod tests {
  use super::{
    ClaimedHoldNavigationAction, SpaceHotkeyAction, SpaceHotkeyDecision,
    claimed_hold_navigation_decision, space_hotkey_decision,
  };
  use crate::platform::{KEYCODE_ARROW_DOWN, KEYCODE_ARROW_UP, MOD_OPTION};

  #[test]
  fn option_space_press_claims_and_dispatches_press() {
    assert_eq!(
      space_hotkey_decision(MOD_OPTION, false, MOD_OPTION, true, false),
      SpaceHotkeyDecision { claimed_after: true, action: SpaceHotkeyAction::Press }
    );
  }

  #[test]
  fn claimed_space_repeat_after_option_release_is_swallowed() {
    assert_eq!(
      space_hotkey_decision(MOD_OPTION, true, 0, true, true),
      SpaceHotkeyDecision { claimed_after: true, action: SpaceHotkeyAction::ClaimOnly }
    );
  }

  #[test]
  fn claimed_space_keydown_after_option_release_is_swallowed() {
    assert_eq!(
      space_hotkey_decision(MOD_OPTION, true, 0, true, false),
      SpaceHotkeyDecision { claimed_after: true, action: SpaceHotkeyAction::ClaimOnly }
    );
  }

  #[test]
  fn claimed_space_keyup_after_option_release_finalizes_non_raw() {
    assert_eq!(
      space_hotkey_decision(MOD_OPTION, true, 0, false, false),
      SpaceHotkeyDecision {
        claimed_after: false,
        action: SpaceHotkeyAction::Release { raw_requested: false },
      }
    );
  }

  #[test]
  fn claimed_space_keyup_while_option_held_finalizes_raw() {
    assert_eq!(
      space_hotkey_decision(MOD_OPTION, true, MOD_OPTION, false, false),
      SpaceHotkeyDecision {
        claimed_after: false,
        action: SpaceHotkeyAction::Release { raw_requested: true },
      }
    );
  }

  #[test]
  fn unclaimed_bare_space_passes_through() {
    assert_eq!(
      space_hotkey_decision(MOD_OPTION, false, 0, true, false),
      SpaceHotkeyDecision { claimed_after: false, action: SpaceHotkeyAction::PassThrough }
    );
    assert_eq!(
      space_hotkey_decision(MOD_OPTION, false, 0, false, false),
      SpaceHotkeyDecision { claimed_after: false, action: SpaceHotkeyAction::PassThrough }
    );
  }

  #[test]
  fn claimed_space_up_opens_history_before_overlay_arrow_registration() {
    assert_eq!(
      claimed_hold_navigation_decision(true, KEYCODE_ARROW_UP, true),
      ClaimedHoldNavigationAction::Navigate(-1)
    );
    assert_eq!(
      claimed_hold_navigation_decision(true, KEYCODE_ARROW_UP, false),
      ClaimedHoldNavigationAction::ClaimOnly
    );
  }

  #[test]
  fn unclaimed_or_non_history_arrows_pass_through() {
    assert_eq!(
      claimed_hold_navigation_decision(false, KEYCODE_ARROW_UP, true),
      ClaimedHoldNavigationAction::PassThrough
    );
    assert_eq!(
      claimed_hold_navigation_decision(true, KEYCODE_ARROW_DOWN, true),
      ClaimedHoldNavigationAction::PassThrough
    );
  }
}
