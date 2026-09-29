//! Resolves history-search keys captured by the helper into text with the active layout.
//!
//! The helper reports HID usages; the text a key produces depends on the current input source,
//! modifier state, Caps Lock and any pending dead key, so it is resolved here with
//! `UCKeyTranslate`, as AppKit would for a key event. Must run on the main thread (Text Input
//! Sources).

use std::cell::Cell;
use std::ffi::c_void;

use azad_capture::policy::{MOD_OPTION, MOD_SHIFT};

const KEYBOARD_TYPE_ISO: u32 = 1;
const UC_KEY_ACTION_DOWN: u16 = 0;
const SHIFT_KEY: u32 = 0x0200 >> 8;
const ALPHA_LOCK: u32 = 0x0400 >> 8;
const OPTION_KEY: u32 = 0x0800 >> 8;
const CG_EVENT_SOURCE_STATE_HID_SYSTEM: i32 = 1;
const CG_EVENT_FLAG_ALPHA_SHIFT: u64 = 0x0001_0000;

thread_local! {
  static DEAD_KEY_STATE: Cell<u32> = const { Cell::new(0) };
}

/// macOS virtual keycode for a HID keyboard-page text usage, on an ANSI keyboard.
fn virtual_keycode(usage: u16) -> Option<u16> {
  const LETTERS: [u16; 26] = [
    0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F, 0x23,
    0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
  ];
  const DIGITS: [u16; 10] = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19, 0x1D];
  Some(match usage {
    0x04..=0x1D => LETTERS[(usage - 0x04) as usize],
    0x1E..=0x27 => DIGITS[(usage - 0x1E) as usize],
    0x2C => 0x31,
    0x2D => 0x1B,
    0x2E => 0x18,
    0x2F => 0x21,
    0x30 => 0x1E,
    0x31 | 0x32 => 0x2A,
    0x33 => 0x29,
    0x34 => 0x27,
    0x35 => 0x32,
    0x36 => 0x2B,
    0x37 => 0x2F,
    0x38 => 0x2C,
    0x54 => 0x4B,
    0x55 => 0x43,
    0x56 => 0x4E,
    0x57 => 0x45,
    0x59 => 0x53,
    0x5A => 0x54,
    0x5B => 0x55,
    0x5C => 0x56,
    0x5D => 0x57,
    0x5E => 0x58,
    0x5F => 0x59,
    0x60 => 0x5B,
    0x61 => 0x5C,
    0x62 => 0x52,
    0x63 => 0x41,
    0x64 => 0x0A,
    0x67 => 0x51,
    0x85 => 0x5F,
    0x87 => 0x5E,
    0x88 => 0x68,
    0x89 => 0x5D,
    _ => return None,
  })
}

/// ISO keyboards report the key left of 1 and the key right of left Shift swapped relative to
/// the ANSI keycodes; macOS swaps them back for ISO keyboard types.
fn layout_keycode(usage: u16, keyboard_type: u32) -> Option<u16> {
  let iso = keyboard_type == KEYBOARD_TYPE_ISO;
  match usage {
    0x35 if iso => Some(0x0A),
    0x64 if iso => Some(0x32),
    _ => virtual_keycode(usage),
  }
}

/// Text for a captured search key, or `None` when it produces nothing printable (including the
/// first half of a dead-key sequence, which is kept for the next key).
pub fn search_key_text(usage: u16, modifiers: u8) -> Option<String> {
  // SAFETY: Called on the main thread; every CF object copied here is released.
  unsafe {
    let kbd_type = LMGetKbdType() as u32;
    let keycode = layout_keycode(usage, KBGetLayoutType(kbd_type as i16) as u32)?;
    let source = TISCopyCurrentKeyboardLayoutInputSource();
    if source.is_null() {
      return None;
    }
    let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
    let text = if data.is_null() {
      None
    } else {
      let layout = CFDataGetBytePtr(data);
      let mut state = 0u32;
      if modifiers & MOD_SHIFT != 0 {
        state |= SHIFT_KEY;
      }
      if modifiers & MOD_OPTION != 0 {
        state |= OPTION_KEY;
      }
      if CGEventSourceFlagsState(CG_EVENT_SOURCE_STATE_HID_SYSTEM) & CG_EVENT_FLAG_ALPHA_SHIFT != 0
      {
        state |= ALPHA_LOCK;
      }
      let mut dead = DEAD_KEY_STATE.with(Cell::get);
      let mut buffer = [0u16; 8];
      let mut length = 0usize;
      let status = UCKeyTranslate(
        layout.cast(),
        keycode,
        UC_KEY_ACTION_DOWN,
        state,
        kbd_type,
        0,
        &mut dead,
        buffer.len(),
        &mut length,
        buffer.as_mut_ptr(),
      );
      DEAD_KEY_STATE.with(|cell| cell.set(dead));
      (status == 0 && length > 0).then(|| String::from_utf16_lossy(&buffer[..length]))
    };
    CFRelease(source);
    text.filter(|text| !text.chars().any(char::is_control))
  }
}

/// Drops a pending dead key, e.g. when history search ends.
pub fn reset_search_key_state() {
  DEAD_KEY_STATE.with(|cell| cell.set(0));
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
  static kTISPropertyUnicodeKeyLayoutData: *const c_void;
  fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
  fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
  fn LMGetKbdType() -> u8;
  fn KBGetLayoutType(keyboard_type: i16) -> i32;
  fn UCKeyTranslate(
    layout: *const c_void,
    virtual_key_code: u16,
    key_action: u16,
    modifier_key_state: u32,
    keyboard_type: u32,
    key_translate_options: u32,
    dead_key_state: *mut u32,
    max_string_length: usize,
    actual_string_length: *mut usize,
    unicode_string: *mut u16,
  ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
  fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
  fn CFRelease(value: *const c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
  fn CGEventSourceFlagsState(state: i32) -> u64;
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn letters_digits_and_punctuation_map_to_ansi_keycodes() {
    assert_eq!(virtual_keycode(0x04), Some(0x00));
    assert_eq!(virtual_keycode(0x1D), Some(0x06));
    assert_eq!(virtual_keycode(0x1E), Some(0x12));
    assert_eq!(virtual_keycode(0x27), Some(0x1D));
    assert_eq!(virtual_keycode(0x2C), Some(0x31));
    assert_eq!(virtual_keycode(0x38), Some(0x2C));
    assert_eq!(virtual_keycode(0x28), None);
  }

  #[test]
  fn iso_keyboards_swap_grave_and_section() {
    assert_eq!(layout_keycode(0x35, 0), Some(0x32));
    assert_eq!(layout_keycode(0x35, KEYBOARD_TYPE_ISO), Some(0x0A));
    assert_eq!(layout_keycode(0x64, KEYBOARD_TYPE_ISO), Some(0x32));
  }

  #[test]
  fn every_policy_text_key_has_a_keycode() {
    for usage in 0u16..=0xFF {
      if azad_capture::policy::is_text_key(usage) {
        assert!(virtual_keycode(usage).is_some(), "usage {usage:#x}");
      }
    }
  }
}
