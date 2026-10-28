use std::time::{Duration, Instant};

use cocoa::appkit::NSPasteboard;
use cocoa::base::{id, nil};
use cocoa::foundation::NSString;
#[cfg(not(test))]
use core_graphics::event::CGEventFlags;
use core_graphics::event::{CGEvent, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc::{class, msg_send, sel, sel_impl};

use crate::settings::{AutoSubmitMode, PasteMethod};
use crate::typing::{TypingCursor, TypingProgress};

#[cfg(not(test))]
use super::KEYCODE_RETURN;
use super::{AZAD_SYNTHETIC_MARKER, KCG_EVENT_SOURCE_USER_DATA_FIELD};
use super::{ensure_accessibility_for_auto_paste, nsstring_to_string};

const KEYCODE_DIRECT_INPUT: u16 = 0x00;
const KEYCODE_LEFT_COMMAND: u16 = 0x37;
const KEYCODE_RIGHT_COMMAND: u16 = 0x36;
const KEYCODE_LEFT_SHIFT: u16 = 0x38;
const KEYCODE_RIGHT_SHIFT: u16 = 0x3C;
const KEYCODE_LEFT_OPTION: u16 = 0x3A;
const KEYCODE_RIGHT_OPTION: u16 = 0x3D;
const KEYCODE_LEFT_CONTROL: u16 = 0x3B;
const KEYCODE_RIGHT_CONTROL: u16 = 0x3E;
const PASTE_CHORD_HOLD_MS: u64 = 100;

// Device-specific modifier bits from IOKit's NX_DEVICE*KEYMASK. Real hardware modifier presses
// set both the high-level MaskX bit AND the device-specific bit. macOS Screen Sharing forwards
// events only when the device bit is present.
#[cfg(not(test))]
const NX_DEVICELCTLKEYMASK: u64 = 0x0000_0001;
#[cfg(not(test))]
const NX_DEVICELSHIFTKEYMASK: u64 = 0x0000_0002;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteResult {
  Pasted,
  EmptyText,
  ClipboardWriteFailed,
  InputEventFailed,
  AccessibilityRequired,
  Cancelled,
}

pub enum InsertStart {
  Finished(PasteResult),
  Paced(PacedTyping),
  PacedFailed(PasteResult),
}

pub struct PacedTyping {
  cursor: TypingCursor,
  copy: bool,
}

#[cfg(not(test))]
thread_local! {
  static TYPING_SOURCE: std::cell::RefCell<Option<CGEventSource>> = const { std::cell::RefCell::new(None) };
}

impl PacedTyping {
  fn new(text: String, method: PasteMethod, rate: u32) -> Result<Self, PasteResult> {
    if text.trim().is_empty() {
      return Err(PasteResult::EmptyText);
    }
    #[cfg(not(test))]
    {
      if !ensure_accessibility_for_auto_paste() {
        return Err(PasteResult::AccessibilityRequired);
      }
      let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| PasteResult::InputEventFailed)?;
      // SAFETY: the source belongs to this insertion on the main thread.
      unsafe { release_modifiers(&source) };
      TYPING_SOURCE.with(|slot| slot.replace(Some(source)));
    }
    Ok(Self {
      cursor: TypingCursor::new(text, rate, Instant::now()),
      copy: method == PasteMethod::DirectTypingAndCopyClipboard,
    })
  }

  pub fn advance(&mut self, now: Instant) -> TypingProgress {
    #[cfg(not(test))]
    let progress = self.cursor.advance(now, |ch| {
      TYPING_SOURCE.with(|slot| {
        let source = slot.borrow();
        let source = source.as_ref().ok_or(())?;
        let (down, up) = direct_key_pair(source, ch)?;
        down.post(CGEventTapLocation::HID);
        up.post(CGEventTapLocation::HID);
        Ok(Instant::now())
      })
    });
    #[cfg(test)]
    let progress = self.cursor.advance(now, |ch| {
      TEST_PAIRS_BEFORE_FAILURE.with(|slot| {
        let mut remaining = slot.borrow_mut();
        if let Some(count) = remaining.as_mut() {
          if *count == 0 {
            return Err(());
          }
          *count -= 1;
        }
        Ok(())
      })?;
      record_test_output(TestOutput::Pair(ch));
      Ok(now)
    });
    if progress == TypingProgress::Complete && self.copy {
      #[cfg(not(test))]
      // SAFETY: the clipboard attempt follows the final pair on the main thread.
      if !unsafe { write_pasteboard_string(self.cursor.text()) } {
        eprintln!("Azad: direct input succeeded but failed to copy text to pasteboard");
      }
      #[cfg(test)]
      record_test_output(TestOutput::Copy(self.cursor.text().into()));
    }
    progress
  }
}

