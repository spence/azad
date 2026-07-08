use std::ffi::CStr;
use std::os::raw::c_char;

use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};

use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod, StartupListenMode};

const PREFERRED_DEVICE_KEY: &str = "AzadPreferredInputDeviceId";
const ALWAYS_LISTENING_KEY: &str = "AzadAlwaysListeningEnabled";
const STARTUP_LISTEN_MODE_KEY: &str = "AzadStartupListenMode";
const DEBUG_STATS_ENABLED_KEY: &str = "AzadDebugStatsEnabled";
const ACTIVATION_LEVEL_KEY: &str = "AzadActivationLevel";
const RUN_ON_STARTUP_KEY: &str = "AzadRunOnStartup";
const PASTE_METHOD_KEY: &str = "AzadPasteMethod";
const AUTO_SUBMIT_MODE_KEY: &str = "AzadAutoSubmit";
const OVERLAY_POSITION_KEY: &str = "AzadOverlayPosition";
const APPEND_TRAILING_SPACE_KEY: &str = "AzadAppendTrailingSpaceOnPaste";
const DEDUPLICATE_WORDS_KEY: &str = "AzadDeduplicateWordsOnPaste";
const CONVERT_NUMBER_WORDS_KEY: &str = "AzadConvertNumberWordsOnPaste";
const CONVERT_SPOKEN_EMOJI_KEY: &str = "AzadConvertSpokenEmojiOnPaste";
const LOWERCASE_EXCEPT_UPPERCASE_WORDS_KEY: &str = "AzadLowercaseExceptUppercaseWordsOnPaste";
const REMOVE_HESITATIONS_KEY: &str = "AzadRemoveHesitationsOnPaste";
const REMOVED_WORDS_HESITATION_MIGRATION_KEY: &str = "AzadRemovedWordsHesitationMigration";
const ACTIVE_MODEL_PACK_KEY: &str = "AzadActiveModelPack";
const REMOVED_WORDS_KEY: &str = "AzadRemovedWords";
const ENABLED_CONNECTORS_KEY: &str = "AzadEnabledConnectors";
const ONBOARDING_COMPLETE_KEY: &str = "AzadOnboardingComplete";
const ACCESSIBILITY_PERMISSION_REQUESTED_KEY: &str = "AzadAccessibilityPermissionRequested";
const HISTORY_ENABLED_KEY: &str = "AzadHistoryEnabled";
const LISTEN_MODIFIERS_KEY: &str = "AzadListenModifiers";

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

pub fn load_startup_listen_mode() -> StartupListenMode {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return StartupListenMode::default();
    }

    let key = NSString::alloc(nil).init_str(STARTUP_LISTEN_MODE_KEY);
    let value: id = msg_send![defaults, stringForKey: key];
    let Some(value) = nsstring_to_string(value) else {
      return StartupListenMode::default();
    };
    StartupListenMode::from_prefs_value(value.trim())
  }
}

pub fn save_startup_listen_mode(mode: StartupListenMode) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(STARTUP_LISTEN_MODE_KEY);
    let value = NSString::alloc(nil).init_str(mode.prefs_value());
    let _: () = msg_send![defaults, setObject: value forKey: key];
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

pub fn load_activation_level() -> i64 {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return 0;
    }

    let key = NSString::alloc(nil).init_str(ACTIVATION_LEVEL_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return 0;
    }

    let value: i64 = msg_send![defaults, integerForKey: key];
    value.clamp(0, 100)
  }
}

pub fn save_activation_level(value: i64) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(ACTIVATION_LEVEL_KEY);
    let value = value.clamp(0, 100);
    let _: () = msg_send![defaults, setInteger: value forKey: key];
  }
}

/// Persisted checkbox state. This value is never used to create a login item
/// outside the welcome/settings checkbox handlers.
pub fn load_run_on_startup_enabled() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return false;
    }

    let key = NSString::alloc(nil).init_str(RUN_ON_STARTUP_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return false;
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

pub fn load_accessibility_permission_requested() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return false;
    }

    let key = NSString::alloc(nil).init_str(ACCESSIBILITY_PERMISSION_REQUESTED_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return false;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_accessibility_permission_requested(requested: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(ACCESSIBILITY_PERMISSION_REQUESTED_KEY);
    let value = if requested { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_history_enabled() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return true;
    }

    let key = NSString::alloc(nil).init_str(HISTORY_ENABLED_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return true;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_history_enabled(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(HISTORY_ENABLED_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

/// Listen-hotkey modifier mask (platform MOD_* bits). `None` when unset, so a
/// returning user with no preference keeps the compiled default (Option).
pub fn load_listen_modifiers() -> Option<u8> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return None;
    }

    let key = NSString::alloc(nil).init_str(LISTEN_MODIFIERS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return None;
    }

    let value: i64 = msg_send![defaults, integerForKey: key];
    Some((value & 0xFF) as u8)
  }
}

pub fn save_listen_modifiers(mask: u8) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(LISTEN_MODIFIERS_KEY);
    let _: () = msg_send![defaults, setInteger: mask as i64 forKey: key];
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

pub fn load_overlay_position() -> OverlayPosition {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return OverlayPosition::default();
    }

    let key = NSString::alloc(nil).init_str(OVERLAY_POSITION_KEY);
    let value: id = msg_send![defaults, stringForKey: key];
    let Some(value) = nsstring_to_string(value) else {
      return OverlayPosition::default();
    };
    OverlayPosition::from_prefs_value(value.trim())
  }
}

