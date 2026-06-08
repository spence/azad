use std::ffi::CStr;
use std::os::raw::c_char;

use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};

use crate::settings::{AutoSubmitMode, PasteMethod};

const PREFERRED_DEVICE_KEY: &str = "AzadPreferredInputDeviceId";
const ALWAYS_LISTENING_KEY: &str = "AzadAlwaysListeningEnabled";
const DEBUG_STATS_ENABLED_KEY: &str = "AzadDebugStatsEnabled";
const RUN_ON_STARTUP_KEY: &str = "AzadRunOnStartup";
const PASTE_METHOD_KEY: &str = "AzadPasteMethod";
const AUTO_SUBMIT_MODE_KEY: &str = "AzadAutoSubmit";
const APPEND_TRAILING_SPACE_KEY: &str = "AzadAppendTrailingSpaceOnPaste";
const ACTIVE_MODEL_PACK_KEY: &str = "AzadActiveModelPack";
const REMOVED_WORDS_KEY: &str = "AzadRemovedWords";
const ONBOARDING_COMPLETE_KEY: &str = "AzadOnboardingComplete";

pub fn load_preferred_device_id() -> Option<String> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return None;
    }

    let key = NSString::alloc(nil).init_str(PREFERRED_DEVICE_KEY);
    let value: id = msg_send![defaults, stringForKey: key];
    nsstring_to_string(value)
  }
}

pub fn save_preferred_device_id(device_id: &str) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(PREFERRED_DEVICE_KEY);
    let value = NSString::alloc(nil).init_str(device_id);
    let _: () = msg_send![defaults, setObject: value forKey: key];
  }
}

pub fn load_always_listening_enabled() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return true;
    }

    let key = NSString::alloc(nil).init_str(ALWAYS_LISTENING_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return true;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_always_listening_enabled(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(ALWAYS_LISTENING_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_debug_stats_enabled() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return false;
    }

    let key = NSString::alloc(nil).init_str(DEBUG_STATS_ENABLED_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return false;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_debug_stats_enabled(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(DEBUG_STATS_ENABLED_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_run_on_startup_enabled() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return true;
    }

    let key = NSString::alloc(nil).init_str(RUN_ON_STARTUP_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return true;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_run_on_startup_enabled(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(RUN_ON_STARTUP_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

/// Returns `None` when the user has never been through onboarding (key absent),
/// so the bootstrap seeding can distinguish a fresh profile from one that
/// explicitly completed (or is mid-) onboarding.
pub fn load_onboarding_complete() -> Option<bool> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return None;
    }

    let key = NSString::alloc(nil).init_str(ONBOARDING_COMPLETE_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return None;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    Some(value != 0)
  }
}

pub fn save_onboarding_complete(complete: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(ONBOARDING_COMPLETE_KEY);
    let value = if complete { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_paste_method() -> PasteMethod {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return PasteMethod::default();
    }

    let key = NSString::alloc(nil).init_str(PASTE_METHOD_KEY);
    let value: id = msg_send![defaults, stringForKey: key];
    let Some(value) = nsstring_to_string(value) else {
      return PasteMethod::default();
    };
    PasteMethod::from_prefs_value(value.trim())
  }
}

pub fn save_paste_method(method: PasteMethod) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(PASTE_METHOD_KEY);
    let value = NSString::alloc(nil).init_str(method.prefs_value());
    let _: () = msg_send![defaults, setObject: value forKey: key];
  }
}

pub fn load_auto_submit_mode() -> AutoSubmitMode {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return AutoSubmitMode::default();
    }

    let key = NSString::alloc(nil).init_str(AUTO_SUBMIT_MODE_KEY);
    let value: id = msg_send![defaults, stringForKey: key];
    let Some(value) = nsstring_to_string(value) else {
      return AutoSubmitMode::default();
    };
    AutoSubmitMode::from_prefs_value(value.trim())
  }
}

pub fn save_auto_submit_mode(mode: AutoSubmitMode) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(AUTO_SUBMIT_MODE_KEY);
    let value = NSString::alloc(nil).init_str(mode.prefs_value());
    let _: () = msg_send![defaults, setObject: value forKey: key];
  }
}

pub fn load_append_trailing_space_on_paste() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return true;
    }

    let key = NSString::alloc(nil).init_str(APPEND_TRAILING_SPACE_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return true;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_append_trailing_space_on_paste(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(APPEND_TRAILING_SPACE_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_active_model_pack() -> Option<String> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return None;
    }

    let key = NSString::alloc(nil).init_str(ACTIVE_MODEL_PACK_KEY);
    let value: id = msg_send![defaults, stringForKey: key];
    nsstring_to_string(value)
  }
}

pub fn save_active_model_pack(pack_id: &str) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(ACTIVE_MODEL_PACK_KEY);
    let value = NSString::alloc(nil).init_str(pack_id);
    let _: () = msg_send![defaults, setObject: value forKey: key];
  }
}

const DEFAULT_REMOVED_WORDS: &[&str] = &["um", "ah"];

pub fn load_removed_words() -> Vec<String> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return DEFAULT_REMOVED_WORDS.iter().map(|s| s.to_string()).collect();
    }

    let key = NSString::alloc(nil).init_str(REMOVED_WORDS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return DEFAULT_REMOVED_WORDS.iter().map(|s| s.to_string()).collect();
    }

    let value: id = msg_send![defaults, stringForKey: key];
    let Some(value) = nsstring_to_string(value) else {
      return Vec::new();
    };
    value
      .split(',')
      .map(|s| s.trim().to_string())
      .filter(|s| !s.is_empty())
      .collect()
  }
}

pub fn save_removed_words(words: &[String]) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(REMOVED_WORDS_KEY);
    let joined = words.join(",");
    let value = NSString::alloc(nil).init_str(&joined);
    let _: () = msg_send![defaults, setObject: value forKey: key];
  }
}

unsafe fn nsstring_to_string(value: id) -> Option<String> {
  if value == nil {
    return None;
  }

  let ptr: *const c_char = unsafe { msg_send![value, UTF8String] };
  if ptr.is_null() {
    return None;
  }

  Some(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}