impl Drop for PacedTyping {
  fn drop(&mut self) {
    #[cfg(not(test))]
    TYPING_SOURCE.with(|slot| slot.replace(None));
  }
}

pub(super) fn direct_key_pair(source: &CGEventSource, ch: char) -> Result<(CGEvent, CGEvent), ()> {
  let down = CGEvent::new_keyboard_event(source.clone(), KEYCODE_DIRECT_INPUT, true)?;
  let up = CGEvent::new_keyboard_event(source.clone(), KEYCODE_DIRECT_INPUT, false)?;
  down.set_string(&ch.to_string());
  for event in [&down, &up] {
    event.set_integer_value_field(KCG_EVENT_SOURCE_USER_DATA_FIELD, AZAD_SYNTHETIC_MARKER);
  }
  Ok((down, up))
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub enum TestOutput {
  Pair(char),
  Copy(String),
  Insert(String, PasteMethod),
  Submit(AutoSubmitMode),
}

#[cfg(test)]
thread_local! {
  static TEST_OUTPUT: std::cell::RefCell<Vec<TestOutput>> = const { std::cell::RefCell::new(Vec::new()) };
  static TEST_PAIRS_BEFORE_FAILURE: std::cell::RefCell<Option<usize>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn fail_typing_after_for_test(pairs: Option<usize>) {
  TEST_PAIRS_BEFORE_FAILURE.with(|slot| slot.replace(pairs));
}

#[cfg(test)]
fn record_test_output(event: TestOutput) {
  TEST_OUTPUT.with(|slot| slot.borrow_mut().push(event));
}

#[cfg(test)]
pub fn take_test_output() -> Vec<TestOutput> {
  TEST_OUTPUT.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
}

pub fn start_insert(
  text: &str,
  method: PasteMethod,
  paste_delay_ms: u64,
  key_rate: u32,
) -> InsertStart {
  if key_rate > 0 && method != PasteMethod::ClipboardPaste {
    #[cfg(not(test))]
    // SAFETY: this only inspects the current foreground application.
    let terminal =
      unsafe { frontmost_bundle_id().is_some_and(|bundle| is_terminal_like_bundle_id(&bundle)) };
    #[cfg(test)]
    let terminal = false;
    if !terminal {
      return match PacedTyping::new(text.to_string(), method, key_rate) {
        Ok(typing) => InsertStart::Paced(typing),
        Err(result) => InsertStart::PacedFailed(result),
      };
    }
  }
  InsertStart::Finished(insert_text(text, method, paste_delay_ms))
}

pub fn insert_text(text: &str, method: PasteMethod, paste_delay_ms: u64) -> PasteResult {
  if text.trim().is_empty() {
    return PasteResult::EmptyText;
  }

  // In test builds, report success without touching the real Accessibility /
  // clipboard / keystroke FFI. App-logic tests exercise the post-paste state
  // transitions; routing them through live paths would inject real Cmd+V
  // keystrokes into whatever app is focused during `cargo test`.
  if cfg!(test) {
    #[cfg(test)]
    record_test_output(TestOutput::Insert(text.into(), method));
    return PasteResult::Pasted;
  }

  if !ensure_accessibility_for_auto_paste() {
    eprintln!("Azad: insert skipped due to missing Accessibility permission");
    return PasteResult::AccessibilityRequired;
  }

  let force_clipboard_bundle =
    if matches!(method, PasteMethod::DirectTyping | PasteMethod::DirectTypingAndCopyClipboard) {
      unsafe { frontmost_bundle_id().filter(|bundle| is_terminal_like_bundle_id(bundle)) }
    } else {
      None
    };

  unsafe {
    match method {
      PasteMethod::ClipboardPaste => {
        if !write_pasteboard_string(text) {
          eprintln!("Azad: failed to write transcript to pasteboard");
          return PasteResult::ClipboardWriteFailed;
        }
        nudge_screen_sharing_clipboard_sync();
        std::thread::sleep(Duration::from_millis(paste_delay_ms));
        send_command_v(text);
      }
      PasteMethod::DirectTyping => {
        if let Some(bundle) = force_clipboard_bundle.as_deref() {
          eprintln!(
            "Azad: direct typing fallback to clipboard paste for frontmost app bundle={bundle}"
          );
          if !write_pasteboard_string(text) {
            eprintln!("Azad: failed to write transcript to pasteboard");
            return PasteResult::ClipboardWriteFailed;
          }
          nudge_screen_sharing_clipboard_sync();
          std::thread::sleep(Duration::from_millis(paste_delay_ms));
          send_command_v(text);
        } else if !send_direct_text_input(text) {
          eprintln!("Azad: failed to send direct text input");
          return PasteResult::InputEventFailed;
        }
      }
      PasteMethod::DirectTypingAndCopyClipboard => {
        if let Some(bundle) = force_clipboard_bundle.as_deref() {
          eprintln!(
            "Azad: direct typing+copy fallback to clipboard paste for frontmost app bundle={bundle}"
          );
          if !write_pasteboard_string(text) {
            eprintln!("Azad: failed to write transcript to pasteboard");
            return PasteResult::ClipboardWriteFailed;
          }
          nudge_screen_sharing_clipboard_sync();
          std::thread::sleep(Duration::from_millis(paste_delay_ms));
          send_command_v(text);
        } else {
          if !send_direct_text_input(text) {
            eprintln!("Azad: failed to send direct text input");
            return PasteResult::InputEventFailed;
          }
          if !write_pasteboard_string(text) {
            eprintln!("Azad: direct input succeeded but failed to copy text to pasteboard");
          }
        }
      }
    }
  }

  PasteResult::Pasted
}

#[cfg(test)]
pub fn send_auto_submit(mode: AutoSubmitMode) -> bool {
  record_test_output(TestOutput::Submit(mode));
  true
}

#[cfg(not(test))]
pub fn send_auto_submit(mode: AutoSubmitMode) -> bool {
  match mode {
    AutoSubmitMode::Off => true,
    AutoSubmitMode::Enter => unsafe { send_key_chord(KEYCODE_RETURN, CGEventFlags::empty()) },
    AutoSubmitMode::CtrlEnter => unsafe {
      send_key_chord(KEYCODE_RETURN, CGEventFlags::CGEventFlagControl)
    },
    AutoSubmitMode::ShiftEnter => unsafe {
      send_key_chord(KEYCODE_RETURN, CGEventFlags::CGEventFlagShift)
    },
  }
}

/// Move focus to the first search hit (Down) then commit with Return. Used when
/// Spotify's search field still has focus after `spotify:search:…`.
#[cfg(test)]
pub fn post_down_then_return() -> bool {
  true
}

#[cfg(not(test))]
pub fn post_down_then_return() -> bool {
  if !ensure_accessibility_for_auto_paste() {
    return false;
  }
  const KEYCODE_ARROW_DOWN: u16 = 0x7D;
  let down_ok = unsafe { send_key_chord(KEYCODE_ARROW_DOWN, CGEventFlags::empty()) };
  std::thread::sleep(Duration::from_millis(80));
  let ret_ok = unsafe { send_key_chord(KEYCODE_RETURN, CGEventFlags::empty()) };
  down_ok && ret_ok
}

fn is_terminal_like_bundle_id(bundle_id: &str) -> bool {
  matches!(
    bundle_id,
    "com.apple.Terminal"
      | "com.googlecode.iterm2"
      | "com.github.wez.wezterm"
      | "dev.warp.Warp-Stable"
      | "dev.warp.Warp"
      | "net.kovidgoyal.kitty"
      | "org.alacritty"
      | "io.alacritty"
      | "com.mitchellh.ghostty"
  )
}

unsafe fn send_direct_text_input(text: &str) -> bool {
  let source = match CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
    Ok(source) => source,
    Err(_) => return false,
  };
  release_modifiers(&source);

  // Dispatch per-character Unicode key events. Some targets ignore or truncate
  // multi-character Unicode payloads in a single CGEvent.
  for ch in text.chars() {
    let Ok((key_down, key_up)) = direct_key_pair(&source, ch) else { return false };
    key_down.post(CGEventTapLocation::HID);
    key_up.post(CGEventTapLocation::HID);
  }
  true
}

#[cfg(not(test))]
unsafe fn send_key_chord(keycode: u16, flags: CGEventFlags) -> bool {
  let source = match CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
    Ok(source) => source,
    Err(_) => return false,
  };

  release_modifiers(&source);

  let (modifier_key, device_bit) = if flags.contains(CGEventFlags::CGEventFlagControl) {
    (Some(KEYCODE_LEFT_CONTROL), NX_DEVICELCTLKEYMASK)
  } else if flags.contains(CGEventFlags::CGEventFlagShift) {
    (Some(KEYCODE_LEFT_SHIFT), NX_DEVICELSHIFTKEYMASK)
  } else {
    (None, 0)
  };

  let chord_flags = if flags.is_empty() {
    flags
  } else {
    CGEventFlags::from_bits_truncate(flags.bits() | device_bit)
  };

  let stamp = |event: &CGEvent| {
    event.set_integer_value_field(KCG_EVENT_SOURCE_USER_DATA_FIELD, AZAD_SYNTHETIC_MARKER);
  };

  if let Some(modifier_key) = modifier_key {
    let Ok(mod_down) = CGEvent::new_keyboard_event(source.clone(), modifier_key, true) else {
      return false;
    };
    mod_down.set_flags(chord_flags);
    stamp(&mod_down);
    mod_down.post(CGEventTapLocation::HID);
  }

  let Ok(key_down) = CGEvent::new_keyboard_event(source.clone(), keycode, true) else {
    return false;
  };
  if !chord_flags.is_empty() {
    key_down.set_flags(chord_flags);
  }
  stamp(&key_down);
  key_down.post(CGEventTapLocation::HID);

  let Ok(key_up) = CGEvent::new_keyboard_event(source.clone(), keycode, false) else {
    return false;
  };
  if !chord_flags.is_empty() {
    key_up.set_flags(chord_flags);
  }
  stamp(&key_up);
  key_up.post(CGEventTapLocation::HID);

  if let Some(modifier_key) = modifier_key {
    if let Ok(mod_up) = CGEvent::new_keyboard_event(source, modifier_key, false) {
      stamp(&mod_up);
      mod_up.post(CGEventTapLocation::HID);
    }
  }

  true
}