pub fn save_overlay_position(pos: OverlayPosition) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(OVERLAY_POSITION_KEY);
    let value = NSString::alloc(nil).init_str(pos.prefs_value());
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

pub fn load_deduplicate_words_on_paste() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return false;
    }

    let key = NSString::alloc(nil).init_str(DEDUPLICATE_WORDS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return false;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_deduplicate_words_on_paste(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(DEDUPLICATE_WORDS_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_convert_number_words_on_paste() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return false;
    }

    let key = NSString::alloc(nil).init_str(CONVERT_NUMBER_WORDS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return false;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_convert_number_words_on_paste(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(CONVERT_NUMBER_WORDS_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_convert_spoken_emoji_on_paste() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return false;
    }

    let key = NSString::alloc(nil).init_str(CONVERT_SPOKEN_EMOJI_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return false;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_convert_spoken_emoji_on_paste(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(CONVERT_SPOKEN_EMOJI_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_lowercase_except_uppercase_words_on_paste() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return false;
    }

    let key = NSString::alloc(nil).init_str(LOWERCASE_EXCEPT_UPPERCASE_WORDS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return false;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_lowercase_except_uppercase_words_on_paste(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(LOWERCASE_EXCEPT_UPPERCASE_WORDS_KEY);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn load_remove_hesitations_on_paste() -> bool {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return true;
    }

    let key = NSString::alloc(nil).init_str(REMOVE_HESITATIONS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return true;
    }

    let value: i8 = msg_send![defaults, boolForKey: key];
    value != 0
  }
}

pub fn save_remove_hesitations_on_paste(enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(REMOVE_HESITATIONS_KEY);
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

pub const BUILT_IN_HESITATIONS: &[&str] =
  &["um", "uhm", "uh", "umm", "uhh", "uhhh", "er", "err", "ah", "ahh", "eh", "hm", "hmm", "mmm"];

fn bool_for_key(key_name: &str) -> Option<bool> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return None;
    }
    let key = NSString::alloc(nil).init_str(key_name);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return None;
    }
    let value: i8 = msg_send![defaults, boolForKey: key];
    Some(value != 0)
  }
}

fn save_bool_for_key(key_name: &str, enabled: bool) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }
    let key = NSString::alloc(nil).init_str(key_name);
    let value = if enabled { YES } else { NO };
    let _: () = msg_send![defaults, setBool: value forKey: key];
  }
}

pub fn is_built_in_hesitation(word: &str) -> bool {
  BUILT_IN_HESITATIONS.iter().any(|h| h.eq_ignore_ascii_case(word.trim()))
}

pub fn load_removed_words() -> Vec<String> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return Vec::new();
    }

    let key = NSString::alloc(nil).init_str(REMOVED_WORDS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return Vec::new();
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

pub fn migrate_hesitations_out_of_removed_words(words: Vec<String>) -> Vec<String> {
  if bool_for_key(REMOVED_WORDS_HESITATION_MIGRATION_KEY).unwrap_or(false) {
    return words;
  }
  let filtered: Vec<String> =
    words.into_iter().filter(|word| !is_built_in_hesitation(word)).collect();
  save_removed_words(&filtered);
  save_bool_for_key(REMOVED_WORDS_HESITATION_MIGRATION_KEY, true);
  filtered
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

/// Returns `None` when the key is absent so bootstrap keeps the built-in default
/// enabled set; `Some(vec![])` means the user explicitly disabled every connector.
pub fn load_enabled_connector_ids() -> Option<Vec<String>> {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return None;
    }

    let key = NSString::alloc(nil).init_str(ENABLED_CONNECTORS_KEY);
    let existing: id = msg_send![defaults, objectForKey: key];
    if existing == nil {
      return None;
    }

    let value: id = msg_send![defaults, stringForKey: key];
    let Some(value) = nsstring_to_string(value) else {
      return Some(Vec::new());
    };
    Some(
      value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect(),
    )
  }
}

pub fn save_enabled_connector_ids(ids: &[String]) {
  unsafe {
    let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
    if defaults == nil {
      return;
    }

    let key = NSString::alloc(nil).init_str(ENABLED_CONNECTORS_KEY);
    let joined = ids.join(",");
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
