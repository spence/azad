use super::{
  KEYCODE_ARROW_DOWN, KEYCODE_ARROW_LEFT, KEYCODE_ARROW_RIGHT, KEYCODE_ARROW_UP, KEYCODE_ESCAPE,
  KEYCODE_NUMPAD_ENTER, KEYCODE_RETURN, MOD_COMMAND, MOD_CONTROL, MOD_OPTION, MOD_SHIFT,
};

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
pub(super) struct OverlayHotkeyState {
  pub(super) escape_enabled: bool,
  pub(super) enter_enabled: bool,
  pub(super) arrows_enabled: bool,
  pub(super) arrow_left_enabled: bool,
  pub(super) arrow_right_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayHotkeyAction {
  PassThrough,
  ClaimOnly,
  Cancel,
  Finalize { raw_requested: bool },
  Navigate(i32),
  HistoryCollapse,
  HistoryExpand,
}

pub(super) fn overlay_hotkey_decision(
  state: OverlayHotkeyState,
  keycode: u16,
  is_option: bool,
  is_shift: bool,
  is_keydown: bool,
) -> OverlayHotkeyAction {
  if state.escape_enabled && keycode == KEYCODE_ESCAPE {
    return if is_keydown { OverlayHotkeyAction::Cancel } else { OverlayHotkeyAction::ClaimOnly };
  }

  if state.enter_enabled && (keycode == KEYCODE_RETURN || keycode == KEYCODE_NUMPAD_ENTER) {
    if is_shift {
      return OverlayHotkeyAction::PassThrough;
    }
    return if is_keydown {
      OverlayHotkeyAction::Finalize { raw_requested: is_option }
    } else {
      OverlayHotkeyAction::ClaimOnly
    };
  }

  if state.arrows_enabled {
    if keycode == KEYCODE_ARROW_UP {
      return if is_keydown {
        OverlayHotkeyAction::Navigate(-1)
      } else {
        OverlayHotkeyAction::ClaimOnly
      };
    }
    if keycode == KEYCODE_ARROW_DOWN {
      return if is_keydown {
        OverlayHotkeyAction::Navigate(1)
      } else {
        OverlayHotkeyAction::ClaimOnly
      };
    }
  }

  if state.arrow_left_enabled && keycode == KEYCODE_ARROW_LEFT {
    return if is_keydown {
      OverlayHotkeyAction::HistoryCollapse
    } else {
      OverlayHotkeyAction::ClaimOnly
    };
  }

  if state.arrow_right_enabled && keycode == KEYCODE_ARROW_RIGHT {
    return if is_keydown {
      OverlayHotkeyAction::HistoryExpand
    } else {
      OverlayHotkeyAction::ClaimOnly
    };
  }

  OverlayHotkeyAction::PassThrough
}

#[cfg(test)]
mod tests {
  use super::{
    OverlayHotkeyAction, OverlayHotkeyState, SpaceHotkeyAction, SpaceHotkeyDecision,
    overlay_hotkey_decision, space_hotkey_decision,
  };
  use crate::platform::{KEYCODE_ARROW_DOWN, KEYCODE_ESCAPE, KEYCODE_RETURN, MOD_OPTION};

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
  fn overlay_escape_is_claimed_when_enabled() {
    let state = OverlayHotkeyState {
      escape_enabled: true,
      enter_enabled: false,
      arrows_enabled: false,
      arrow_left_enabled: false,
      arrow_right_enabled: false,
    };

    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_ESCAPE, false, false, true),
      OverlayHotkeyAction::Cancel
    );
    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_ESCAPE, false, false, false),
      OverlayHotkeyAction::ClaimOnly
    );
  }

  #[test]
  fn overlay_enter_is_claimed_unless_shift_is_held() {
    let state = OverlayHotkeyState {
      escape_enabled: false,
      enter_enabled: true,
      arrows_enabled: false,
      arrow_left_enabled: false,
      arrow_right_enabled: false,
    };

    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_RETURN, false, false, true),
      OverlayHotkeyAction::Finalize { raw_requested: false }
    );
    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_RETURN, true, false, true),
      OverlayHotkeyAction::Finalize { raw_requested: true }
    );
    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_RETURN, false, false, false),
      OverlayHotkeyAction::ClaimOnly
    );
    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_RETURN, false, true, true),
      OverlayHotkeyAction::PassThrough
    );
  }

  #[test]
  fn overlay_arrow_navigation_is_claimed_when_enabled() {
    let state = OverlayHotkeyState {
      escape_enabled: false,
      enter_enabled: false,
      arrows_enabled: true,
      arrow_left_enabled: false,
      arrow_right_enabled: false,
    };

    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_ARROW_DOWN, false, false, true),
      OverlayHotkeyAction::Navigate(1)
    );
    assert_eq!(
      overlay_hotkey_decision(state, KEYCODE_ARROW_DOWN, false, false, false),
      OverlayHotkeyAction::ClaimOnly
    );
  }
}