unsafe fn send_command_v(text: &str) {
  use enigo::{Direction, Enigo, Key, Keyboard, Settings};

  trace_paste_handoff("before_shortcut", text);
  let settings =
    Settings { event_source_user_data: Some(AZAD_SYNTHETIC_MARKER), ..Settings::default() };
  let mut enigo = match Enigo::new(&settings) {
    Ok(e) => e,
    Err(err) => {
      eprintln!("Azad: enigo init failed for Cmd+V paste: {err}");
      return;
    }
  };

  if let Err(err) = enigo.key(Key::Meta, Direction::Press) {
    eprintln!("Azad: enigo Cmd down failed: {err}");
    return;
  }
  // `Key::Other(9)` is the physical V keycode on macOS. That keeps paste
  // working on non-US layouts.
  if let Err(err) = enigo.key(Key::Other(9), Direction::Click) {
    eprintln!("Azad: enigo V click failed: {err}");
  }
  std::thread::sleep(Duration::from_millis(PASTE_CHORD_HOLD_MS));
  if let Err(err) = enigo.key(Key::Meta, Direction::Release) {
    eprintln!("Azad: enigo Cmd up failed: {err}");
  }
  drop(enigo);
  trace_paste_handoff("after_shortcut", text);
}

unsafe fn release_modifiers(source: &CGEventSource) {
  for key in [
    KEYCODE_LEFT_SHIFT,
    KEYCODE_RIGHT_SHIFT,
    KEYCODE_LEFT_OPTION,
    KEYCODE_RIGHT_OPTION,
    KEYCODE_LEFT_CONTROL,
    KEYCODE_RIGHT_CONTROL,
    KEYCODE_LEFT_COMMAND,
    KEYCODE_RIGHT_COMMAND,
  ] {
    if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), key, false) {
      event.post(CGEventTapLocation::HID);
    }
  }
}

