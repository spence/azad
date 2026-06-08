#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasteMethod {
  #[default]
  ClipboardPaste,
  DirectTyping,
  DirectTypingAndCopyClipboard,
}

impl PasteMethod {
  pub fn from_prefs_value(value: &str) -> Self {
    match value {
      "direct_typing" => Self::DirectTyping,
      "direct_typing_copy_clipboard" => Self::DirectTypingAndCopyClipboard,
      _ => Self::ClipboardPaste,
    }
  }

  pub fn prefs_value(self) -> &'static str {
    match self {
      Self::ClipboardPaste => "clipboard_paste",
      Self::DirectTyping => "direct_typing",
      Self::DirectTypingAndCopyClipboard => "direct_typing_copy_clipboard",
    }
  }

  pub fn from_ui_index(index: i64) -> Self {
    match index {
      1 => Self::DirectTyping,
      2 => Self::DirectTypingAndCopyClipboard,
      _ => Self::ClipboardPaste,
    }
  }

  pub fn ui_index(self) -> i64 {
    match self {
      Self::ClipboardPaste => 0,
      Self::DirectTyping => 1,
      Self::DirectTypingAndCopyClipboard => 2,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoSubmitMode {
  #[default]
  Off,
  Enter,
  CtrlEnter,
  ShiftEnter,
}

impl AutoSubmitMode {
  pub fn from_prefs_value(value: &str) -> Self {
    match value {
      "enter" => Self::Enter,
      "ctrl_enter" => Self::CtrlEnter,
      "shift_enter" => Self::ShiftEnter,
      _ => Self::Off,
    }
  }

  pub fn prefs_value(self) -> &'static str {
    match self {
      Self::Off => "off",
      Self::Enter => "enter",
      Self::CtrlEnter => "ctrl_enter",
      Self::ShiftEnter => "shift_enter",
    }
  }

  pub fn from_ui_index(index: i64) -> Self {
    match index {
      1 => Self::Enter,
      2 => Self::CtrlEnter,
      3 => Self::ShiftEnter,
      _ => Self::Off,
    }
  }

  pub fn ui_index(self) -> i64 {
    match self {
      Self::Off => 0,
      Self::Enter => 1,
      Self::CtrlEnter => 2,
      Self::ShiftEnter => 3,
    }
  }
}

/// Which display the speaking overlay appears on. All modes keep the existing
/// top-center anchor; they differ only in which screen is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayPosition {
  /// The screen under the mouse cursor (the app's original hardcoded behavior).
  #[default]
  FollowCursor,
  /// The primary display (the one carrying the menu bar).
  PrimaryMonitor,
  /// The display containing the focused window of the frontmost app.
  ActiveWindow,
}

impl OverlayPosition {
  pub fn from_prefs_value(value: &str) -> Self {
    match value {
      "primary_monitor" => Self::PrimaryMonitor,
      "active_window" => Self::ActiveWindow,
      _ => Self::FollowCursor,
    }
  }

  pub fn prefs_value(self) -> &'static str {
    match self {
      Self::FollowCursor => "follow_cursor",
      Self::PrimaryMonitor => "primary_monitor",
      Self::ActiveWindow => "active_window",
    }
  }

  pub fn from_ui_index(index: i64) -> Self {
    match index {
      1 => Self::PrimaryMonitor,
      2 => Self::ActiveWindow,
      _ => Self::FollowCursor,
    }
  }

  pub fn ui_index(self) -> i64 {
    match self {
      Self::FollowCursor => 0,
      Self::PrimaryMonitor => 1,
      Self::ActiveWindow => 2,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{AutoSubmitMode, OverlayPosition, PasteMethod};

  #[test]
  fn paste_method_roundtrips_preferences_values() {
    assert_eq!(
      PasteMethod::from_prefs_value(PasteMethod::ClipboardPaste.prefs_value()),
      PasteMethod::ClipboardPaste
    );
    assert_eq!(
      PasteMethod::from_prefs_value(PasteMethod::DirectTyping.prefs_value()),
      PasteMethod::DirectTyping
    );
    assert_eq!(
      PasteMethod::from_prefs_value(PasteMethod::DirectTypingAndCopyClipboard.prefs_value()),
      PasteMethod::DirectTypingAndCopyClipboard
    );
  }

  #[test]
  fn paste_method_invalid_pref_defaults_to_clipboard() {
    assert_eq!(PasteMethod::from_prefs_value("not_a_real_value"), PasteMethod::ClipboardPaste);
  }

  #[test]
  fn auto_submit_roundtrips_preferences_values() {
    for mode in [
      AutoSubmitMode::Off,
      AutoSubmitMode::Enter,
      AutoSubmitMode::CtrlEnter,
      AutoSubmitMode::ShiftEnter,
    ] {
      assert_eq!(AutoSubmitMode::from_prefs_value(mode.prefs_value()), mode);
    }
  }

  #[test]
  fn auto_submit_invalid_pref_defaults_off() {
    assert_eq!(AutoSubmitMode::from_prefs_value("invalid"), AutoSubmitMode::Off);
  }

  #[test]
  fn overlay_position_roundtrips_preferences_values() {
    for pos in [
      OverlayPosition::FollowCursor,
      OverlayPosition::PrimaryMonitor,
      OverlayPosition::ActiveWindow,
    ] {
      assert_eq!(OverlayPosition::from_prefs_value(pos.prefs_value()), pos);
      assert_eq!(OverlayPosition::from_ui_index(pos.ui_index()), pos);
    }
  }

  #[test]
  fn overlay_position_invalid_pref_defaults_to_follow_cursor() {
    assert_eq!(OverlayPosition::from_prefs_value("nope"), OverlayPosition::FollowCursor);
    assert_eq!(OverlayPosition::from_ui_index(99), OverlayPosition::FollowCursor);
  }
}