unsafe fn write_pasteboard_string(text: &str) -> bool {
  let pasteboard = NSPasteboard::generalPasteboard(nil);
  let _: usize = msg_send![pasteboard, clearContents];
  let ns_text = NSString::alloc(nil).init_str(text);
  let array: id = msg_send![class!(NSArray), arrayWithObject: ns_text];
  let ok: i8 = msg_send![pasteboard, writeObjects: array];
  trace_paste_handoff("clipboard_write", text);
  ok != 0
}

/// Record producer-side paste state when debug stats are enabled, without text contents.
/// These snapshots do not confirm delivery to the receiving application.
unsafe fn trace_paste_handoff(stage: &str, expected: &str) {
  if !super::overlay_debug_logs_enabled() {
    return;
  }
  // SAFETY: this is the main-thread insertion path; these queries do not request permissions.
  let (change_count, foreground, post_access) = unsafe {
    let board = NSPasteboard::generalPasteboard(nil);
    let change_count: isize = msg_send![board, changeCount];
    (change_count, frontmost_bundle_id(), CGPreflightPostEventAccess())
  };
  let ts_ms = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map_or(0, |duration| duration.as_millis());
  eprintln!(
    "AZAD_PASTE_HANDOFF ts_ms={ts_ms} stage={stage} foreground={foreground:?} \
     post_access={post_access} change_count={change_count} \
     chars={}",
    expected.chars().count(),
  );
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
  fn CGPreflightPostEventAccess() -> bool;
}

unsafe fn nudge_screen_sharing_clipboard_sync() {
  let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
  if workspace == nil {
    return;
  }
  let frontmost: id = msg_send![workspace, frontmostApplication];
  if frontmost == nil {
    return;
  }

  let bundle_id: id = msg_send![frontmost, bundleIdentifier];
  let Some(bundle) = nsstring_to_string(bundle_id) else {
    return;
  };
  if bundle != "com.apple.ScreenSharing" {
    return;
  }

  const ACTIVATE_IGNORING_OTHER_APPS: u64 = 1 << 1;

  let current: id = msg_send![class!(NSRunningApplication), currentApplication];
  if current == nil {
    return;
  }

  let _: bool = msg_send![current, activateWithOptions: ACTIVATE_IGNORING_OTHER_APPS];
  std::thread::sleep(Duration::from_millis(60));
  let _: bool = msg_send![frontmost, activateWithOptions: ACTIVATE_IGNORING_OTHER_APPS];
  std::thread::sleep(Duration::from_millis(100));
}

unsafe fn frontmost_bundle_id() -> Option<String> {
  let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
  if workspace == nil {
    return None;
  }
  let frontmost: id = msg_send![workspace, frontmostApplication];
  if frontmost == nil {
    return None;
  }
  let bundle_id: id = msg_send![frontmost, bundleIdentifier];
  nsstring_to_string(bundle_id)
}

#[cfg(test)]
mod tests {
  use super::is_terminal_like_bundle_id;

  #[test]
  fn terminal_like_bundle_ids_force_clipboard_fallback() {
    assert!(is_terminal_like_bundle_id("com.apple.Terminal"));
    assert!(is_terminal_like_bundle_id("com.mitchellh.ghostty"));
    assert!(is_terminal_like_bundle_id("dev.warp.Warp"));
    assert!(!is_terminal_like_bundle_id("com.apple.TextEdit"));
  }
}
