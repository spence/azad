use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cocoa::appkit::{
  NSApp, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSImage,
  NSMainMenuWindowLevel, NSMenu, NSMenuItem, NSPasteboard, NSScreen, NSStatusBar, NSStatusItem,
  NSVariableStatusItemLength, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use crate::app::AppEvent;
use crate::gateway::ConvStatus;
use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod};

const KEYCODE_RETURN: u16 = 0x24;
const KEYCODE_DIRECT_INPUT: u16 = 0x00;
const KEYCODE_LEFT_COMMAND: u16 = 0x37;
const KEYCODE_RIGHT_COMMAND: u16 = 0x36;
const KEYCODE_LEFT_SHIFT: u16 = 0x38;
const KEYCODE_RIGHT_SHIFT: u16 = 0x3C;
const KEYCODE_LEFT_OPTION: u16 = 0x3A;
const KEYCODE_RIGHT_OPTION: u16 = 0x3D;
const KEYCODE_LEFT_CONTROL: u16 = 0x3B;
const KEYCODE_RIGHT_CONTROL: u16 = 0x3E;
// Virtual keycodes consumed by the HID event tap (Claim-on-press hotkeys).
const KEYCODE_SPACE: u16 = 0x31;
const KEYCODE_ESCAPE: u16 = 0x35;
const KEYCODE_NUMPAD_ENTER: u16 = 0x4C;
const KEYCODE_ARROW_UP: u16 = 0x7E;
const KEYCODE_ARROW_DOWN: u16 = 0x7D;
const KEYCODE_ARROW_LEFT: u16 = 0x7B;
const KEYCODE_ARROW_RIGHT: u16 = 0x7C;
const PASTE_CHORD_HOLD_MS: u64 = 100;

// Device-specific modifier bits from IOKit's NX_DEVICE*KEYMASK. Real hardware modifier presses
// set both the high-level MaskX bit AND the device-specific bit. macOS Screen Sharing forwards
// events only when the device bit is present — without it, the modifier gets stripped and the
// remote side sees only the bare key.
const NX_DEVICELCTLKEYMASK: u64 = 0x0000_0001;
const NX_DEVICELSHIFTKEYMASK: u64 = 0x0000_0002;
const OVERLAY_WIDTH_MIN: f64 = 300.0;
const OVERLAY_WIDTH_MAX: f64 = 620.0;
const OVERLAY_HEIGHT_MIN: f64 = 60.0;
const OVERLAY_HEIGHT_MAX: f64 = 540.0;
const OVERLAY_STACK_GAP: f64 = 10.0;
const OVERLAY_CARD_RADIUS: f64 = 22.0;
const OVERLAY_BORDER_THICKNESS: f64 = 2.0;
const OVERLAY_BUSY_RING_THICKNESS: f64 = 3.4;
const OVERLAY_PAD_X: f64 = 12.0;
const OVERLAY_PAD_TOP: f64 = 12.0;
const OVERLAY_PAD_BOTTOM: f64 = 8.0;
const OVERLAY_TEXT_FONT_SIZE: f64 = 16.0;
const OVERLAY_TEXT_LINE_HEIGHT: f64 = 20.0;
const OVERLAY_WAVE_BG_HEIGHT: f64 = 84.0;
const OVERLAY_WAVE_BAR_COUNT: usize = 96;
const OVERLAY_WAVE_BAR_MIN_HEIGHT: f64 = 3.0;
const OVERLAY_RAW_BADGE_FONT_SIZE: f64 = 12.0;
const OVERLAY_RAW_BADGE_WIDTH: f64 = 44.0;
const OVERLAY_RAW_BADGE_HEIGHT: f64 = 16.0;
const OVERLAY_RAW_BADGE_RIGHT_INSET: f64 = 14.0;
const OVERLAY_RAW_BADGE_BOTTOM_INSET: f64 = 9.0;
const OVERLAY_HOLD_BADGE_WIDTH: f64 = 46.0;
const OVERLAY_HOLD_BADGE_HEIGHT: f64 = 16.0;
const OVERLAY_BADGE_GAP: f64 = 8.0;
const DEVICE_HEADER_WIDTH: f64 = 280.0;
const DEVICE_HEADER_MIN_WIDTH: f64 = 220.0;
const DEVICE_HEADER_HEIGHT: f64 = 24.0;
const DEVICE_HEADER_TEXT_LEADING: f64 = 14.0;
const DEVICE_HEADER_TRAILING: f64 = 12.0;
const DEVICE_HEADER_CHEVRON_SIZE: f64 = 10.0;
const DEVICE_HEADER_LABEL_TO_CHEVRON_GAP: f64 = 8.0;
const DEVICE_HEADER_EXTRA_TOP_PADDING: f64 = 1.0;
const DEVICE_HEADER_EXTRA_SIDE_MARGIN: f64 = 2.0;
const ALWAYS_LISTENING_ROW_HEIGHT: f64 = 24.0;
const ALWAYS_LISTENING_LABEL_LEADING: f64 = 14.0;
const ALWAYS_LISTENING_LABEL_TO_SWITCH_GAP: f64 = 10.0;
const ALWAYS_LISTENING_SWITCH_WIDTH: f64 = 32.0;
const ALWAYS_LISTENING_SWITCH_HEIGHT: f64 = 18.0;
const ALWAYS_LISTENING_SWITCH_INSET: f64 = 2.0;
const ALWAYS_LISTENING_SWITCH_THUMB_SIZE: f64 =
  ALWAYS_LISTENING_SWITCH_HEIGHT - (ALWAYS_LISTENING_SWITCH_INSET * 2.0);
const DEVICE_MENU_ROW_CHROME_WIDTH: f64 = 46.0;
const DEVICE_MENU_INDENT_LEVEL_WIDTH: f64 = 16.0;
const DEVICE_MENU_CHECKMARK_WIDTH: f64 = 18.0;
const DEVICE_MENU_TEXT_SAFETY_PADDING: f64 = 10.0;
const DEVICE_MENU_SCREEN_EDGE_MARGIN: f64 = 24.0;
const DEVICE_HEADER_MENU_COMPENSATION: f64 = 22.0;
const SETTINGS_WINDOW_WIDTH: f64 = 820.0;
const SETTINGS_WINDOW_HEIGHT: f64 = 460.0;
const SETTINGS_INSET_X: f64 = 20.0;
const SETTINGS_TOP_MARGIN: f64 = 18.0;
const SETTINGS_CONTROL_HEIGHT: f64 = 24.0;
const SETTINGS_REFRESH_WIDTH: f64 = 90.0;
const SETTINGS_METRICS_TOP_GAP: f64 = 14.0;
const SETTINGS_SIDEBAR_WIDTH: f64 = 154.0;
const SETTINGS_SIDEBAR_ROW_HEIGHT: f64 = 30.0;
const SETTINGS_SIDEBAR_TO_CONTENT_GAP: f64 = 12.0;
const ONBOARDING_WINDOW_WIDTH: f64 = 640.0;
const ONBOARDING_WINDOW_HEIGHT: f64 = 640.0;
const ONBOARDING_PAD_X: f64 = 40.0;
const ONBOARDING_LABEL_WIDTH: f64 = 170.0;
const ONBOARDING_CONTROL_X: f64 = ONBOARDING_PAD_X + ONBOARDING_LABEL_WIDTH + 12.0;
const ONBOARDING_CONTROL_WIDTH: f64 =
  ONBOARDING_WINDOW_WIDTH - ONBOARDING_CONTROL_X - ONBOARDING_PAD_X;
const ONBOARDING_ROW_HEIGHT: f64 = 26.0;
const SETTINGS_LABEL_WIDTH: f64 = 180.0;
const SETTINGS_POPUP_WIDTH: f64 = 220.0;
const SETTINGS_CONTROL_VERTICAL_GAP: f64 = 14.0;
const LISTEN_NOTICE_CARD_ALPHA: f64 = 0.92;
const LISTEN_NOTICE_WAVE_BASE_ALPHA: f64 = 0.060;
const LISTEN_NOTICE_WAVE_PEAK_ALPHA: f64 = 0.170;
const OVERLAY_NOTICE_KEYCAP_HEIGHT: f64 = 22.0;
const OVERLAY_NOTICE_KEYCAP_OPTION_WIDTH: f64 = 30.0;
const OVERLAY_NOTICE_KEYCAP_SPACE_WIDTH: f64 = 82.0;
const OVERLAY_NOTICE_KEYCAP_PLUS_WIDTH: f64 = 16.0;
const OVERLAY_NOTICE_KEYCAP_FONT_SIZE: f64 = 13.0;
const OVERLAY_NOTICE_AUTO_ON_CHIP_WIDTH: f64 = 96.0;
const OVERLAY_NOTICE_AUTO_ON_CHIP_HEIGHT: f64 = 22.0;
const OVERLAY_NOTICE_AUTO_ON_FONT_SIZE: f64 = 11.5;

// Connector tag chip: a small rounded pill rendered top-left inside the overlay
// card, above the live transcription. `RESERVE` (height + gap) is folded into the
// card's height math so the body text drops below the chip without overlap.
const OVERLAY_CONNECTOR_CHIP_HEIGHT: f64 = 20.0;
const OVERLAY_CONNECTOR_CHIP_GAP: f64 = 6.0;
const OVERLAY_CONNECTOR_CHIP_PAD_X: f64 = 9.0;
const OVERLAY_CONNECTOR_CHIP_FONT_SIZE: f64 = 11.5;
const OVERLAY_CONNECTOR_CHIP_RADIUS: f64 = 9.0;
const OVERLAY_CONNECTOR_CHIP_ICON_SIZE: f64 = 13.0;
const OVERLAY_CONNECTOR_CHIP_ICON_GAP: f64 = 5.0;
// Claude brand color (#D97757) — used for the connector chip fill and the reply text.
const CLAUDE_BRAND_R: f64 = 0.851;
const CLAUDE_BRAND_G: f64 = 0.467;
const CLAUDE_BRAND_B: f64 = 0.341;

// Conversation-mode layout. The query is capped to a few lines (head-kept); the divider
// is a thin rule with gaps above/below; the reply consumes the remaining space up to the
// card max, keeping the streaming tail visible.
const OVERLAY_CONV_DIVIDER_THICKNESS: f64 = 1.0;
const OVERLAY_CONV_DIVIDER_ALPHA: f64 = 0.15;
const OVERLAY_CONV_DIVIDER_GAP: f64 = 8.0;
const OVERLAY_CONV_QUERY_MAX_HEIGHT: f64 = OVERLAY_TEXT_LINE_HEIGHT * 3.0;
const OVERLAY_CONV_STATUS_FONT_SIZE: f64 = 14.0;
// A thin voice-activity strip pinned at the bottom in conversation mode, so the user can
// see the mic is live and hears their follow-up even while the prior reply is on screen.
const OVERLAY_CONV_WAVE_HEIGHT: f64 = 18.0;
const OVERLAY_CONV_WAVE_GAP: f64 = 6.0;
// Warning amber for the error line (distinct from the terracotta reply).
const OVERLAY_CONV_ERROR_R: f64 = 1.0;
const OVERLAY_CONV_ERROR_G: f64 = 0.58;
const OVERLAY_CONV_ERROR_B: f64 = 0.22;

// NSAutoresizingMaskOptions (see AppKit NSView.h)
const NS_VIEW_MIN_X_MARGIN: u64 = 1 << 0;
const NS_VIEW_WIDTH_SIZABLE: u64 = 1 << 1;
const NS_VIEW_HEIGHT_SIZABLE: u64 = 1 << 4;
const NS_VIEW_MAX_Y_MARGIN: u64 = 1 << 5;
const NSEVENT_MODIFIER_FLAG_OPTION: u64 = 1 << 19;
const HOLD_HOTKEY_KEY: Code = Code::Space;

// The listen hotkey is always Space; only the modifier combination is
// user-configurable (>=1 required, default Option). Our own 4-bit mask so it
// serializes cleanly and is independent of CGEventFlags / global_hotkey.
pub const MOD_SHIFT: u8 = 1;
pub const MOD_CONTROL: u8 = 2;
pub const MOD_OPTION: u8 = 4;
pub const MOD_COMMAND: u8 = 8;
// Read on the azad-hotkey-tap thread (Acquire); written from the main thread
// (Release). One byte = no torn read. Default Option == today's behavior.
static LISTEN_MODIFIERS: AtomicU8 = AtomicU8::new(MOD_OPTION);

// Which display the overlay targets. Stored as `OverlayPosition::ui_index()`;
// read on the main thread inside the positioner, written from AppController.
// Default 0 == FollowCursor == today's hardcoded behavior.
static OVERLAY_POSITION: AtomicU8 = AtomicU8::new(0);

static DELEGATE_CLASS: OnceLock<&'static Class> = OnceLock::new();
static OVERLAY_WINDOW_CLASS: OnceLock<&'static Class> = OnceLock::new();
static DEVICE_HEADER_VIEW_CLASS: OnceLock<&'static Class> = OnceLock::new();
static SEARCH_FIELD_DELEGATE_CLASS: OnceLock<&'static Class> = OnceLock::new();
// Carbon-fallback id for the listen hotkey. Mutable (AtomicU32, 0 = unset) so
// the modifier combination can be re-registered live. Tap path is primary; this
// only fires when Accessibility is denied.
static HOTKEY_LISTEN_ID: AtomicU32 = AtomicU32::new(0);
static HOTKEY_ESCAPE_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ENTER_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ENTER_OPTION_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_NUMPAD_ENTER_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_NUMPAD_ENTER_OPTION_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ARROW_UP_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ARROW_DOWN_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ARROW_LEFT_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ARROW_RIGHT_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ESCAPE_REGISTERED: AtomicBool = AtomicBool::new(false);
static HOTKEY_ENTER_REGISTERED: AtomicBool = AtomicBool::new(false);
static HOTKEY_ARROWS_REGISTERED: AtomicBool = AtomicBool::new(false);
// Toggled separately from `HOTKEY_ARROWS_REGISTERED` because Left only dismisses
// while history-browse mode is active, not whenever the overlay is visible.
static HOTKEY_ARROW_LEFT_REGISTERED: AtomicBool = AtomicBool::new(false);
// Right is registered while history-browse mode is active (to expand the
// selected entry). Tracked separately for the same reason as Left.
static HOTKEY_ARROW_RIGHT_REGISTERED: AtomicBool = AtomicBool::new(false);
// Mirrors the app's `debug_stats_enabled` so platform-side renderers can
// emit `OVERLAY_*` log lines under the same gate as the engine's `TOON_*`
// logs. Set via `set_overlay_debug_logs_enabled` from app.rs whenever the
// debug-stats setting toggles.
static OVERLAY_DEBUG_LOGS_ENABLED: AtomicBool = AtomicBool::new(false);
static OPENED_ACCESSIBILITY_SETTINGS: AtomicBool = AtomicBool::new(false);
// Tracks the last sampled `pressedMouseButtons` bitmask between `on_tick`
// polls. We dispatch a click-outside-overlay signal when a button transitions
// from up→down (i.e., a bit goes 0→1) and the cursor is outside the overlay
// frame. Reset on history-mode entry so the very first tick doesn't false-fire
// from a pre-existing held button.
static MOUSE_BUTTON_PREV_STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// Whether the overlay panel may become the key window. Off by default so
// the panel never steals focus from the user's foreground app. Flipped on
// while history mode is active (so the search field can receive typed
// characters) and back off on exit. See `set_overlay_key_input_enabled`.
static OVERLAY_ACCEPTS_KEY_INPUT: AtomicBool = AtomicBool::new(false);

// Event-tap state. `EVENT_TAP_PORT` holds the CFMachPortRef so the callback can re-enable the
// tap after macOS times it out. `SPACE_HOLD_CLAIMED` tracks whether we consumed a keydown for
// Option+Space so we know to also consume (and dispatch Released for) the matching keyup —
// macOS can deliver the Space keyup with Option already released, so we can't re-check flags.
static EVENT_TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static SPACE_HOLD_CLAIMED: AtomicBool = AtomicBool::new(false);

// Tag value stamped onto every synthetic CGEvent Azad posts (via `send_key_chord`). The tap
// callback checks this field and passes through any event that matches, so our own Cmd+V,
// Enter, Ctrl+Enter, etc. don't recursively retrigger hotkey dispatch.
const AZAD_SYNTHETIC_MARKER: i64 = 0x1A2A_D1A2;

// IOKit / CoreGraphics tap constants — values straight from <CoreGraphics/CGEventTypes.h>.
const KCG_HID_EVENT_TAP: u32 = 0;
const KCG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const KCG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const KCG_EVENT_KEY_DOWN: u32 = 10;
const KCG_EVENT_KEY_UP: u32 = 11;
const KCG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const KCG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const KCG_KEYBOARD_EVENT_KEYCODE_FIELD: u32 = 9;
const KCG_KEYBOARD_EVENT_AUTOREPEAT_FIELD: u32 = 8;
const KCG_EVENT_SOURCE_USER_DATA_FIELD: u32 = 42;

thread_local! {
    static OVERLAY_REFS: RefCell<Option<OverlayRefs>> = const { RefCell::new(None) };
    static OVERLAY_TOP_REFS: RefCell<Option<OverlayRefs>> = const { RefCell::new(None) };
    // Cached connector chip icon, keyed by asset name. Loaded as a template image
    // (tinted at render time) once per name to keep file I/O off the streaming path.
    static CONNECTOR_ICON_CACHE: RefCell<Option<(String, id)>> = const { RefCell::new(None) };
    static STATUS_MENU_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static STATUS_DELEGATE_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static SEARCH_FIELD_DELEGATE_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static ALWAYS_LISTENING_TRACK_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static ALWAYS_LISTENING_THUMB_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_VIEW_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_BUTTON_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_HIGHLIGHT_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_LABEL_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_CHEVRON_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_OPEN_MAX_WIDTH: RefCell<Option<f64>> = const { RefCell::new(None) };
    static DEVICE_MENU_OPEN_MAX_WIDTH: RefCell<Option<f64>> = const { RefCell::new(None) };
    static DEVICE_ROW_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static DEVICE_MENU_MODEL: RefCell<DeviceMenuModel> = RefCell::new(DeviceMenuModel::default());
    static HOTKEY_MANAGER_REF: RefCell<Option<GlobalHotKeyManager>> = const { RefCell::new(None) };
    static SETTINGS_WINDOW_REFS: RefCell<Option<SettingsWindowRefs>> = const { RefCell::new(None) };
    static ONBOARDING_WINDOW_REFS: RefCell<Option<OnboardingWindowRefs>> = const { RefCell::new(None) };
    static SETTINGS_LAST_MODEL: RefCell<Option<SettingsViewModel>> = const { RefCell::new(None) };
    // Short-TTL cache of the active-window display frame so ActiveWindow mode
    // doesn't do synchronous Accessibility IPC on every streaming reposition.
    static ACTIVE_WINDOW_SCREEN_CACHE: RefCell<Option<(NSRect, Instant)>> =
      const { RefCell::new(None) };
}

// How long an `ax_focused_window_screen_frame` result stays fresh before
// ActiveWindow mode re-queries Accessibility.
const ACTIVE_WINDOW_SCREEN_CACHE_TTL: Duration = Duration::from_millis(300);

#[derive(Clone, Copy)]
struct OverlayRefs {
  window: id,
  card_view: id,
  label: id,
  hold_badge: id,
  raw_badge: id,
  connector_chip: id,
  connector_chip_label: id,
  connector_chip_icon: id,
  // Conversation-mode views (gateway "hey claude" replies). Stacked below the pinned chip:
  // the user's query, a divider, the streaming reply, and a status/error line. Mutually
  // exclusive with the speech `label`/`meter_view` — each renderer hides the other's.
  conv_query_label: id,
  conv_divider: id,
  // The reply lives in a scrollable NSTextView so long answers can be scrolled rather
  // than tail-truncated. `conv_reply_scroll` is the NSScrollView container (what we
  // show/hide/frame); `conv_reply_text` is its NSTextView documentView.
  conv_reply_scroll: id,
  conv_reply_text: id,
  conv_status_label: id,
  meter_view: id,
  wave_bars: [id; OVERLAY_WAVE_BAR_COUNT],
  busy_gradient_layer: id,
  busy_mask_layer: id,
  notice_accessory_row: id,
  notice_option_key: id,
  notice_option_label: id,
  notice_plus_label: id,
  notice_space_key: id,
  notice_space_label: id,
  notice_auto_on_chip: id,
  notice_auto_on_label: id,
  autocomplete_separator: id,
  autocomplete_labels: [id; AUTOCOMPLETE_MAX_ITEMS],
  autocomplete_bgs: [id; AUTOCOMPLETE_MAX_ITEMS],
  /// Tiny "▶" labels parallel to `autocomplete_labels`. Shown only on rows
  /// whose body text was truncated; tells the user that row is expandable
  /// via right-arrow.
  autocomplete_expand_markers: [id; AUTOCOMPLETE_MAX_ITEMS],
  /// Time-ago labels ("5s", "12m", "1h", "2d") parallel to
  /// `autocomplete_labels`. Drawn outside the highlight bg, on the left
  /// side of every row, so the user can quickly date each entry without
  /// the label competing visually with the body text.
  autocomplete_ts_labels: [id; AUTOCOMPLETE_MAX_ITEMS],
  /// Char-count labels ("47", "1.2k", "12k") parallel to
  /// `autocomplete_labels`. Stacked ABOVE the time-ago label inside the
  /// highlight bg, top-anchored. Same right-aligned 26pt column as the
  /// timestamp; body wrap budget unchanged. Hidden on selected+expanded
  /// rows (matches the time-ago hide rule) and on 1-line rows where
  /// only char-count fits in the right meta column.
  autocomplete_char_count_labels: [id; AUTOCOMPLETE_MAX_ITEMS],
  search_field: id,
  search_icon: id,
  /// 1-pt-wide blinking caret rendered manually because the panel's native
  /// NSTextField caret never appears (likely the non-activating-panel +
  /// accessory-policy combo confuses AppKit's responder-chain blink). A
  /// CALayer-backed NSView with a white fill and an "opacity" blink
  /// animation; positioned per keystroke at the right edge of the typed
  /// text by `layout_history_search_caret`.
  search_caret: id,
}

const AUTOCOMPLETE_ROW_HEIGHT: f64 = 22.0;
const AUTOCOMPLETE_SEPARATOR_HEIGHT: f64 = 1.0;
// Max preallocated row slots in the history overlay. Sized for the worst case
// where every visible entry is single-line (~30 pt tall + 2 pt gap), so a
// 248 pt content budget can accommodate up to ~7 rows. We pre-allocate 9 to
// have headroom if the body line height ever shrinks. The actual visible
// count is determined dynamically by greedy-fitting rows into the card's
// vertical budget — see `render_overlay_history_list`.
pub const AUTOCOMPLETE_MAX_ITEMS: usize = 9;
const AUTOCOMPLETE_TEXT_ALPHA: f64 = 0.45;
const AUTOCOMPLETE_FOCUSED_BG_ALPHA: f64 = 0.08;

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct SettingsWindowRefs {
  window: id,
  tab_list_view: id,
  general_container: id,
  models_container: id,
  permissions_container: id,
  perm_accessibility_status: id,
  perm_microphone_status: id,
  debug_container: id,
  connectors_container: id,
  connectors_checkboxes_view: id,
  run_on_startup_checkbox: id,
  paste_method_popup: id,
  auto_submit_popup: id,
  overlay_position_popup: id,
  append_trailing_space_checkbox: id,
  removed_words_tags_view: id,
  removed_words_input: id,
  removed_words_add_button: id,
  debug_checkbox: id,
  metrics_text_view: id,
  models_status_label: id,
  models_progress_indicator: id,
  models_download_button: id,
  models_cancel_button: id,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct OnboardingWindowRefs {
  window: id,
  get_started_button: id,
  model_status_label: id,
  download_button: id,
  trigger_popup: id,
  listen_mod_shift: id,
  listen_mod_control: id,
  listen_mod_option: id,
  listen_mod_command: id,
  history_checkbox: id,
  insert_popup: id,
  append_trailing_space_checkbox: id,
  overlay_position_popup: id,
  login_checkbox: id,
  device_popup: id,
  perm_accessibility_status: id,
  perm_microphone_status: id,
}

/// State pushed to the first-run onboarding window so its controls reflect the
/// current preferences. Fields are added as sections land.
#[derive(Debug, Clone, Default)]
pub struct OnboardingViewModel {
  pub always_listening_enabled: bool,
  pub history_enabled: bool,
  pub paste_method: PasteMethod,
  pub append_trailing_space_on_paste: bool,
  pub overlay_position: OverlayPosition,
  pub run_on_startup_enabled: bool,
  pub accessibility_status: PermissionStatus,
  pub microphone_status: PermissionStatus,
  pub model_status_text: String,
  /// Download button enabled only when the model is absent (not ready, not
  /// already downloading).
  pub download_enabled: bool,
  /// "Get started" is enabled only once Download has been clicked (or the model
  /// is already present) AND Microphone + Accessibility are granted.
  pub get_started_enabled: bool,
  /// Input devices as (id, display name); the picker is populated from these.
  pub devices: Vec<(String, String)>,
  pub selected_device_index: Option<usize>,
  /// Listen-hotkey modifier mask (platform MOD_* bits); drives the checkboxes.
  pub listen_modifiers: u8,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceMenuModel {
  pub always_listening_enabled: bool,
  pub header_label: String,
  pub expanded: bool,
  pub rows: Vec<DeviceMenuRow>,
}

#[derive(Debug, Clone)]
pub struct DeviceMenuRow {
  pub id: String,
  pub label: String,
  pub checked: bool,
}

#[derive(Debug, Clone)]
pub struct SettingsViewModel {
  pub selected_tab: SettingsTab,
  pub accessibility_status: PermissionStatus,
  pub microphone_status: PermissionStatus,
  pub run_on_startup_enabled: bool,
  pub paste_method: PasteMethod,
  pub auto_submit_mode: AutoSubmitMode,
  pub overlay_position: OverlayPosition,
  pub append_trailing_space_on_paste: bool,
  pub debug_stats_enabled: bool,
  pub metrics_text: String,
  pub model_pack_size_label: String,
  pub model_pack_status: crate::models::PackStatus,
  pub model_download_bytes_done: u64,
  pub model_download_bytes_total: u64,
  pub removed_words: Vec<String>,
  pub connectors: Vec<ConnectorRowVM>,
}

/// One row in the Settings → Connectors tab. The toggle handler keys off the row
/// index (matching `AppController::connectors` order), so the row only carries
/// what it renders.
#[derive(Debug, Clone)]
pub struct ConnectorRowVM {
  pub display_name: String,
  pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
  #[default]
  General,
  Models,
  Permissions,
  Debug,
  Connectors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteResult {
  Pasted,
  EmptyText,
  ClipboardWriteFailed,
  InputEventFailed,
  AccessibilityRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayNoticeSegment {
  Text(String),
  #[allow(dead_code)]
  Keycap(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum OverlayNoticeStyle {
  Standard,
  ListenToggle { enabled: bool, progress: f32 },
}

pub fn run_app() {
  unsafe {
    let _pool = NSAutoreleasePool::new(nil);
    let app = NSApp();
    app.setActivationPolicy_(NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory);

    let delegate_class = register_delegate_class();
    let delegate: id = msg_send![delegate_class, new];

    STATUS_DELEGATE_REF.with(|r| {
      r.borrow_mut().replace(delegate);
    });

    setup_status_bar(delegate);
    // Seed the configured listen-hotkey modifiers before installing the hotkeys,
    // so the Carbon fallback registers the right chord and the tap reads it from
    // the first keystroke. Absent pref keeps the compiled default (Option).
    if let Some(mask) = crate::preferred_store::load_listen_modifiers() {
      if mask != 0 {
        LISTEN_MODIFIERS.store(mask, Ordering::Release);
      }
    }
    install_global_hotkeys();
    install_hotkey_event_tap();

    let _: () = msg_send![app, setDelegate: delegate];
    app.run();
  }
}

pub fn check_required_permissions_on_startup() {
  let _ = ensure_accessibility_for_auto_paste();
}

pub fn ensure_accessibility_for_auto_paste() -> bool {
  if is_accessibility_trusted() {
    return true;
  }
  maybe_request_accessibility_permission_once();
  eprintln!(
    "Azad: Accessibility permission missing. Enable Azad in System Settings -> Privacy & Security -> Accessibility."
  );
  false
}

pub fn set_device_menu(model: DeviceMenuModel) {
  DEVICE_MENU_MODEL.with(|slot| {
    slot.borrow_mut().clone_from(&model);
  });
  rebuild_status_menu();
}

pub fn show_settings_window(model: SettingsViewModel) {
  unsafe {
    let refs = ensure_settings_window();
    let tab = model.selected_tab;
    apply_settings_view_model(refs, &model);
    apply_settings_selected_tab(refs, tab);
    let app = NSApp();
    // Become a regular app so macOS grants activation and shows the window
    // in front. Reverted to Accessory when the settings window closes.
    app.setActivationPolicy_(NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular);
    let _: () = msg_send![app, activateIgnoringOtherApps: YES];
    let _: () = msg_send![refs.window, makeKeyAndOrderFront: nil];
  }
}

pub fn show_onboarding_window(model: OnboardingViewModel) {
  unsafe {
    let refs = ensure_onboarding_window();
    apply_onboarding_view_model(refs, &model);
    let app = NSApp();
    app.setActivationPolicy_(NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular);
    let _: () = msg_send![app, activateIgnoringOtherApps: YES];
    let _: () = msg_send![refs.window, makeKeyAndOrderFront: nil];
  }
}

unsafe fn apply_onboarding_view_model(refs: OnboardingWindowRefs, model: &OnboardingViewModel) {
  let trigger_index: i64 = if model.always_listening_enabled { 0 } else { 1 };
  let _: () = msg_send![refs.trigger_popup, selectItemAtIndex: trigger_index];
  let history_state: i64 = if model.history_enabled { 1 } else { 0 };
  let _: () = msg_send![refs.history_checkbox, setState: history_state];
  let _: () = msg_send![refs.insert_popup, selectItemAtIndex: model.paste_method.ui_index()];
  let trailing_space_state: i64 = if model.append_trailing_space_on_paste { 1 } else { 0 };
  let _: () = msg_send![refs.append_trailing_space_checkbox, setState: trailing_space_state];
  let _: () =
    msg_send![refs.overlay_position_popup, selectItemAtIndex: model.overlay_position.ui_index()];
  let login_state: i64 = if model.run_on_startup_enabled { 1 } else { 0 };
  let _: () = msg_send![refs.login_checkbox, setState: login_state];
  let _: () = msg_send![refs.device_popup, removeAllItems];
  for (_id, name) in &model.devices {
    let _: () = msg_send![refs.device_popup, addItemWithTitle: NSString::alloc(nil).init_str(name)];
  }
  if let Some(idx) = model.selected_device_index {
    let _: () = msg_send![refs.device_popup, selectItemAtIndex: idx as i64];
  }
  sync_listen_mod_checkboxes(refs, model.listen_modifiers);
  apply_onboarding_dynamic(refs, model);
}

unsafe fn sync_listen_mod_checkboxes(refs: OnboardingWindowRefs, mask: u8) {
  let on = |bit: u8| -> i64 { if mask & bit != 0 { 1 } else { 0 } };
  let _: () = msg_send![refs.listen_mod_shift, setState: on(MOD_SHIFT)];
  let _: () = msg_send![refs.listen_mod_control, setState: on(MOD_CONTROL)];
  let _: () = msg_send![refs.listen_mod_option, setState: on(MOD_OPTION)];
  let _: () = msg_send![refs.listen_mod_command, setState: on(MOD_COMMAND)];
}

/// Re-sync the listen-modifier checkboxes to `mask` — used after a toggle so a
/// rejected change (e.g. unchecking the last modifier) snaps back visually.
pub fn sync_onboarding_listen_modifiers(mask: u8) {
  if let Some(refs) = current_onboarding_window() {
    unsafe {
      sync_listen_mod_checkboxes(refs, mask);
    }
  }
}

pub fn close_onboarding_window() {
  let refs = ONBOARDING_WINDOW_REFS.with(|store| store.borrow_mut().take());
  if let Some(refs) = refs {
    unsafe {
      let _: () = msg_send![refs.window, orderOut: nil];
      let app = NSApp();
      app.setActivationPolicy_(
        NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
      );
    }
  }
}

unsafe fn ensure_onboarding_window() -> OnboardingWindowRefs {
  if let Some(existing) = ONBOARDING_WINDOW_REFS.with(|store| *store.borrow()) {
    return existing;
  }
  let refs = create_onboarding_window();
  ONBOARDING_WINDOW_REFS.with(|store| {
    store.borrow_mut().replace(refs);
  });
  refs
}

fn current_onboarding_window() -> Option<OnboardingWindowRefs> {
  ONBOARDING_WINDOW_REFS.with(|store| *store.borrow())
}

/// Refresh the dynamic parts of the welcome window (model status, the
/// "Get started" gate, and permission indicators) while it's open, so they
/// update live as the download progresses and the user grants access. Leaves
/// the user-controlled popups and checkboxes untouched.
pub fn update_onboarding_window(model: OnboardingViewModel) {
  if let Some(refs) = current_onboarding_window() {
    unsafe {
      apply_onboarding_dynamic(refs, &model);
    }
  }
}

unsafe fn apply_onboarding_dynamic(refs: OnboardingWindowRefs, model: &OnboardingViewModel) {
  let status = NSString::alloc(nil).init_str(&model.model_status_text);
  let _: () = msg_send![refs.model_status_label, setStringValue: status];
  let _: () =
    msg_send![refs.download_button, setEnabled: if model.download_enabled { YES } else { NO }];
  let _: () = msg_send![refs.get_started_button, setEnabled: if model.get_started_enabled { YES } else { NO }];
  set_permission_status_label(refs.perm_accessibility_status, model.accessibility_status);
  set_permission_status_label(refs.perm_microphone_status, model.microphone_status);
}

unsafe fn set_permission_status_label(label: id, status: PermissionStatus) {
  let (text, r, g, b) = match status {
    PermissionStatus::Granted => ("● Granted", 0.20, 0.65, 0.30),
    _ => ("○ Not granted", 0.85, 0.45, 0.10),
  };
  let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str(text)];
  let color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, r, g, b, 1.0);
  let _: () = msg_send![label, setTextColor: color];
}

unsafe fn make_onboarding_label(text: &str, frame: NSRect, font_size: f64, bold: bool) -> id {
  let label: id = msg_send![class!(NSTextField), alloc];
  let label: id = msg_send![label, initWithFrame: frame];
  let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str(text)];
  let _: () = msg_send![label, setBezeled: NO];
  let _: () = msg_send![label, setDrawsBackground: NO];
  let _: () = msg_send![label, setEditable: NO];
  let _: () = msg_send![label, setSelectable: NO];
  let font: id = if bold {
    msg_send![class!(NSFont), boldSystemFontOfSize: font_size]
  } else {
    msg_send![class!(NSFont), systemFontOfSize: font_size]
  };
  let _: () = msg_send![label, setFont: font];
  label
}

unsafe fn make_onboarding_row_label(text: &str, y: f64) -> id {
  let frame = NSRect::new(
    NSPoint::new(ONBOARDING_PAD_X, y),
    NSSize::new(ONBOARDING_LABEL_WIDTH, ONBOARDING_ROW_HEIGHT),
  );
  make_onboarding_label(text, frame, 13.0, false)
}

unsafe fn make_onboarding_popup(items: &[&str], y: f64, action: Sel) -> id {
  let frame = NSRect::new(
    NSPoint::new(ONBOARDING_CONTROL_X, y - 2.0),
    NSSize::new(ONBOARDING_CONTROL_WIDTH, ONBOARDING_ROW_HEIGHT + 4.0),
  );
  let popup: id = msg_send![class!(NSPopUpButton), alloc];
  let popup: id = msg_send![popup, initWithFrame: frame pullsDown: NO];
  for item in items {
    let _: () = msg_send![popup, addItemWithTitle: NSString::alloc(nil).init_str(item)];
  }
  let _: () = msg_send![popup, setAction: action];
  popup
}

unsafe fn make_onboarding_checkbox(title: &str, y: f64, action: Sel) -> id {
  let frame = NSRect::new(
    NSPoint::new(ONBOARDING_CONTROL_X, y),
    NSSize::new(ONBOARDING_CONTROL_WIDTH, ONBOARDING_ROW_HEIGHT),
  );
  let checkbox: id = msg_send![class!(NSButton), alloc];
  let checkbox: id = msg_send![checkbox, initWithFrame: frame];
  let _: () = msg_send![checkbox, setButtonType: 3usize];
  let _: () = msg_send![checkbox, setTitle: NSString::alloc(nil).init_str(title)];
  let _: () = msg_send![checkbox, setAction: action];
  checkbox
}

/// A small modifier checkbox for the listen-shortcut row, tagged with its MOD_*
/// bit so one handler routes all four. Target is set with the other controls.
unsafe fn make_onboarding_mod_checkbox(
  content_view: id,
  title: &str,
  x: f64,
  y: f64,
  tag: i64,
) -> id {
  let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(52.0, ONBOARDING_ROW_HEIGHT));
  let cb: id = msg_send![class!(NSButton), alloc];
  let cb: id = msg_send![cb, initWithFrame: frame];
  let _: () = msg_send![cb, setButtonType: 3usize];
  let _: () = msg_send![cb, setTitle: NSString::alloc(nil).init_str(title)];
  let _: () = msg_send![cb, setTag: tag];
  let _: () = msg_send![cb, setAction: sel!(onboardingToggleListenModifier:)];
  let _: () = msg_send![content_view, addSubview: cb];
  cb
}

/// A permission row: name on the left, a live status label, and an "Open
/// Settings" button tagged so one handler can route to the right pane. Returns
/// the status-label id so the caller can refresh it live.
unsafe fn make_onboarding_permission_row(
  content_view: id,
  delegate: id,
  label_text: &str,
  y: f64,
  tag: i64,
) -> id {
  let label_frame =
    NSRect::new(NSPoint::new(ONBOARDING_PAD_X, y), NSSize::new(150.0, ONBOARDING_ROW_HEIGHT));
  let label = make_onboarding_label(label_text, label_frame, 13.0, false);
  let _: () = msg_send![content_view, addSubview: label];

  let status_frame = NSRect::new(
    NSPoint::new(ONBOARDING_PAD_X + 160.0, y),
    NSSize::new(150.0, ONBOARDING_ROW_HEIGHT),
  );
  let status_label = make_onboarding_label("…", status_frame, 13.0, false);
  let _: () = msg_send![content_view, addSubview: status_label];

  let button_w = 110.0;
  let button_frame = NSRect::new(
    NSPoint::new(ONBOARDING_WINDOW_WIDTH - ONBOARDING_PAD_X - button_w, y - 2.0),
    NSSize::new(button_w, ONBOARDING_ROW_HEIGHT + 4.0),
  );
  let button: id = msg_send![class!(NSButton), alloc];
  let button: id = msg_send![button, initWithFrame: button_frame];
  let _: () = msg_send![button, setTitle: NSString::alloc(nil).init_str("Open Settings")];
  let _: () = msg_send![button, setBezelStyle: 1usize];
  let _: () = msg_send![button, setButtonType: 0usize];
  let _: () = msg_send![button, setTag: tag];
  let _: () = msg_send![button, setAction: sel!(onboardingOpenPermission:)];
  if delegate != nil {
    let _: () = msg_send![button, setTarget: delegate];
  }
  let _: () = msg_send![content_view, addSubview: button];

  status_label
}

unsafe fn create_onboarding_window() -> OnboardingWindowRefs {
  let frame = main_screen_frame();
  let x = frame.origin.x + (frame.size.width - ONBOARDING_WINDOW_WIDTH) * 0.5;
  let y = frame.origin.y + (frame.size.height - ONBOARDING_WINDOW_HEIGHT) * 0.5;
  let window_frame =
    NSRect::new(NSPoint::new(x, y), NSSize::new(ONBOARDING_WINDOW_WIDTH, ONBOARDING_WINDOW_HEIGHT));

  // Chromeless: a titled window with its title bar and traffic-light buttons
  // hidden, content drawn full-height (FullSizeContentView = 1 << 15), and
  // movement disabled — so it reads as a custom welcome panel, not a normal
  // window the user can drag, minimize, or close. Setup completes via
  // "Get started", never by dismissing it.
  let style_mask: u64 = NSWindowStyleMask::NSTitledWindowMask.bits() | (1u64 << 15);
  let window: id = msg_send![class!(NSWindow), alloc];
  let window: id = msg_send![window, initWithContentRect: window_frame
                                                styleMask: style_mask
                                                  backing: NSBackingStoreType::NSBackingStoreBuffered
                                                    defer: NO];
  let _: () = msg_send![window, setReleasedWhenClosed: NO];
  let _: () = msg_send![window, setTitleVisibility: 1isize]; // NSWindowTitleHidden
  let _: () = msg_send![window, setTitlebarAppearsTransparent: YES];
  // Chromeless but draggable: no title bar to grab, so let the user move it by
  // dragging anywhere on the background (clicks on controls still work normally).
  let _: () = msg_send![window, setMovableByWindowBackground: YES];
  // Hide close / miniaturize / zoom (NSWindowButton 0 / 1 / 2).
  for button_kind in [0isize, 1isize, 2isize] {
    let btn: id = msg_send![window, standardWindowButton: button_kind];
    if btn != nil {
      let _: () = msg_send![btn, setHidden: YES];
    }
  }

  let content_view: id = msg_send![window, contentView];

  let full_w = ONBOARDING_WINDOW_WIDTH - ONBOARDING_PAD_X * 2.0;
  let heading = make_onboarding_label(
    "Welcome to Azad",
    NSRect::new(
      NSPoint::new(ONBOARDING_PAD_X, ONBOARDING_WINDOW_HEIGHT - 48.0),
      NSSize::new(full_w, 30.0),
    ),
    22.0,
    true,
  );
  let _: () = msg_send![content_view, addSubview: heading];
  let subhead = make_onboarding_label(
    "Let's get you set up — finish below to start dictating.",
    NSRect::new(
      NSPoint::new(ONBOARDING_PAD_X, ONBOARDING_WINDOW_HEIGHT - 72.0),
      NSSize::new(full_w, 18.0),
    ),
    13.0,
    false,
  );
  let _: () = msg_send![content_view, addSubview: subhead];

  // Model section: a two-line status (name · size, then location/progress); the
  // Download button is disabled once the model is present.
  let model_y = ONBOARDING_WINDOW_HEIGHT - 116.0;
  let model_label = make_onboarding_row_label("Model", model_y + 9.0);
  let _: () = msg_send![content_view, addSubview: model_label];
  let model_status_label: id = {
    let frame = NSRect::new(
      NSPoint::new(ONBOARDING_PAD_X + 84.0, model_y - 4.0),
      NSSize::new(full_w - 84.0 - 122.0, 42.0),
    );
    let l: id = msg_send![class!(NSTextField), alloc];
    let l: id = msg_send![l, initWithFrame: frame];
    let _: () = msg_send![l, setStringValue: NSString::alloc(nil).init_str("…")];
    let _: () = msg_send![l, setBezeled: NO];
    let _: () = msg_send![l, setDrawsBackground: NO];
    let _: () = msg_send![l, setEditable: NO];
    let _: () = msg_send![l, setSelectable: NO];
    let _: () = msg_send![l, setUsesSingleLineMode: NO];
    let font: id = msg_send![class!(NSFont), systemFontOfSize: 12.0];
    let _: () = msg_send![l, setFont: font];
    let _: () = msg_send![content_view, addSubview: l];
    l
  };
  let download_button: id = {
    let w = 110.0;
    let frame = NSRect::new(
      NSPoint::new(ONBOARDING_WINDOW_WIDTH - ONBOARDING_PAD_X - w, model_y + 5.0),
      NSSize::new(w, ONBOARDING_ROW_HEIGHT + 4.0),
    );
    let b: id = msg_send![class!(NSButton), alloc];
    let b: id = msg_send![b, initWithFrame: frame];
    let _: () = msg_send![b, setTitle: NSString::alloc(nil).init_str("Download")];
    let _: () = msg_send![b, setBezelStyle: 1usize];
    let _: () = msg_send![b, setButtonType: 0usize];
    let _: () = msg_send![b, setAction: sel!(onboardingDownloadModel:)];
    let _: () = msg_send![content_view, addSubview: b];
    b
  };

  let trigger_y = model_y - 50.0;
  let trigger_label = make_onboarding_row_label("Start listening", trigger_y);
  let _: () = msg_send![content_view, addSubview: trigger_label];
  let trigger_popup = make_onboarding_popup(
    &["Automatically", "Manually (hold shortcut)"],
    trigger_y,
    sel!(onboardingSetTrigger:),
  );
  let _: () = msg_send![content_view, addSubview: trigger_popup];

  // Listen shortcut: Space is fixed; the user picks the modifier combination
  // (>=1 required). History stays built-in (hold + Up), so it isn't shown here.
  let shortcut_y = trigger_y - 40.0;
  let shortcut_label = make_onboarding_row_label("Listen shortcut", shortcut_y);
  let _: () = msg_send![content_view, addSubview: shortcut_label];
  let listen_mod_shift = make_onboarding_mod_checkbox(
    content_view,
    "⇧",
    ONBOARDING_CONTROL_X,
    shortcut_y,
    MOD_SHIFT as i64,
  );
  let listen_mod_control = make_onboarding_mod_checkbox(
    content_view,
    "⌃",
    ONBOARDING_CONTROL_X + 52.0,
    shortcut_y,
    MOD_CONTROL as i64,
  );
  let listen_mod_option = make_onboarding_mod_checkbox(
    content_view,
    "⌥",
    ONBOARDING_CONTROL_X + 104.0,
    shortcut_y,
    MOD_OPTION as i64,
  );
  let listen_mod_command = make_onboarding_mod_checkbox(
    content_view,
    "⌘",
    ONBOARDING_CONTROL_X + 156.0,
    shortcut_y,
    MOD_COMMAND as i64,
  );
  // Vertically center against the checkbox glyphs (the checkboxes center their
  // titles; a full-height top-aligned label sits too high).
  let space_hint_frame = NSRect::new(
    NSPoint::new(ONBOARDING_CONTROL_X + 214.0, shortcut_y + 5.0),
    NSSize::new(80.0, 16.0),
  );
  let space_hint = make_onboarding_label("+ Space", space_hint_frame, 12.0, false);
  let _: () = msg_send![content_view, addSubview: space_hint];

  let history_y = shortcut_y - 40.0;
  let history_label = make_onboarding_row_label("History", history_y);
  let _: () = msg_send![content_view, addSubview: history_label];
  let history_checkbox = make_onboarding_checkbox(
    "Keep a searchable history of dictations",
    history_y,
    sel!(onboardingToggleHistory:),
  );
  let _: () = msg_send![content_view, addSubview: history_checkbox];

  let insert_y = history_y - 40.0;
  let insert_label = make_onboarding_row_label("Insert text by", insert_y);
  let _: () = msg_send![content_view, addSubview: insert_label];
  let insert_popup = make_onboarding_popup(
    &["Paste", "Direct", "Direct + copy to clipboard"],
    insert_y,
    sel!(settingsSelectPasteMethod:),
  );
  let _: () = msg_send![content_view, addSubview: insert_popup];

  let trailing_space_y = insert_y - 40.0;
  let trailing_space_label = make_onboarding_row_label("Trailing space", trailing_space_y);
  let _: () = msg_send![content_view, addSubview: trailing_space_label];
  let append_trailing_space_checkbox = make_onboarding_checkbox(
    "Append a space after each insert",
    trailing_space_y,
    sel!(onboardingToggleAppendTrailingSpace:),
  );
  let _: () = msg_send![content_view, addSubview: append_trailing_space_checkbox];

  let overlay_position_y = trailing_space_y - 40.0;
  let overlay_position_label = make_onboarding_row_label("Overlay position", overlay_position_y);
  let _: () = msg_send![content_view, addSubview: overlay_position_label];
  let overlay_position_popup = make_onboarding_popup(
    &["Follow cursor", "Primary display", "Active window"],
    overlay_position_y,
    sel!(onboardingSetOverlayPosition:),
  );
  let _: () = msg_send![content_view, addSubview: overlay_position_popup];

  let login_y = overlay_position_y - 40.0;
  let login_label = make_onboarding_row_label("Startup", login_y);
  let _: () = msg_send![content_view, addSubview: login_label];
  let login_checkbox = make_onboarding_checkbox(
    "Open Azad automatically at login",
    login_y,
    sel!(onboardingToggleLogin:),
  );
  let _: () = msg_send![content_view, addSubview: login_checkbox];

  let delegate = STATUS_DELEGATE_REF.with(|slot| *slot.borrow()).unwrap_or(nil);

  // Permissions section.
  let perms_header_y = login_y - 42.0;
  let perms_header_frame = NSRect::new(
    NSPoint::new(ONBOARDING_PAD_X, perms_header_y),
    NSSize::new(ONBOARDING_WINDOW_WIDTH - ONBOARDING_PAD_X * 2.0, ONBOARDING_ROW_HEIGHT),
  );
  let perms_header = make_onboarding_label("Permissions", perms_header_frame, 15.0, true);
  let _: () = msg_send![content_view, addSubview: perms_header];

  let perm_accessibility_status = make_onboarding_permission_row(
    content_view,
    delegate,
    "Accessibility",
    perms_header_y - 30.0,
    0,
  );
  let perm_microphone_status =
    make_onboarding_permission_row(content_view, delegate, "Microphone", perms_header_y - 60.0, 1);

  let hint_frame = NSRect::new(
    NSPoint::new(ONBOARDING_PAD_X, perms_header_y - 82.0),
    NSSize::new(ONBOARDING_WINDOW_WIDTH - ONBOARDING_PAD_X * 2.0, 18.0),
  );
  let hint = make_onboarding_label(
    "Microphone and Accessibility are required to use Azad.",
    hint_frame,
    11.0,
    false,
  );
  let _: () = msg_send![content_view, addSubview: hint];

  // Microphone device picker.
  let device_y = perms_header_y - 116.0;
  let device_label = make_onboarding_row_label("Microphone device", device_y);
  let _: () = msg_send![content_view, addSubview: device_label];
  let device_popup = make_onboarding_popup(&[], device_y, sel!(onboardingSelectDevice:));
  let _: () = msg_send![content_view, addSubview: device_popup];

  let button_w = 160.0;
  let button_frame = NSRect::new(
    NSPoint::new((ONBOARDING_WINDOW_WIDTH - button_w) * 0.5, device_y - 44.0),
    NSSize::new(button_w, 34.0),
  );
  let get_started_button: id = msg_send![class!(NSButton), alloc];
  let get_started_button: id = msg_send![get_started_button, initWithFrame: button_frame];
  let _: () = msg_send![get_started_button, setTitle: NSString::alloc(nil).init_str("Get started")];
  let _: () = msg_send![get_started_button, setBezelStyle: 1usize];
  let _: () = msg_send![get_started_button, setButtonType: 0usize];
  let _: () = msg_send![get_started_button, setKeyEquivalent: NSString::alloc(nil).init_str("\r")];
  let _: () = msg_send![get_started_button, setAction: sel!(onboardingGetStarted:)];
  let _: () = msg_send![content_view, addSubview: get_started_button];

  if delegate != nil {
    let _: () = msg_send![get_started_button, setTarget: delegate];
    let _: () = msg_send![download_button, setTarget: delegate];
    let _: () = msg_send![trigger_popup, setTarget: delegate];
    let _: () = msg_send![listen_mod_shift, setTarget: delegate];
    let _: () = msg_send![listen_mod_control, setTarget: delegate];
    let _: () = msg_send![listen_mod_option, setTarget: delegate];
    let _: () = msg_send![listen_mod_command, setTarget: delegate];
    let _: () = msg_send![history_checkbox, setTarget: delegate];
    let _: () = msg_send![insert_popup, setTarget: delegate];
    let _: () = msg_send![append_trailing_space_checkbox, setTarget: delegate];
    let _: () = msg_send![overlay_position_popup, setTarget: delegate];
    let _: () = msg_send![login_checkbox, setTarget: delegate];
    let _: () = msg_send![device_popup, setTarget: delegate];
  }

  OnboardingWindowRefs {
    window,
    get_started_button,
    model_status_label,
    download_button,
    trigger_popup,
    listen_mod_shift,
    listen_mod_control,
    listen_mod_option,
    listen_mod_command,
    history_checkbox,
    insert_popup,
    append_trailing_space_checkbox,
    overlay_position_popup,
    login_checkbox,
    device_popup,
    perm_accessibility_status,
    perm_microphone_status,
  }
}

pub fn update_settings_window(model: SettingsViewModel) {
  if let Some(refs) = current_settings_window() {
    unsafe {
      apply_settings_view_model(refs, &model);
    }
  }
}

pub fn set_launch_agent_startup_enabled(enabled: bool) -> bool {
  let plist_path = match launch_agent_plist_path() {
    Some(path) => path,
    None => {
      eprintln!("Azad: unable to resolve LaunchAgent plist path for startup toggle");
      return false;
    }
  };

  if !plist_path.exists() {
    eprintln!("Azad: LaunchAgent plist not found at {} (run install first)", plist_path.display());
    return false;
  }

  let output = match Command::new("plutil")
    .arg("-replace")
    .arg("RunAtLoad")
    .arg("-bool")
    .arg(if enabled { "true" } else { "false" })
    .arg(plist_path.as_os_str())
    .output()
  {
    Ok(output) => output,
    Err(err) => {
      eprintln!("Azad: failed to update RunAtLoad in LaunchAgent plist: {err}");
      return false;
    }
  };

  if output.status.success() {
    return true;
  }

  let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
  if stderr.is_empty() {
    eprintln!("Azad: failed to update RunAtLoad (status {})", output.status);
  } else {
    eprintln!("Azad: failed to update RunAtLoad: {stderr}");
  }
  false
}

pub fn focus_existing_instance(bundle_id: &str) -> bool {
  unsafe {
    let _pool = NSAutoreleasePool::new(nil);
    let bundle = NSString::alloc(nil).init_str(bundle_id);
    if bundle == nil {
      return false;
    }

    let running_apps: id = msg_send![
        class!(NSRunningApplication),
        runningApplicationsWithBundleIdentifier: bundle
    ];
    if running_apps == nil {
      return false;
    }

    let current_pid = std::process::id() as i32;
    let count: usize = msg_send![running_apps, count];
    for idx in 0..count {
      let running: id = msg_send![running_apps, objectAtIndex: idx];
      if running == nil {
        continue;
      }

      let pid: i32 = msg_send![running, processIdentifier];
      if pid == current_pid {
        continue;
      }

      let activated: bool = msg_send![running, activateWithOptions: (1u64 << 1)];
      if activated {
        return true;
      }
    }
  }

  false
}

fn launch_agent_plist_path() -> Option<PathBuf> {
  let home = std::env::var_os("HOME")?;
  let mut path = PathBuf::from(home);
  path.push("Library");
  path.push("LaunchAgents");
  path.push("ai.azad.plist");
  Some(path)
}

pub fn launch_agent_plist_exists() -> bool {
  launch_agent_plist_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn create_launch_agent_plist_if_missing() {
  let plist_path = match launch_agent_plist_path() {
    Some(p) => p,
    None => return,
  };
  if plist_path.exists() {
    return;
  }

  let exe = match std::env::current_exe() {
    Ok(p) => p,
    Err(e) => {
      eprintln!("Azad: cannot resolve current_exe for LaunchAgent: {e}");
      return;
    }
  };

  let home = match std::env::var_os("HOME") {
    Some(h) => h,
    None => return,
  };
  let log_dir = PathBuf::from(&home).join("Library/Logs/Azad");
  let _ = std::fs::create_dir_all(&log_dir);

  let resources_dir = exe
    .parent()
    .and_then(|p| p.parent())
    .map(|p| p.join("Resources"))
    .unwrap_or_default();

  let plist = format!(
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>ai.azad</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>LimitLoadToSessionType</key>
  <array>
    <string>Aqua</string>
  </array>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>Nice</key>
  <integer>-10</integer>
  <key>EnvironmentVariables</key>
  <dict>
    <key>AZAD_ASSETS_DIR</key>
    <string>{resources}</string>
  </dict>
  <key>StandardOutPath</key>
  <string>{stdout}</string>
  <key>StandardErrorPath</key>
  <string>{stderr}</string>
</dict>
</plist>"#,
    exe = exe.display(),
    resources = resources_dir.display(),
    stdout = log_dir.join("stdout.log").display(),
    stderr = log_dir.join("stderr.log").display(),
  );

  if let Some(parent) = plist_path.parent() {
    let _ = std::fs::create_dir_all(parent);
  }
  if let Err(e) = std::fs::write(&plist_path, plist) {
    eprintln!("Azad: failed to write LaunchAgent plist: {e}");
  }
}

#[track_caller]
pub fn show_overlay() {
  if overlay_debug_logs_enabled() {
    let loc = std::panic::Location::caller();
    eprintln!("AZAD_WIN main=front at {}:{}", loc.file(), loc.line());
  }
  unsafe {
    let refs = ensure_overlay();
    move_overlay_to_target_screen(refs, true);
    let _: () = msg_send![refs.window, orderFrontRegardless];
  }
  set_escape_hotkey_enabled(true);
  set_enter_hotkey_enabled(true);
  set_arrow_hotkeys_enabled(true);
}

#[track_caller]
pub fn show_overlay_top() {
  if overlay_debug_logs_enabled() {
    let loc = std::panic::Location::caller();
    eprintln!("AZAD_WIN top=front at {}:{}", loc.file(), loc.line());
  }
  unsafe {
    let refs = ensure_overlay_top();
    if let Some(bottom) = current_overlay() {
      position_overlay_top_relative_to_bottom(refs, bottom);
    } else {
      move_overlay_to_target_screen(refs, true);
    }
    let _: () = msg_send![refs.window, orderFrontRegardless];
  }
}

#[track_caller]
pub fn hide_overlay() {
  hide_overlay_top();
  if let Some(refs) = current_overlay() {
    if overlay_debug_logs_enabled() {
      let loc = std::panic::Location::caller();
      eprintln!("AZAD_WIN main=out at {}:{}", loc.file(), loc.line());
    }
    unsafe {
      let _: () = msg_send![refs.window, orderOut: nil];
      let app = NSApp();
      let _: () = msg_send![app, updateWindows];
    }
  }
  set_escape_hotkey_enabled(false);
  set_enter_hotkey_enabled(false);
  set_arrow_hotkeys_enabled(false);
}

#[track_caller]
pub fn hide_overlay_top() {
  if let Some(refs) = current_overlay_top() {
    if overlay_debug_logs_enabled() {
      let loc = std::panic::Location::caller();
      eprintln!("AZAD_WIN top=out at {}:{}", loc.file(), loc.line());
    }
    unsafe {
      let _: () = msg_send![refs.window, orderOut: nil];
    }
  }
}

pub fn set_overlay_stream_content(
  draft: &str,
  activity: &[f32],
  busy_phase: Option<f32>,
  show_raw_badge: bool,
  show_hold_badge: bool,
  history_position: &str,
  connector_tag: &str,
  connector_icon: &str,
) {
  let Some(refs) = current_overlay() else {
    return;
  };
  unsafe {
    move_overlay_to_target_screen(refs, false);
    render_overlay_text(
      refs,
      draft,
      activity,
      busy_phase,
      show_raw_badge,
      show_hold_badge,
      connector_tag,
      connector_icon,
    );
    render_overlay_history_position(refs, history_position);
  }
}

/// Render a gateway conversation turn into the overlay: pinned chip, the user's query, a
/// divider, then the streaming reply (or a thinking/error status line), plus a bottom
/// voice-activity strip. Mutually exclusive with `set_overlay_stream_content`.
pub fn set_overlay_conversation_content(
  connector_tag: &str,
  connector_icon: &str,
  user_query: &str,
  reply: &str,
  status: ConvStatus,
  error_msg: &str,
  activity: &[f32],
  busy_phase: Option<f32>,
) {
  let Some(refs) = current_overlay() else {
    return;
  };
  unsafe {
    move_overlay_to_target_screen(refs, false);
    render_overlay_conversation(
      refs,
      connector_tag,
      connector_icon,
      user_query,
      reply,
      status,
      error_msg,
      activity,
      busy_phase,
    );
  }
}

/// Clear and hide the conversation-mode views so a hide/show cycle never flashes stale
/// query/reply text. Safe to call when no overlay exists.
pub fn reset_overlay_conversation_views() {
  let Some(refs) = current_overlay() else {
    return;
  };
  unsafe {
    for v in [refs.conv_query_label, refs.conv_status_label] {
      if v != nil {
        let _: () = msg_send![v, setStringValue: NSString::alloc(nil).init_str("")];
      }
    }
    if refs.conv_reply_text != nil {
      let _: () = msg_send![refs.conv_reply_text, setString: NSString::alloc(nil).init_str("")];
    }
    hide_conversation_views(refs);
  }
}

pub struct HistoryEntryView<'a> {
  pub text: &'a str,
  /// Byte ranges inside `text` that should be highlighted. Populated when
  /// the history list is filtered by a search query — the renderer paints
  /// each range with a translucent yellow background. Empty for
  /// unfiltered/empty-query renders. Multiple ranges support multi-token
  /// highlighting from FTS5.
  pub match_ranges: Vec<(usize, usize)>,
  /// Wall-clock timestamp (ms since UNIX epoch) when the entry was
  /// recorded. Renderer turns this into a compact "5s / 12m / 1h / 2d"
  /// label drawn outside the highlight bg on the left of the row.
  pub ts_ms: i64,
  /// User-perceived character count (`text.chars().count()`). Renderer turns
  /// this into a compact label ("47" / "1.2k" / "12k") rendered above the
  /// time-ago label inside the highlight bg, top-anchored. Same
  /// right-aligned column as the timestamp; body wrap budget unchanged.
  pub char_count: usize,
}

pub fn set_overlay_history_content(
  entries: &[HistoryEntryView<'_>],
  selected_index: usize,
  visible_start: usize,
  expanded: bool,
) {
  let Some(refs) = current_overlay() else {
    return;
  };
  unsafe {
    move_overlay_to_target_screen(refs, false);
    render_overlay_history_list(refs, entries, selected_index, visible_start, expanded);
  }
}

pub fn set_overlay_top_stream_content(draft: &str, activity: &[f32], busy_phase: Option<f32>) {
  let Some(refs) = current_overlay_top() else {
    return;
  };
  unsafe {
    if let Some(bottom) = current_overlay() {
      position_overlay_top_relative_to_bottom(refs, bottom);
    }
    render_overlay_text(refs, draft, activity, busy_phase, false, false, "", "");
    if let Some(bottom) = current_overlay() {
      position_overlay_top_relative_to_bottom(refs, bottom);
    }
  }
}

pub fn set_overlay_notice_content(title: &str, body: &str) {
  set_overlay_notice_content_styled(
    title,
    &[OverlayNoticeSegment::Text(body.to_string())],
    OverlayNoticeStyle::Standard,
  );
}

pub fn set_overlay_listen_toggle_notice_content(
  title: &str,
  body_segments: &[OverlayNoticeSegment],
  enabled: bool,
  progress: f32,
) {
  set_overlay_notice_content_styled(
    title,
    body_segments,
    OverlayNoticeStyle::ListenToggle { enabled, progress: progress.clamp(0.0, 1.0) },
  );
}

fn render_overlay_notice_body(segments: &[OverlayNoticeSegment]) -> String {
  let mut out = String::new();
  for seg in segments {
    match seg {
      OverlayNoticeSegment::Text(t) => out.push_str(t),
      OverlayNoticeSegment::Keycap(k) => {
        out.push('[');
        out.push_str(k);
        out.push(']');
      }
    }
  }
  out
}

fn set_overlay_notice_content_styled(
  title: &str,
  body_segments: &[OverlayNoticeSegment],
  style: OverlayNoticeStyle,
) {
  let Some(refs) = current_overlay() else {
    return;
  };
  let title = title.trim();
  let body = render_overlay_notice_body(body_segments);
  let body = body.trim();
  let rendered = if body.is_empty() { title.to_string() } else { format!("{title}\n{body}") };

  unsafe {
    move_overlay_to_target_screen(refs, false);
    let notice_activity = match style {
      OverlayNoticeStyle::Standard => Vec::new(),
      OverlayNoticeStyle::ListenToggle { enabled, progress } => {
        listen_toggle_notice_activity(enabled, progress)
      }
    };
    render_overlay_text(refs, &rendered, &notice_activity, None, false, false, "", "");
    apply_overlay_notice_style(refs, style);
    hide_overlay_notice_accessory(refs);
  }
}

pub fn is_option_pressed() -> bool {
  unsafe {
    let flags: u64 = msg_send![class!(NSEvent), modifierFlags];
    (flags & NSEVENT_MODIFIER_FLAG_OPTION) != 0
  }
}

pub fn is_raw_mode_pressed() -> bool {
  is_option_pressed()
}

pub fn hold_hotkey_overlaps_raw_modifier() -> bool {
  LISTEN_MODIFIERS.load(Ordering::Relaxed) & MOD_OPTION != 0
}

pub fn insert_text(text: &str, method: PasteMethod, paste_delay_ms: u64) -> PasteResult {
  if text.trim().is_empty() {
    return PasteResult::EmptyText;
  }

  // In test builds, report success without touching the real Accessibility /
  // clipboard / keystroke FFI. App-logic tests exercise the post-paste state
  // transitions; routing them through the live paths would (a) make them depend
  // on the unsigned test binary's TCC grant — which resets on every rebuild and
  // flips `AccessibilityRequired`, wiping the state under test — and (b) inject
  // real Cmd+V keystrokes into whatever app is focused during `cargo test`.
  if cfg!(test) {
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
        // Clipboard propagation delay so focused target app sees the new value.
        std::thread::sleep(Duration::from_millis(paste_delay_ms));
        send_command_v();
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
          send_command_v();
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
          send_command_v();
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

fn register_delegate_class() -> &'static Class {
  DELEGATE_CLASS.get_or_init(|| unsafe {
    let superclass = class!(NSObject);
    let mut decl =
      ClassDecl::new("AzadStatusDelegate", superclass).expect("failed to declare delegate");

    decl.add_ivar::<id>("statusItem");
    decl.add_method(
      sel!(toggleAlwaysListening:),
      toggle_always_listening as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(sel!(quit:), quit as extern "C" fn(&Object, Sel, id));
    decl.add_method(sel!(tick:), tick as extern "C" fn(&Object, Sel, id));
    decl.add_method(sel!(toggleDevices:), toggle_devices as extern "C" fn(&Object, Sel, id));
    decl.add_method(sel!(openSettings:), open_settings as extern "C" fn(&Object, Sel, id));
    decl.add_method(
      sel!(onboardingGetStarted:),
      onboarding_get_started as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingSetTrigger:),
      onboarding_set_trigger as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingToggleHistory:),
      onboarding_toggle_history as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingToggleAppendTrailingSpace:),
      onboarding_toggle_append_trailing_space as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingSetOverlayPosition:),
      onboarding_set_overlay_position as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingToggleLogin:),
      onboarding_toggle_login as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingOpenPermission:),
      onboarding_open_permission as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingDownloadModel:),
      onboarding_download_model as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingSelectDevice:),
      onboarding_select_device as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(onboardingToggleListenModifier:),
      onboarding_toggle_listen_modifier as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsToggleRunOnStartup:),
      settings_toggle_run_on_startup as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsToggleDebug:),
      settings_toggle_debug as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsToggleConnector:),
      settings_toggle_connector as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsSelectPasteMethod:),
      settings_select_paste_method as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsSelectAutoSubmit:),
      settings_select_auto_submit as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsSelectOverlayPosition:),
      settings_select_overlay_position as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsToggleAppendTrailingSpace:),
      settings_toggle_append_trailing_space as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsAddRemovedWord:),
      settings_add_removed_word as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsRemoveRemovedWord:),
      settings_remove_removed_word as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(windowWillClose:),
      settings_window_will_close as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(sel!(settingsRefresh:), settings_refresh as extern "C" fn(&Object, Sel, id));
    decl.add_method(
      sel!(settingsDownloadModel:),
      settings_download_model as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(settingsCancelDownload:),
      settings_cancel_download as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(numberOfRowsInTableView:),
      settings_tab_rows as extern "C" fn(&Object, Sel, id) -> isize,
    );
    decl.add_method(
      sel!(tableView:viewForTableColumn:row:),
      settings_tab_row_view as extern "C" fn(&Object, Sel, id, id, isize) -> id,
    );
    decl.add_method(
      sel!(tableViewSelectionDidChange:),
      settings_tab_selection_did_change as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(sel!(selectDevice:), select_device as extern "C" fn(&Object, Sel, id));
    decl.add_method(sel!(menuWillOpen:), menu_will_open as extern "C" fn(&Object, Sel, id));
    decl.add_method(sel!(menuDidClose:), menu_did_close as extern "C" fn(&Object, Sel, id));
    decl.add_method(
      sel!(menu:willHighlightItem:),
      menu_will_highlight_item as extern "C" fn(&Object, Sel, id, id),
    );
    decl.add_method(sel!(syncMenuLayout:), sync_menu_layout as extern "C" fn(&Object, Sel, id));
    decl.add_method(sel!(noop:), noop as extern "C" fn(&Object, Sel, id));

    decl.register()
  })
}

fn register_overlay_window_class() -> &'static Class {
  OVERLAY_WINDOW_CLASS.get_or_init(|| unsafe {
    // Subclass NSPanel (not NSWindow) so we can pair canBecomeKeyWindow=YES
    // with the NSWindowStyleMaskNonactivatingPanel style mask: the panel
    // takes key without bringing Azad to the foreground, the same trick
    // Spotlight uses. That gives the search field a real first-responder
    // status — and therefore a real blinking caret — while the user's
    // foreground app keeps its menu bar and dock badge.
    let superclass = class!(NSPanel);
    let mut decl = ClassDecl::new("AzadOverlayWindow", superclass)
      .expect("failed to declare overlay window class");

    decl.add_method(
      sel!(canBecomeKeyWindow),
      overlay_can_become_key_window as extern "C" fn(&Object, Sel) -> bool,
    );
    decl.add_method(
      sel!(canBecomeMainWindow),
      overlay_can_become_main_window as extern "C" fn(&Object, Sel) -> bool,
    );
    decl.add_method(sel!(keyDown:), overlay_key_down as extern "C" fn(&Object, Sel, id));

    decl.register()
  })
}

fn register_search_field_delegate_class() -> &'static Class {
  SEARCH_FIELD_DELEGATE_CLASS.get_or_init(|| unsafe {
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("AzadSearchFieldDelegate", superclass)
      .expect("failed to declare search field delegate class");
    decl.add_method(
      sel!(controlTextDidChange:),
      search_field_text_did_change as extern "C" fn(&Object, Sel, id),
    );
    decl.register()
  })
}

extern "C" fn search_field_text_did_change(_: &Object, _: Sel, notification: id) {
  unsafe {
    let field: id = msg_send![notification, object];
    if field == nil {
      return;
    }
    let value: id = msg_send![field, stringValue];
    if value == nil {
      return;
    }
    let utf8: *const c_char = msg_send![value, UTF8String];
    if utf8.is_null() {
      return;
    }
    let s = match CStr::from_ptr(utf8).to_str() {
      Ok(s) => s.to_string(),
      Err(_) => return,
    };
    crate::app::send_event(crate::app::AppEvent::HistorySearchChanged(s));
  }
}

fn register_device_header_view_class() -> &'static Class {
  DEVICE_HEADER_VIEW_CLASS.get_or_init(|| unsafe {
    let superclass = class!(NSView);
    let mut decl = ClassDecl::new("AzadDeviceHeaderView", superclass)
      .expect("failed to declare device header view class");

    decl.add_method(
      sel!(setFrame:),
      device_header_view_set_frame as extern "C" fn(&Object, Sel, NSRect),
    );
    decl.add_method(
      sel!(setFrameSize:),
      device_header_view_set_frame_size as extern "C" fn(&Object, Sel, NSSize),
    );
    decl.add_method(
      sel!(viewDidMoveToWindow),
      device_header_view_did_move_to_window as extern "C" fn(&Object, Sel),
    );
    decl.add_method(
      sel!(viewDidMoveToSuperview),
      device_header_view_did_move_to_superview as extern "C" fn(&Object, Sel),
    );

    decl.register()
  })
}

unsafe fn menu_window_content_width_for_view(view: id) -> Option<f64> {
  if view == nil {
    return None;
  }

  let window: id = msg_send![view, window];
  if window == nil {
    return None;
  }

  let content_view: id = msg_send![window, contentView];
  if content_view != nil {
    let bounds: NSRect = msg_send![content_view, bounds];
    if bounds.size.width.is_finite() && bounds.size.width > 0.0 {
      return Some(bounds.size.width);
    }
  }

  let frame: NSRect = msg_send![window, frame];
  if frame.size.width.is_finite() && frame.size.width > 0.0 {
    return Some(frame.size.width);
  }

  None
}

extern "C" fn device_header_view_set_frame(this: &Object, _: Sel, mut rect: NSRect) {
  unsafe {
    let view_id = this as *const Object as id;
    if let Some(window_width) = menu_window_content_width_for_view(view_id) {
      // Pin the custom row width to the *rendered menu window* width.
      // This prevents AppKit from leaving our custom view narrower than the
      // outer menu background during expand/collapse transitions.
      rect.size.width = window_width;
    }
    let _: () = msg_send![super(view_id, class!(NSView)), setFrame: rect];
  }
}

extern "C" fn device_header_view_set_frame_size(this: &Object, _: Sel, mut size: NSSize) {
  unsafe {
    let view_id = this as *const Object as id;
    if let Some(window_width) = menu_window_content_width_for_view(view_id) {
      size.width = window_width;
    }
    let _: () = msg_send![super(view_id, class!(NSView)), setFrameSize: size];
  }
}

extern "C" fn device_header_view_did_move_to_window(this: &Object, _: Sel) {
  unsafe {
    let view_id = this as *const Object as id;
    let _: () = msg_send![super(view_id, class!(NSView)), viewDidMoveToWindow];
    // Re-apply pinned width now that we have a window (menu attaches lazily).
    let frame: NSRect = msg_send![view_id, frame];
    let _: () = msg_send![view_id, setFrame: frame];
  }
}

extern "C" fn device_header_view_did_move_to_superview(this: &Object, _: Sel) {
  unsafe {
    let view_id = this as *const Object as id;
    let _: () = msg_send![super(view_id, class!(NSView)), viewDidMoveToSuperview];
    // In some transitions AppKit re-parents/re-lays out menu item views.
    // Re-assert our pinned width on reattachment.
    let frame: NSRect = msg_send![view_id, frame];
    let _: () = msg_send![view_id, setFrame: frame];
  }
}

extern "C" fn toggle_always_listening(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::MenuToggleAlwaysListening);
  crate::app::drain_events();
}

extern "C" fn quit(_: &Object, _: Sel, _: id) {
  unsafe {
    let app = NSApp();
    let _: () = msg_send![app, terminate: nil];
  }
}

extern "C" fn tick(_: &Object, _: Sel, _: id) {
  crate::app::drain_events();
}

extern "C" fn toggle_devices(this: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::MenuToggleDevices);
  crate::app::drain_events();
  schedule_menu_layout_sync(this as *const Object as id);
}

extern "C" fn open_settings(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::MenuOpenSettings);
  crate::app::drain_events();
}

extern "C" fn onboarding_get_started(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::OnboardingGetStarted);
  crate::app::drain_events();
}

extern "C" fn onboarding_set_trigger(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let index: i64 = msg_send![sender, indexOfSelectedItem];
    // Item 0 = Automatically (always listening), 1 = Manually.
    crate::app::send_event(AppEvent::OnboardingSetTrigger(index == 0));
    crate::app::drain_events();
  }
}

extern "C" fn onboarding_toggle_history(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::OnboardingToggleHistory(state != 0));
    crate::app::drain_events();
  }
}

extern "C" fn onboarding_toggle_append_trailing_space(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::OnboardingToggleAppendTrailingSpace(state != 0));
    crate::app::drain_events();
  }
}

extern "C" fn onboarding_set_overlay_position(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let index: i64 = msg_send![sender, indexOfSelectedItem];
    crate::app::send_event(AppEvent::OnboardingSetOverlayPosition(OverlayPosition::from_ui_index(
      index,
    )));
    crate::app::drain_events();
  }
}

extern "C" fn onboarding_toggle_login(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::OnboardingToggleLogin(state != 0));
    crate::app::drain_events();
  }
}

extern "C" fn onboarding_download_model(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::OnboardingDownloadModel);
  crate::app::drain_events();
}

extern "C" fn onboarding_select_device(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let index: i64 = msg_send![sender, indexOfSelectedItem];
    if index < 0 {
      return;
    }
    crate::app::send_event(AppEvent::OnboardingSelectDevice(index as usize));
    crate::app::drain_events();
  }
}

extern "C" fn onboarding_toggle_listen_modifier(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let tag: i64 = msg_send![sender, tag];
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::OnboardingSetListenModifier {
      bit: tag as u8,
      enabled: state != 0,
    });
    crate::app::drain_events();
  }
}

extern "C" fn onboarding_open_permission(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let tag: i64 = msg_send![sender, tag];
    let anchor = match tag {
      0 => "Privacy_Accessibility",
      1 => "Privacy_Microphone",
      _ => return,
    };
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
  }
}

extern "C" fn settings_toggle_run_on_startup(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::SettingsToggleRunOnStartup(state != 0));
    crate::app::drain_events();
  }
}

extern "C" fn settings_toggle_debug(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::SettingsToggleDebugStats(state != 0));
    crate::app::drain_events();
  }
}

extern "C" fn settings_toggle_connector(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let index: isize = msg_send![sender, tag];
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::SettingsToggleConnector {
      index: index as usize,
      enabled: state != 0,
    });
    crate::app::drain_events();
  }
}

extern "C" fn settings_select_paste_method(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let index: i64 = msg_send![sender, indexOfSelectedItem];
    crate::app::send_event(AppEvent::SettingsSelectPasteMethod(PasteMethod::from_ui_index(index)));
    crate::app::drain_events();
  }
}

extern "C" fn settings_select_auto_submit(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let index: i64 = msg_send![sender, indexOfSelectedItem];
    crate::app::send_event(AppEvent::SettingsSelectAutoSubmit(AutoSubmitMode::from_ui_index(
      index,
    )));
    crate::app::drain_events();
  }
}

extern "C" fn settings_select_overlay_position(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let index: i64 = msg_send![sender, indexOfSelectedItem];
    crate::app::send_event(AppEvent::SettingsSelectOverlayPosition(
      OverlayPosition::from_ui_index(index),
    ));
    crate::app::drain_events();
  }
}

extern "C" fn settings_toggle_append_trailing_space(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let state: i64 = msg_send![sender, state];
    crate::app::send_event(AppEvent::SettingsToggleAppendTrailingSpace(state != 0));
    crate::app::drain_events();
  }
}

extern "C" fn settings_add_removed_word(_: &Object, _: Sel, _: id) {
  if let Some(refs) = current_settings_window() {
    unsafe {
      let value: id = msg_send![refs.removed_words_input, stringValue];
      if let Some(text) = nsstring_to_string(value) {
        if !text.is_empty() {
          let _: () = msg_send![
              refs.removed_words_input,
              setStringValue: NSString::alloc(nil).init_str("")
          ];
          crate::app::send_event(AppEvent::SettingsAddRemovedWord(text));
          crate::app::drain_events();
        }
      }
    }
  }
}

extern "C" fn settings_remove_removed_word(_: &Object, _: Sel, sender: id) {
  unsafe {
    if sender == nil {
      return;
    }
    let tag: isize = msg_send![sender, tag];
    let model = SETTINGS_LAST_MODEL.with(|m| m.borrow().clone());
    if let Some(model) = model {
      if let Some(word) = model.removed_words.get(tag as usize) {
        crate::app::send_event(AppEvent::SettingsRemoveRemovedWord(word.clone()));
        crate::app::drain_events();
      }
    }
  }
}

extern "C" fn settings_refresh(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::SettingsRefresh);
  crate::app::drain_events();
}

extern "C" fn settings_download_model(_: &Object, _: Sel, _: id) {
  if let Some(refs) = current_settings_window() {
    let pack_id = unsafe {
      let tag: isize = msg_send![refs.models_download_button, tag];
      settings_pack_id_for_tag(tag)
    };
    crate::app::send_event(AppEvent::SettingsDownloadModel(pack_id));
    crate::app::drain_events();
  }
}

extern "C" fn settings_cancel_download(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::SettingsCancelDownload);
  crate::app::drain_events();
}

fn settings_pack_id_for_tag(_tag: isize) -> String {
  crate::models::default_pack().id.to_string()
}

extern "C" fn settings_tab_rows(_: &Object, _: Sel, _: id) -> isize {
  5
}

unsafe fn settings_tab_label(row: isize) -> &'static str {
  match row {
    1 => "Models",
    2 => "Permissions",
    3 => "Debug",
    4 => "Connectors",
    _ => "General",
  }
}

unsafe fn settings_tab_from_row(row: isize) -> SettingsTab {
  match row {
    1 => SettingsTab::Models,
    2 => SettingsTab::Permissions,
    3 => SettingsTab::Debug,
    4 => SettingsTab::Connectors,
    _ => SettingsTab::General,
  }
}

unsafe fn settings_row_for_tab(tab: SettingsTab) -> isize {
  match tab {
    SettingsTab::General => 0,
    SettingsTab::Models => 1,
    SettingsTab::Permissions => 2,
    SettingsTab::Debug => 3,
    SettingsTab::Connectors => 4,
  }
}

extern "C" fn settings_tab_row_view(_: &Object, _: Sel, table_view: id, _: id, row: isize) -> id {
  unsafe {
    if table_view == nil {
      return nil;
    }

    let identifier = NSString::alloc(nil).init_str("AzadSettingsTabCell");
    let mut cell: id = msg_send![table_view, makeViewWithIdentifier: identifier owner: nil];
    if cell == nil {
      let row_height: f64 = msg_send![table_view, rowHeight];
      let bounds: NSRect = msg_send![table_view, bounds];
      let width = bounds.size.width.max(SETTINGS_SIDEBAR_WIDTH);

      let created: id = msg_send![class!(NSTableCellView), alloc];
      cell = msg_send![created, initWithFrame: NSRect::new(
          NSPoint::new(0.0, 0.0),
          NSSize::new(width, row_height.max(SETTINGS_SIDEBAR_ROW_HEIGHT))
      )];
      let _: () = msg_send![cell, setIdentifier: identifier];

      let text: id = msg_send![class!(NSTextField), alloc];
      let text: id = msg_send![text, initWithFrame: NSRect::new(
          NSPoint::new(10.0, 4.0),
          NSSize::new((width - 20.0).max(1.0), (row_height - 8.0).max(1.0))
      )];
      let _: () = msg_send![text, setBezeled: NO];
      let _: () = msg_send![text, setDrawsBackground: NO];
      let _: () = msg_send![text, setEditable: NO];
      let _: () = msg_send![text, setSelectable: NO];
      let _: () = msg_send![text, setAlignment: 0usize];
      let _: () = msg_send![text, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];
      let font: id = msg_send![class!(NSFont), systemFontOfSize: 13.0f64];
      if font != nil {
        let _: () = msg_send![text, setFont: font];
      }
      let _: () = msg_send![cell, addSubview: text];
      let _: () = msg_send![cell, setTextField: text];
    }

    let text_field: id = msg_send![cell, textField];
    if text_field != nil {
      let label = NSString::alloc(nil).init_str(settings_tab_label(row));
      let _: () = msg_send![text_field, setStringValue: label];
    }
    cell
  }
}

extern "C" fn settings_tab_selection_did_change(_: &Object, _: Sel, notification: id) {
  unsafe {
    if notification == nil {
      return;
    }
    let table_view: id = msg_send![notification, object];
    if table_view == nil {
      return;
    }
    let row: isize = msg_send![table_view, selectedRow];
    if row < 0 {
      return;
    }
    if let Some(refs) = current_settings_window() {
      if refs.tab_list_view != table_view {
        return;
      }
      apply_settings_selected_tab(refs, settings_tab_from_row(row));
    }
  }
}

extern "C" fn settings_window_will_close(_: &Object, _: Sel, _: id) {
  SETTINGS_WINDOW_REFS.with(|store| {
    store.borrow_mut().take();
  });
  unsafe {
    let app = NSApp();
    app.setActivationPolicy_(NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory);
  }
}

extern "C" fn select_device(_: &Object, _: Sel, sender: id) {
  unsafe {
    let tag: i64 = msg_send![sender, tag];
    if tag < 0 {
      return;
    }

    let selected = DEVICE_ROW_IDS.with(|rows| rows.borrow().get(tag as usize).cloned());
    if let Some(device_id) = selected {
      crate::app::send_event(AppEvent::MenuSelectDevice(device_id));
    }
  }
}

extern "C" fn menu_will_open(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::MenuOpened);

  let menu = STATUS_MENU_REF.with(|slot| *slot.borrow());
  let Some(menu) = menu else {
    return;
  };

  let model = DEVICE_MENU_MODEL.with(|slot| slot.borrow().clone());
  let target_width = compute_device_menu_target_width(&model);
  apply_device_menu_layout(menu, target_width);

  let delegate = STATUS_DELEGATE_REF.with(|slot| *slot.borrow());
  if let Some(delegate) = delegate {
    schedule_menu_layout_sync(delegate);
  }
}

extern "C" fn menu_did_close(_: &Object, _: Sel, _: id) {
  crate::app::send_event(AppEvent::MenuClosed);
  set_device_header_highlighted(false);
  reset_device_menu_open_width_sticky_state();
}

extern "C" fn menu_will_highlight_item(_: &Object, _: Sel, _: id, item: id) {
  let is_header = unsafe {
    if item == nil {
      false
    } else {
      let item_view: id = msg_send![item, view];
      DEVICE_HEADER_VIEW_REF
        .with(|slot| slot.borrow().is_some_and(|header_view| header_view == item_view))
    }
  };
  set_device_header_highlighted(is_header);
}

extern "C" fn sync_menu_layout(_: &Object, _: Sel, _: id) {
  sync_device_header_width_to_live_menu();
}

extern "C" fn noop(_: &Object, _: Sel, _: id) {}

extern "C" fn overlay_can_become_key_window(_: &Object, _: Sel) -> bool {
  OVERLAY_ACCEPTS_KEY_INPUT.load(Ordering::Relaxed)
}

/// Toggle whether the overlay intercepts keystrokes (via the HID event tap)
/// AND whether the panel takes key window status. The HID tap is what
/// actually feeds the search field; promoting the panel to key + the field
/// to first responder gives AppKit enough state to draw the caret. Because
/// the overlay panel uses the non-activating style mask, becoming key does
/// NOT bring Azad to the foreground — the user's app keeps its menu bar.
pub fn set_overlay_key_input_enabled(enabled: bool) {
  OVERLAY_ACCEPTS_KEY_INPUT.store(enabled, Ordering::Relaxed);
  let Some(refs) = current_overlay() else { return };
  unsafe {
    if enabled {
      let _: () = msg_send![refs.window, makeKeyWindow];
      if refs.search_field != nil {
        let _: () = msg_send![refs.window, makeFirstResponder: refs.search_field];
      }
    } else {
      let _: () = msg_send![refs.window, resignKeyWindow];
    }
  }
}

/// Programmatically set the search field's text without firing the
/// `controlTextDidChange:` delegate (used to clear the field on
/// enter/exit). NSTextField's `setStringValue:` doesn't trigger the
/// notification, so this is safe by construction.
pub fn set_overlay_search_query(s: &str) {
  let Some(refs) = current_overlay() else { return };
  if refs.search_field == nil {
    return;
  }
  unsafe {
    let ns = NSString::alloc(nil).init_str(s);
    let _: () = msg_send![refs.search_field, setStringValue: ns];
  }
}

extern "C" fn overlay_can_become_main_window(_: &Object, _: Sel) -> bool {
  false
}

extern "C" fn overlay_key_down(this: &Object, _: Sel, event: id) {
  unsafe {
    let key_code: u16 = msg_send![event, keyCode];
    if key_code == 53 {
      crate::app::send_event(AppEvent::OverlayCancel);
      return;
    }

    let window_id = this as *const Object as id;
    let _: () = msg_send![super(window_id, class!(NSWindow)), keyDown: event];
  }
}

unsafe fn setup_status_bar(delegate: id) {
  let status_item =
    NSStatusBar::systemStatusBar(nil).statusItemWithLength_(NSVariableStatusItemLength);
  let menu = NSMenu::new(nil).autorelease();

  let _: () = msg_send![menu, setDelegate: delegate];

  status_item.setMenu_(menu);
  assign_status_icon(status_item);

  STATUS_MENU_REF.with(|slot| {
    slot.borrow_mut().replace(menu);
  });

  // Poll app events on the main thread.
  let timer: id = msg_send![
      class!(NSTimer),
      scheduledTimerWithTimeInterval: 0.05f64
      target: delegate
      selector: sel!(tick:)
      userInfo: nil
      repeats: YES
  ];
  let run_loop: id = msg_send![class!(NSRunLoop), mainRunLoop];
  let tracking_mode = NSString::alloc(nil).init_str("NSEventTrackingRunLoopMode");
  // Also run while the menu is open and tracking mouse hover.
  let _: () = msg_send![run_loop, addTimer: timer forMode: tracking_mode];

  let delegate_obj = &mut *(delegate as *mut Object);
  delegate_obj.set_ivar("statusItem", status_item);

  rebuild_status_menu();
}

fn rebuild_status_menu() {
  let menu = STATUS_MENU_REF.with(|slot| *slot.borrow());
  let delegate = STATUS_DELEGATE_REF.with(|slot| *slot.borrow());
  let model = DEVICE_MENU_MODEL.with(|slot| slot.borrow().clone());

  let (Some(menu), Some(delegate)) = (menu, delegate) else {
    return;
  };

  let target_width = compute_device_menu_target_width(&model);
  if menu_is_open(menu) {
    update_menu_inline(menu, delegate, &model);
  } else {
    build_menu_fresh(menu, delegate, &model);
  }
  apply_device_menu_layout(menu, target_width);
}

fn menu_is_open(menu: id) -> bool {
  unsafe {
    let attached: i8 = msg_send![menu, isAttached];
    attached != 0
  }
}

fn apply_device_menu_layout(menu: id, target_width: f64) {
  if !target_width.is_finite() || target_width <= 0.0 {
    return;
  }

  let max_width = menu_screen_width_cap();
  let min_width = DEVICE_HEADER_MIN_WIDTH.min(max_width);
  let width = target_width.max(min_width).min(max_width);
  let menu_width = if menu_is_open(menu) { sticky_menu_open_width(width) } else { width };

  unsafe {
    let can_set_minimum: i8 = msg_send![menu, respondsToSelector: sel!(setMinimumWidth:)];
    if can_set_minimum != 0 {
      let _: () = msg_send![menu, setMinimumWidth: menu_width];
    }
    let can_size_to_fit: i8 = msg_send![menu, respondsToSelector: sel!(sizeToFit)];
    if can_size_to_fit != 0 {
      let _: () = msg_send![menu, sizeToFit];
    }
  }

  let row_width = adjusted_header_row_width(menu_width);
  let final_width =
    if menu_is_open(menu) { sticky_header_open_width(row_width) } else { row_width };
  relayout_device_header_row(final_width);
}

fn sync_device_header_width_to_live_menu() {
  let menu = STATUS_MENU_REF.with(|slot| *slot.borrow());
  let Some(menu) = menu else {
    return;
  };
  if !menu_is_open(menu) {
    return;
  }

  let width = current_menu_width(menu);
  if width > 0.0 {
    let stable_menu_width = sticky_menu_open_width(width);
    let final_width = sticky_header_open_width(adjusted_header_row_width(stable_menu_width));
    relayout_device_header_row(final_width);
  }
}

fn sticky_header_open_width(candidate: f64) -> f64 {
  if !candidate.is_finite() || candidate <= 0.0 {
    return candidate;
  }

  DEVICE_HEADER_OPEN_MAX_WIDTH.with(|slot| {
    let mut current = slot.borrow_mut();
    let next = match *current {
      Some(previous) => previous.max(candidate),
      None => candidate,
    };
    *current = Some(next);
    next
  })
}

fn sticky_menu_open_width(candidate: f64) -> f64 {
  if !candidate.is_finite() || candidate <= 0.0 {
    return candidate;
  }

  DEVICE_MENU_OPEN_MAX_WIDTH.with(|slot| {
    let mut current = slot.borrow_mut();
    let next = match *current {
      Some(previous) => previous.max(candidate),
      None => candidate,
    };
    *current = Some(next);
    next
  })
}

fn reset_device_menu_open_width_sticky_state() {
  DEVICE_HEADER_OPEN_MAX_WIDTH.with(|slot| {
    slot.borrow_mut().take();
  });
  DEVICE_MENU_OPEN_MAX_WIDTH.with(|slot| {
    slot.borrow_mut().take();
  });
}

fn adjusted_header_row_width(base_width: f64) -> f64 {
  let max_width = menu_screen_width_cap();
  let adjusted = (base_width + DEVICE_HEADER_MENU_COMPENSATION).min(max_width);
  if adjusted.is_finite() && adjusted > 0.0 { adjusted } else { base_width }
}

fn schedule_menu_layout_sync(delegate: id) {
  unsafe {
    if delegate == nil {
      return;
    }
    let _: () = msg_send![
        delegate,
        performSelector: sel!(syncMenuLayout:)
        withObject: nil
        afterDelay: 0.0f64
    ];
  }
}

fn current_menu_width(menu: id) -> f64 {
  unsafe {
    if menu == nil {
      return 0.0;
    }

    let header_context_width = DEVICE_HEADER_VIEW_REF.with(|slot| {
      let Some(view) = *slot.borrow() else {
        return None;
      };

      // Prefer the menu window's content width. This matches the rendered
      // menu background and stays stable across expand/collapse.
      if let Some(width) = menu_window_content_width_for_view(view) {
        return Some(width);
      }

      // Fallback: immediate superview width (can be narrower than the
      // menu background on some AppKit transitions).
      let superview: id = msg_send![view, superview];
      if superview == nil {
        return None;
      }

      let bounds: NSRect = msg_send![superview, bounds];
      if bounds.size.width.is_finite() && bounds.size.width > 0.0 {
        return Some(bounds.size.width);
      }

      let frame: NSRect = msg_send![superview, frame];
      if frame.size.width.is_finite() && frame.size.width > 0.0 {
        return Some(frame.size.width);
      }

      None
    });
    if let Some(width) = header_context_width {
      return width;
    }

    let menu_size: NSSize = msg_send![menu, size];
    if menu_size.width.is_finite() && menu_size.width > 0.0 {
      menu_size.width
    } else {
      DEVICE_HEADER_WIDTH
    }
  }
}

fn relayout_device_header_row(view_width: f64) {
  if !view_width.is_finite() || view_width <= 0.0 {
    return;
  }
  let view_height = DEVICE_HEADER_HEIGHT;

  unsafe {
    DEVICE_HEADER_VIEW_REF.with(|slot| {
      if let Some(view) = *slot.borrow() {
        let _: () = msg_send![
            view,
            setFrame: NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(view_width, view_height)
            )
        ];
      }
    });
  }
}

fn compute_device_menu_target_width(model: &DeviceMenuModel) -> f64 {
  unsafe {
    let font = menu_row_font();
    let mut max_width = DEVICE_HEADER_MIN_WIDTH;

    max_width = max_width.max(always_listening_row_width(font));
    max_width = max_width.max(menu_row_width_for_text("Quit", font, 0, false));
    max_width = max_width.max(menu_row_width_for_text("Settings...", font, 0, false));

    if model.expanded {
      if model.rows.is_empty() {
        max_width = max_width.max(menu_row_width_for_text("No input devices", font, 1, false));
      } else {
        for row in &model.rows {
          max_width = max_width.max(menu_row_width_for_text(&row.label, font, 1, true));
        }
      }
    }

    max_width = max_width.max(device_header_width_for_label(&device_header_label(model), font));

    let screen_cap = menu_screen_width_cap();
    max_width.min(screen_cap)
  }
}

unsafe fn always_listening_row_width(font: id) -> f64 {
  let text_width = measure_text_width("Listen", font);
  ALWAYS_LISTENING_LABEL_LEADING
    + text_width
    + ALWAYS_LISTENING_LABEL_TO_SWITCH_GAP
    + ALWAYS_LISTENING_SWITCH_WIDTH
    + DEVICE_HEADER_TRAILING
    + DEVICE_MENU_TEXT_SAFETY_PADDING
}

unsafe fn menu_row_font() -> id {
  let font_class = class!(NSFont);
  let supports_menu_font: i8 = msg_send![font_class, respondsToSelector: sel!(menuFontOfSize:)];
  if supports_menu_font != 0 {
    let menu_font: id = msg_send![font_class, menuFontOfSize: 0.0f64];
    if menu_font != nil {
      return menu_font;
    }
  }
  msg_send![font_class, systemFontOfSize: 13.0f64]
}

unsafe fn measure_text_width(text: &str, font: id) -> f64 {
  if text.is_empty() {
    return 0.0;
  }

  let ns_text = NSString::alloc(nil).init_str(text);
  if ns_text == nil {
    return text.chars().count() as f64 * 7.0;
  }

  let size: NSSize = if font != nil {
    let key = NSString::alloc(nil).init_str("NSFont");
    let attrs: id = msg_send![class!(NSDictionary), dictionaryWithObject: font forKey: key];
    msg_send![ns_text, sizeWithAttributes: attrs]
  } else {
    msg_send![ns_text, sizeWithAttributes: nil]
  };

  if size.width.is_finite() && size.width > 0.0 {
    size.width
  } else {
    text.chars().count() as f64 * 7.0
  }
}

unsafe fn menu_row_width_for_text(
  text: &str,
  font: id,
  indent_level: usize,
  with_checkmark: bool,
) -> f64 {
  let text_width = measure_text_width(text, font);
  let mut width = text_width + DEVICE_MENU_ROW_CHROME_WIDTH + DEVICE_MENU_TEXT_SAFETY_PADDING;
  width += indent_level as f64 * DEVICE_MENU_INDENT_LEVEL_WIDTH;
  if with_checkmark {
    width += DEVICE_MENU_CHECKMARK_WIDTH;
  }
  width
}

unsafe fn device_header_width_for_label(label: &str, font: id) -> f64 {
  let text_width = measure_text_width(label, font);
  let mut width = text_width
    + DEVICE_HEADER_TEXT_LEADING
    + DEVICE_HEADER_LABEL_TO_CHEVRON_GAP
    + DEVICE_HEADER_CHEVRON_SIZE
    + DEVICE_HEADER_TRAILING
    + DEVICE_MENU_TEXT_SAFETY_PADDING;
  width += 6.0 + (DEVICE_HEADER_EXTRA_SIDE_MARGIN * 2.0);
  width
}

fn menu_screen_width_cap() -> f64 {
  unsafe {
    let screen = NSScreen::mainScreen(nil);
    if screen == nil {
      return DEVICE_HEADER_WIDTH * 2.5;
    }

    let frame = NSScreen::frame(screen);
    let cap = frame.size.width - (DEVICE_MENU_SCREEN_EDGE_MARGIN * 2.0);
    if cap.is_finite() && cap > DEVICE_HEADER_MIN_WIDTH { cap } else { DEVICE_HEADER_WIDTH * 2.5 }
  }
}

fn build_menu_fresh(menu: id, delegate: id, model: &DeviceMenuModel) {
  unsafe {
    let _: () = msg_send![menu, removeAllItems];

    let always_item = make_always_listening_item(delegate, model.always_listening_enabled);
    menu.addItem_(always_item);

    let separator_top: id = msg_send![class!(NSMenuItem), separatorItem];
    menu.addItem_(separator_top);

    let header_item = make_device_header_item(delegate, model);
    menu.addItem_(header_item);

    insert_device_rows(menu, delegate, model, 3);

    let separator_bottom: id = msg_send![class!(NSMenuItem), separatorItem];
    menu.addItem_(separator_bottom);

    let settings_item = NSMenuItem::alloc(nil)
      .initWithTitle_action_keyEquivalent_(
        NSString::alloc(nil).init_str("Settings..."),
        sel!(openSettings:),
        NSString::alloc(nil).init_str(","),
      )
      .autorelease();
    settings_item.setTarget_(delegate);
    menu.addItem_(settings_item);

    let quit_item = NSMenuItem::alloc(nil)
      .initWithTitle_action_keyEquivalent_(
        NSString::alloc(nil).init_str("Quit"),
        sel!(quit:),
        NSString::alloc(nil).init_str("q"),
      )
      .autorelease();
    quit_item.setTarget_(delegate);
    menu.addItem_(quit_item);
  }
}

fn update_menu_inline(menu: id, delegate: id, model: &DeviceMenuModel) {
  set_always_listening_switch_state(model.always_listening_enabled);
  set_device_header_title(model);
  clear_device_rows(menu);

  unsafe {
    let count: i64 = msg_send![menu, numberOfItems];
    // Items are [Listen, Separator, Header, ..., Separator, Settings..., Quit]
    let insert_at = if count >= 3 { count - 3 } else { 0 };
    insert_device_rows(menu, delegate, model, insert_at);
  }
}

fn set_always_listening_switch_state(enabled: bool) {
  unsafe {
    ALWAYS_LISTENING_TRACK_REF.with(|slot| {
      if let Some(track) = *slot.borrow() {
        let layer: id = msg_send![track, layer];
        if layer != nil {
          let color = always_listening_track_color(track, enabled);
          if color != nil {
            let cg_color: id = msg_send![color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
          }
        }
      }
    });
    ALWAYS_LISTENING_THUMB_REF.with(|slot| {
      if let Some(thumb) = *slot.borrow() {
        let _: () = msg_send![thumb, setFrame: always_listening_thumb_frame(enabled)];
      }
    });
  }
}

unsafe fn always_listening_track_color(view: id, enabled: bool) -> id {
  let ns_color_class = class!(NSColor);
  let base_color: id = if enabled {
    msg_send![ns_color_class, systemBlueColor]
  } else {
    msg_send![ns_color_class, systemGrayColor]
  };

  if view == nil || base_color == nil {
    return base_color;
  }

  let appearance: id = msg_send![view, effectiveAppearance];
  let has_resolve: i8 =
    msg_send![base_color, respondsToSelector: sel!(resolvedColorWithAppearance:)];
  if appearance != nil && has_resolve != 0 {
    let resolved: id = msg_send![base_color, resolvedColorWithAppearance: appearance];
    if resolved != nil {
      return resolved;
    }
  }

  base_color
}

fn always_listening_thumb_frame(enabled: bool) -> NSRect {
  let x = if enabled {
    ALWAYS_LISTENING_SWITCH_WIDTH
      - ALWAYS_LISTENING_SWITCH_INSET
      - ALWAYS_LISTENING_SWITCH_THUMB_SIZE
  } else {
    ALWAYS_LISTENING_SWITCH_INSET
  };
  NSRect::new(
    NSPoint::new(x, ALWAYS_LISTENING_SWITCH_INSET),
    NSSize::new(ALWAYS_LISTENING_SWITCH_THUMB_SIZE, ALWAYS_LISTENING_SWITCH_THUMB_SIZE),
  )
}

fn device_header_label(model: &DeviceMenuModel) -> String {
  if model.header_label.trim().is_empty() {
    "No Input Device".to_string()
  } else {
    model.header_label.clone()
  }
}

fn device_header_chevron_image_name(model: &DeviceMenuModel) -> &'static str {
  if model.expanded { "NSTouchBarGoDownTemplate" } else { "NSGoRightTemplate" }
}

unsafe fn device_header_chevron_image(model: &DeviceMenuModel) -> id {
  let image_name = NSString::alloc(nil).init_str(device_header_chevron_image_name(model));
  msg_send![class!(NSImage), imageNamed: image_name]
}

unsafe fn set_image_view_tint_if_supported(image_view: id, tint_color: id) {
  let supports_tint: i8 = msg_send![image_view, respondsToSelector: sel!(setContentTintColor:)];
  if supports_tint != 0 {
    let _: () = msg_send![image_view, setContentTintColor: tint_color];
  }
}

unsafe fn resolved_menu_highlight_color_for_view(view: id) -> id {
  let ns_color_class = class!(NSColor);
  let has_selected_content_bg: i8 =
    msg_send![ns_color_class, respondsToSelector: sel!(selectedContentBackgroundColor)];
  let base_color: id = if has_selected_content_bg != 0 {
    msg_send![ns_color_class, selectedContentBackgroundColor]
  } else {
    msg_send![ns_color_class, selectedMenuItemColor]
  };

  if view == nil || base_color == nil {
    return base_color;
  }

  let appearance: id = msg_send![view, effectiveAppearance];
  let has_resolve: i8 =
    msg_send![base_color, respondsToSelector: sel!(resolvedColorWithAppearance:)];
  if appearance != nil && has_resolve != 0 {
    let resolved: id = msg_send![base_color, resolvedColorWithAppearance: appearance];
    if resolved != nil {
      return resolved;
    }
  }

  base_color
}

fn set_device_header_highlighted(is_highlighted: bool) {
  unsafe {
    DEVICE_HEADER_HIGHLIGHT_REF.with(|slot| {
      if let Some(bg) = *slot.borrow() {
        if is_highlighted {
          // Resolve menu selection color at highlight time so the custom row
          // follows current macOS appearance/accent.
          let layer: id = msg_send![bg, layer];
          if layer != nil {
            let highlight_color = resolved_menu_highlight_color_for_view(bg);
            let highlight_cg_color: id = msg_send![highlight_color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: highlight_cg_color];
          }
        }
        let _: () = msg_send![bg, setHidden: if is_highlighted { NO } else { YES }];
      }
    });

    DEVICE_HEADER_LABEL_REF.with(|slot| {
      if let Some(label) = *slot.borrow() {
        let color: id = if is_highlighted {
          msg_send![class!(NSColor), selectedMenuItemTextColor]
        } else {
          msg_send![class!(NSColor), labelColor]
        };
        let _: () = msg_send![label, setTextColor: color];
      }
    });

    DEVICE_HEADER_CHEVRON_REF.with(|slot| {
      if let Some(image_view) = *slot.borrow() {
        let tint: id = if is_highlighted {
          msg_send![class!(NSColor), selectedMenuItemTextColor]
        } else {
          msg_send![class!(NSColor), secondaryLabelColor]
        };
        set_image_view_tint_if_supported(image_view, tint);
      }
    });
  }
}

unsafe fn make_always_listening_item(delegate: id, enabled: bool) -> id {
  let item = NSMenuItem::alloc(nil)
    .initWithTitle_action_keyEquivalent_(
      NSString::alloc(nil).init_str(""),
      sel!(noop:),
      NSString::alloc(nil).init_str(""),
    )
    .autorelease();

  let view_frame = NSRect::new(
    NSPoint::new(0.0, 0.0),
    NSSize::new(DEVICE_HEADER_WIDTH, ALWAYS_LISTENING_ROW_HEIGHT),
  );
  let view: id = msg_send![class!(NSView), alloc];
  let view: id = msg_send![view, initWithFrame: view_frame];
  let _: () = msg_send![view, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];

  // Let users click anywhere on the row, not only on the switch thumb.
  let row_button: id = msg_send![class!(NSButton), alloc];
  let row_button: id = msg_send![row_button, initWithFrame: view_frame];
  let _: () =
    msg_send![row_button, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE];
  let _: () = msg_send![row_button, setBordered: NO];
  let _: () = msg_send![row_button, setTitle: NSString::alloc(nil).init_str("")];
  let _: () = msg_send![row_button, setTarget: delegate];
  let _: () = msg_send![row_button, setAction: sel!(toggleAlwaysListening:)];

  let label_width = DEVICE_HEADER_WIDTH
    - ALWAYS_LISTENING_LABEL_LEADING
    - DEVICE_HEADER_TRAILING
    - ALWAYS_LISTENING_SWITCH_WIDTH
    - ALWAYS_LISTENING_LABEL_TO_SWITCH_GAP;
  let label_frame =
    NSRect::new(NSPoint::new(ALWAYS_LISTENING_LABEL_LEADING, 2.0), NSSize::new(label_width, 18.0));
  let label: id = msg_send![class!(NSTextField), alloc];
  let label: id = msg_send![label, initWithFrame: label_frame];
  let _: () = msg_send![label, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];
  let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str("Listen")];
  let _: () = msg_send![label, setBezeled: NO];
  let _: () = msg_send![label, setDrawsBackground: NO];
  let _: () = msg_send![label, setEditable: NO];
  let _: () = msg_send![label, setSelectable: NO];
  let _: () = msg_send![label, setAlignment: 0isize];
  let font: id = msg_send![class!(NSFont), menuFontOfSize: 0.0f64];
  let _: () = msg_send![label, setFont: font];
  let text_color: id = msg_send![class!(NSColor), labelColor];
  let _: () = msg_send![label, setTextColor: text_color];

  let switch_x = DEVICE_HEADER_WIDTH - DEVICE_HEADER_TRAILING - ALWAYS_LISTENING_SWITCH_WIDTH;
  let switch_y = (ALWAYS_LISTENING_ROW_HEIGHT - ALWAYS_LISTENING_SWITCH_HEIGHT) * 0.5;
  let switch_frame = NSRect::new(
    NSPoint::new(switch_x, switch_y),
    NSSize::new(ALWAYS_LISTENING_SWITCH_WIDTH, ALWAYS_LISTENING_SWITCH_HEIGHT),
  );
  let switch_container: id = msg_send![class!(NSView), alloc];
  let switch_container: id = msg_send![switch_container, initWithFrame: switch_frame];
  let _: () = msg_send![switch_container, setAutoresizingMask: NS_VIEW_MIN_X_MARGIN];
  let _: () = msg_send![switch_container, setWantsLayer: YES];

  let track_frame = NSRect::new(
    NSPoint::new(0.0, 0.0),
    NSSize::new(ALWAYS_LISTENING_SWITCH_WIDTH, ALWAYS_LISTENING_SWITCH_HEIGHT),
  );
  let track_view: id = msg_send![class!(NSView), alloc];
  let track_view: id = msg_send![track_view, initWithFrame: track_frame];
  let _: () = msg_send![track_view, setWantsLayer: YES];
  let track_layer: id = msg_send![track_view, layer];
  if track_layer != nil {
    let _: () = msg_send![track_layer, setCornerRadius: (ALWAYS_LISTENING_SWITCH_HEIGHT * 0.5)];
    let color = always_listening_track_color(track_view, enabled);
    if color != nil {
      let cg_color: id = msg_send![color, CGColor];
      let _: () = msg_send![track_layer, setBackgroundColor: cg_color];
    }
  }

  let thumb_view: id = msg_send![class!(NSView), alloc];
  let thumb_view: id = msg_send![thumb_view, initWithFrame: always_listening_thumb_frame(enabled)];
  let _: () = msg_send![thumb_view, setWantsLayer: YES];
  let thumb_layer: id = msg_send![thumb_view, layer];
  if thumb_layer != nil {
    let _: () = msg_send![thumb_layer, setCornerRadius: (ALWAYS_LISTENING_SWITCH_THUMB_SIZE * 0.5)];
    let thumb_color: id = msg_send![class!(NSColor), whiteColor];
    let thumb_cg_color: id = msg_send![thumb_color, CGColor];
    let _: () = msg_send![thumb_layer, setBackgroundColor: thumb_cg_color];
  }

  let _: () = msg_send![switch_container, addSubview: track_view];
  let _: () = msg_send![switch_container, addSubview: thumb_view];

  let _: () = msg_send![view, addSubview: label];
  let _: () = msg_send![view, addSubview: switch_container];
  // Keep full-row click handling by layering this transparent button on top.
  let _: () = msg_send![view, addSubview: row_button];
  let _: () = msg_send![item, setView: view];

  ALWAYS_LISTENING_TRACK_REF.with(|slot| {
    slot.borrow_mut().replace(track_view);
  });
  ALWAYS_LISTENING_THUMB_REF.with(|slot| {
    slot.borrow_mut().replace(thumb_view);
  });
  set_always_listening_switch_state(enabled);

  item
}

unsafe fn make_device_header_item(delegate: id, model: &DeviceMenuModel) -> id {
  let item = NSMenuItem::alloc(nil)
    .initWithTitle_action_keyEquivalent_(
      NSString::alloc(nil).init_str(""),
      sel!(noop:),
      NSString::alloc(nil).init_str(""),
    )
    .autorelease();

  let view_frame =
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(DEVICE_HEADER_WIDTH, DEVICE_HEADER_HEIGHT));
  let header_view_class = register_device_header_view_class();
  let view: id = msg_send![header_view_class, alloc];
  let view: id = msg_send![view, initWithFrame: view_frame];
  let _: () = msg_send![view, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];

  let highlight_frame = NSRect::new(
    NSPoint::new(3.0 + DEVICE_HEADER_EXTRA_SIDE_MARGIN, 1.0 + DEVICE_HEADER_EXTRA_TOP_PADDING),
    NSSize::new(
      DEVICE_HEADER_WIDTH - (6.0 + DEVICE_HEADER_EXTRA_SIDE_MARGIN * 2.0),
      DEVICE_HEADER_HEIGHT - (2.0 + DEVICE_HEADER_EXTRA_TOP_PADDING),
    ),
  );
  let highlight_view: id = msg_send![class!(NSView), alloc];
  let highlight_view: id = msg_send![highlight_view, initWithFrame: highlight_frame];
  let _: () = msg_send![highlight_view, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];
  let _: () = msg_send![highlight_view, setWantsLayer: YES];
  let highlight_layer: id = msg_send![highlight_view, layer];
  let highlight_color: id = msg_send![class!(NSColor), selectedMenuItemColor];
  let highlight_cg_color: id = msg_send![highlight_color, CGColor];
  let _: () = msg_send![highlight_layer, setBackgroundColor: highlight_cg_color];
  let _: () = msg_send![highlight_layer, setCornerRadius: 4.0f64];
  let _: () = msg_send![highlight_view, setHidden: YES];

  let button_frame =
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(DEVICE_HEADER_WIDTH, DEVICE_HEADER_HEIGHT));
  let button: id = msg_send![class!(NSButton), alloc];
  let button: id = msg_send![button, initWithFrame: button_frame];
  let _: () =
    msg_send![button, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE];
  let _: () = msg_send![button, setBordered: NO];
  let _: () = msg_send![button, setTarget: delegate];
  let _: () = msg_send![button, setAction: sel!(toggleDevices:)];
  let _: () = msg_send![button, setTitle: NSString::alloc(nil).init_str("")];

  let font: id = msg_send![class!(NSFont), systemFontOfSize: 13.0f64];
  let text_color: id = msg_send![class!(NSColor), labelColor];

  let title_width = DEVICE_HEADER_WIDTH
    - DEVICE_HEADER_TEXT_LEADING
    - DEVICE_HEADER_TRAILING
    - DEVICE_HEADER_CHEVRON_SIZE
    - DEVICE_HEADER_LABEL_TO_CHEVRON_GAP;
  let title_label_frame = NSRect::new(
    NSPoint::new(DEVICE_HEADER_TEXT_LEADING, 2.0 + DEVICE_HEADER_EXTRA_TOP_PADDING),
    NSSize::new(title_width, 18.0),
  );
  let title_label: id = msg_send![class!(NSTextField), alloc];
  let title_label: id = msg_send![title_label, initWithFrame: title_label_frame];
  let _: () = msg_send![title_label, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];
  let _: () = msg_send![title_label, setStringValue: NSString::alloc(nil).init_str(&device_header_label(model))];
  let _: () = msg_send![title_label, setBezeled: NO];
  let _: () = msg_send![title_label, setDrawsBackground: NO];
  let _: () = msg_send![title_label, setEditable: NO];
  let _: () = msg_send![title_label, setSelectable: NO];
  let _: () = msg_send![title_label, setAlignment: 0isize];
  let _: () = msg_send![title_label, setFont: font];
  let _: () = msg_send![title_label, setTextColor: text_color];

  let chevron_x = DEVICE_HEADER_WIDTH - DEVICE_HEADER_TRAILING - DEVICE_HEADER_CHEVRON_SIZE;
  let chevron_y =
    (DEVICE_HEADER_HEIGHT - DEVICE_HEADER_CHEVRON_SIZE) * 0.5 + DEVICE_HEADER_EXTRA_TOP_PADDING;
  let chevron_frame = NSRect::new(
    NSPoint::new(chevron_x, chevron_y),
    NSSize::new(DEVICE_HEADER_CHEVRON_SIZE, DEVICE_HEADER_CHEVRON_SIZE),
  );
  let chevron_view: id = msg_send![class!(NSImageView), alloc];
  let chevron_view: id = msg_send![chevron_view, initWithFrame: chevron_frame];
  let _: () = msg_send![chevron_view, setAutoresizingMask: NS_VIEW_MIN_X_MARGIN];
  let _: () = msg_send![chevron_view, setImage: device_header_chevron_image(model)];
  let chevron_tint: id = msg_send![class!(NSColor), secondaryLabelColor];
  set_image_view_tint_if_supported(chevron_view, chevron_tint);

  let _: () = msg_send![view, addSubview: highlight_view];
  let _: () = msg_send![view, addSubview: title_label];
  let _: () = msg_send![view, addSubview: chevron_view];
  // Add the button last so the entire row is a single click target.
  let _: () = msg_send![view, addSubview: button];
  let _: () = msg_send![item, setView: view];

  DEVICE_HEADER_VIEW_REF.with(|slot| {
    slot.borrow_mut().replace(view);
  });
  DEVICE_HEADER_BUTTON_REF.with(|slot| {
    slot.borrow_mut().replace(button);
  });
  DEVICE_HEADER_HIGHLIGHT_REF.with(|slot| {
    slot.borrow_mut().replace(highlight_view);
  });
  DEVICE_HEADER_LABEL_REF.with(|slot| {
    slot.borrow_mut().replace(title_label);
  });
  DEVICE_HEADER_CHEVRON_REF.with(|slot| {
    slot.borrow_mut().replace(chevron_view);
  });

  item
}

fn set_device_header_title(model: &DeviceMenuModel) {
  unsafe {
    DEVICE_HEADER_LABEL_REF.with(|slot| {
      if let Some(label) = *slot.borrow() {
        let text = NSString::alloc(nil).init_str(&device_header_label(model));
        let _: () = msg_send![label, setStringValue: text];
      }
    });
    DEVICE_HEADER_CHEVRON_REF.with(|slot| {
      if let Some(image_view) = *slot.borrow() {
        let _: () = msg_send![image_view, setImage: device_header_chevron_image(model)];
      }
    });
  }
}

fn clear_device_rows(menu: id) {
  unsafe {
    let count: i64 = msg_send![menu, numberOfItems];
    // Keep [Listen, Separator, Header, Separator, Settings..., Quit].
    if count >= 7 {
      for idx in (3..=(count - 4)).rev() {
        let _: () = msg_send![menu, removeItemAtIndex: idx];
      }
    }
  }

  DEVICE_ROW_IDS.with(|rows| rows.borrow_mut().clear());
}

unsafe fn insert_device_rows(menu: id, delegate: id, model: &DeviceMenuModel, mut insert_at: i64) {
  if !model.expanded {
    return;
  }

  if model.rows.is_empty() {
    let placeholder_item = NSMenuItem::alloc(nil)
      .initWithTitle_action_keyEquivalent_(
        NSString::alloc(nil).init_str("No input devices"),
        sel!(noop:),
        NSString::alloc(nil).init_str(""),
      )
      .autorelease();
    placeholder_item.setTarget_(delegate);
    let _: () = msg_send![placeholder_item, setEnabled: NO];
    let _: () = msg_send![placeholder_item, setIndentationLevel: 1isize];
    let _: () = msg_send![menu, insertItem: placeholder_item atIndex: insert_at];
    return;
  }

  for row in &model.rows {
    let tag = DEVICE_ROW_IDS.with(|rows| {
      let mut rows = rows.borrow_mut();
      rows.push(row.id.clone());
      (rows.len() - 1) as i64
    });

    let row_item = NSMenuItem::alloc(nil)
      .initWithTitle_action_keyEquivalent_(
        NSString::alloc(nil).init_str(&row.label),
        sel!(selectDevice:),
        NSString::alloc(nil).init_str(""),
      )
      .autorelease();
    row_item.setTarget_(delegate);
    let _: () = msg_send![row_item, setTag: tag];
    let state = if row.checked { 1isize } else { 0isize };
    let _: () = msg_send![row_item, setState: state];
    let _: () = msg_send![row_item, setIndentationLevel: 1isize];
    let _: () = msg_send![menu, insertItem: row_item atIndex: insert_at];
    insert_at += 1;
  }
}

unsafe fn ensure_overlay() -> OverlayRefs {
  if let Some(existing) = current_overlay() {
    return existing;
  }

  let refs = create_overlay_window(false);
  OVERLAY_REFS.with(|store| {
    store.borrow_mut().replace(refs);
  });
  refs
}

fn current_overlay() -> Option<OverlayRefs> {
  OVERLAY_REFS.with(|store| *store.borrow())
}

unsafe fn ensure_overlay_top() -> OverlayRefs {
  if let Some(existing) = current_overlay_top() {
    return existing;
  }

  let refs = create_overlay_window(true);
  OVERLAY_TOP_REFS.with(|store| {
    store.borrow_mut().replace(refs);
  });
  refs
}

fn current_overlay_top() -> Option<OverlayRefs> {
  OVERLAY_TOP_REFS.with(|store| *store.borrow())
}

unsafe fn position_overlay_top_relative_to_bottom(top: OverlayRefs, bottom: OverlayRefs) {
  let bottom_frame: NSRect = msg_send![bottom.window, frame];
  if bottom_frame.size.width <= 0.0 || bottom_frame.size.height <= 0.0 {
    return;
  }

  let top_frame: NSRect = msg_send![top.window, frame];
  let screen = overlay_screen_frame_for_window(bottom_frame);
  let width = overlay_width_for_screen(screen);
  let height = if top_frame.size.height > 0.0 {
    top_frame.size.height.clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX)
  } else {
    OVERLAY_HEIGHT_MIN
  };

  let mut x = bottom_frame.origin.x;
  let mut y = bottom_frame.origin.y + bottom_frame.size.height + OVERLAY_STACK_GAP;
  let max_x = (screen.origin.x + screen.size.width - width).max(screen.origin.x);
  let max_y = (screen.origin.y + screen.size.height - height).max(screen.origin.y);
  x = x.clamp(screen.origin.x, max_x);
  y = y.clamp(screen.origin.y, max_y);

  let target_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
  if (top_frame.origin.x - target_frame.origin.x).abs() > 0.05
    || (top_frame.origin.y - target_frame.origin.y).abs() > 0.05
    || (top_frame.size.width - target_frame.size.width).abs() > 0.05
    || (top_frame.size.height - target_frame.size.height).abs() > 0.05
  {
    let _: () = msg_send![top.window, setFrame: target_frame display: YES];
  }
}

unsafe fn ensure_settings_window() -> SettingsWindowRefs {
  if let Some(existing) = current_settings_window() {
    return existing;
  }

  let refs = create_settings_window();
  SETTINGS_WINDOW_REFS.with(|store| {
    store.borrow_mut().replace(refs);
  });
  refs
}

fn current_settings_window() -> Option<SettingsWindowRefs> {
  SETTINGS_WINDOW_REFS.with(|store| *store.borrow())
}

pub fn settings_window_is_open() -> bool {
  current_settings_window().is_some()
}

/// Live-refresh the Settings → Permissions indicators while the window is open,
/// so they flip as the user grants access in System Settings.
pub fn refresh_settings_permissions(accessibility: PermissionStatus, microphone: PermissionStatus) {
  if let Some(refs) = current_settings_window() {
    unsafe {
      set_permission_status_label(refs.perm_accessibility_status, accessibility);
      set_permission_status_label(refs.perm_microphone_status, microphone);
    }
  }
}

unsafe fn apply_settings_selected_tab(refs: SettingsWindowRefs, tab: SettingsTab) {
  let (general_hidden, models_hidden, permissions_hidden, debug_hidden, connectors_hidden) =
    match tab {
      SettingsTab::General => (NO, YES, YES, YES, YES),
      SettingsTab::Models => (YES, NO, YES, YES, YES),
      SettingsTab::Permissions => (YES, YES, NO, YES, YES),
      SettingsTab::Debug => (YES, YES, YES, NO, YES),
      SettingsTab::Connectors => (YES, YES, YES, YES, NO),
    };
  let _: () = msg_send![refs.general_container, setHidden: general_hidden];
  let _: () = msg_send![refs.models_container, setHidden: models_hidden];
  let _: () = msg_send![refs.permissions_container, setHidden: permissions_hidden];
  let _: () = msg_send![refs.debug_container, setHidden: debug_hidden];
  let _: () = msg_send![refs.connectors_container, setHidden: connectors_hidden];

  if refs.tab_list_view != nil {
    let row = settings_row_for_tab(tab);
    if row >= 0 {
      let selected_row: isize = msg_send![refs.tab_list_view, selectedRow];
      if selected_row != row {
        let selection: id = msg_send![class!(NSIndexSet), indexSetWithIndex: row as usize];
        let _: () = msg_send![
            refs.tab_list_view,
            selectRowIndexes: selection
            byExtendingSelection: NO
        ];
      }
    }
  }
}

unsafe fn apply_settings_view_model(refs: SettingsWindowRefs, model: &SettingsViewModel) {
  set_permission_status_label(refs.perm_accessibility_status, model.accessibility_status);
  set_permission_status_label(refs.perm_microphone_status, model.microphone_status);
  let run_on_startup_state: i64 = if model.run_on_startup_enabled { 1 } else { 0 };
  let _: () = msg_send![refs.run_on_startup_checkbox, setState: run_on_startup_state];

  let _: () = msg_send![
      refs.paste_method_popup,
      selectItemAtIndex: model.paste_method.ui_index()
  ];
  let _: () = msg_send![
      refs.auto_submit_popup,
      selectItemAtIndex: model.auto_submit_mode.ui_index()
  ];
  let _: () = msg_send![
      refs.overlay_position_popup,
      selectItemAtIndex: model.overlay_position.ui_index()
  ];
  let append_trailing_space_state: i64 = if model.append_trailing_space_on_paste { 1 } else { 0 };
  let _: () = msg_send![
      refs.append_trailing_space_checkbox,
      setState: append_trailing_space_state
  ];

  let debug_checkbox_state: i64 = if model.debug_stats_enabled { 1 } else { 0 };
  let _: () = msg_send![refs.debug_checkbox, setState: debug_checkbox_state];

  let metrics = NSString::alloc(nil).init_str(&model.metrics_text);
  let _: () = msg_send![refs.metrics_text_view, setString: metrics];

  apply_removed_words_tags(refs, &model.removed_words);
  apply_connector_rows(refs, &model.connectors);
  apply_models_view_state(refs, model);

  SETTINGS_LAST_MODEL.with(|m| m.borrow_mut().replace(model.clone()));

  let selected_row: isize = msg_send![refs.tab_list_view, selectedRow];
  if selected_row >= 0 {
    apply_settings_selected_tab(refs, settings_tab_from_row(selected_row));
  } else {
    apply_settings_selected_tab(refs, model.selected_tab);
  }
}

unsafe fn apply_removed_words_tags(refs: SettingsWindowRefs, words: &[String]) {
  let container = refs.removed_words_tags_view;
  if container == nil {
    return;
  }

  // Remove all existing tag subviews
  loop {
    let subviews: id = msg_send![container, subviews];
    let count: usize = msg_send![subviews, count];
    if count == 0 {
      break;
    }
    let child: id = msg_send![subviews, objectAtIndex: 0usize];
    let _: () = msg_send![child, removeFromSuperview];
  }

  let mut x = 0.0f64;
  let tag_height = 22.0f64;
  for (i, word) in words.iter().enumerate() {
    let title = format!("{word}  \u{00d7}");
    let title_ns = NSString::alloc(nil).init_str(&title);

    let button: id = msg_send![class!(NSButton), alloc];
    let initial_frame = NSRect::new(NSPoint::new(x, 0.0), NSSize::new(80.0, tag_height));
    let button: id = msg_send![button, initWithFrame: initial_frame];
    let _: () = msg_send![button, setBezelStyle: 1usize];
    let _: () = msg_send![button, setTitle: title_ns];
    let _: () = msg_send![button, setTag: i as isize];
    let _: () = msg_send![button, setAction: sel!(settingsRemoveRemovedWord:)];

    STATUS_DELEGATE_REF.with(|r| {
      if let Some(delegate) = *r.borrow() {
        let _: () = msg_send![button, setTarget: delegate];
      }
    });

    let _: () = msg_send![button, sizeToFit];
    let fitted: NSRect = msg_send![button, frame];
    let _: () = msg_send![button, setFrame: NSRect::new(
      NSPoint::new(x, 0.0),
      NSSize::new(fitted.size.width, tag_height),
    )];
    let _: () = msg_send![container, addSubview: button];
    x += fitted.size.width + 6.0;
  }
}

unsafe fn apply_connector_rows(refs: SettingsWindowRefs, connectors: &[ConnectorRowVM]) {
  let container = refs.connectors_checkboxes_view;
  if container == nil {
    return;
  }

  loop {
    let subviews: id = msg_send![container, subviews];
    let count: usize = msg_send![subviews, count];
    if count == 0 {
      break;
    }
    let child: id = msg_send![subviews, objectAtIndex: 0usize];
    let _: () = msg_send![child, removeFromSuperview];
  }

  let container_frame: NSRect = msg_send![container, frame];
  let row_stride = SETTINGS_CONTROL_HEIGHT + 6.0;
  for (i, c) in connectors.iter().enumerate() {
    let y = container_frame.size.height - (i as f64 + 1.0) * row_stride;
    let frame = NSRect::new(
      NSPoint::new(0.0, y),
      NSSize::new(container_frame.size.width, SETTINGS_CONTROL_HEIGHT),
    );
    let checkbox: id = msg_send![class!(NSButton), alloc];
    let checkbox: id = msg_send![checkbox, initWithFrame: frame];
    let _: () = msg_send![checkbox, setButtonType: 3usize];
    let _: () = msg_send![checkbox, setTitle: NSString::alloc(nil).init_str(&c.display_name)];
    let state: i64 = if c.enabled { 1 } else { 0 };
    let _: () = msg_send![checkbox, setState: state];
    let _: () = msg_send![checkbox, setTag: i as isize];
    let _: () = msg_send![checkbox, setAction: sel!(settingsToggleConnector:)];
    STATUS_DELEGATE_REF.with(|r| {
      if let Some(delegate) = *r.borrow() {
        let _: () = msg_send![checkbox, setTarget: delegate];
      }
    });
    let _: () = msg_send![container, addSubview: checkbox];
  }
}

unsafe fn apply_models_view_state(refs: SettingsWindowRefs, model: &SettingsViewModel) {
  use crate::models::{PackStatus, format_size};

  let (status_text, show_download, show_cancel, show_progress, progress_value) =
    match &model.model_pack_status {
      PackStatus::Ready => ("Installed".to_string(), false, false, false, 0.0),
      PackStatus::NotDownloaded => {
        let label = format!("Not downloaded ({})", model.model_pack_size_label);
        (label, true, false, false, 0.0)
      }
      PackStatus::Incomplete => {
        let label = format!("Incomplete ({})", model.model_pack_size_label);
        (label, true, false, false, 0.0)
      }
      PackStatus::Downloading { progress_pct } => {
        let done = format_size(model.model_download_bytes_done);
        let total = format_size(model.model_download_bytes_total);
        let label = format!("Downloading... {done} / {total} ({progress_pct}%)");
        (label, false, true, true, *progress_pct as f64)
      }
    };

  let status_ns = NSString::alloc(nil).init_str(&status_text);
  let _: () = msg_send![refs.models_status_label, setStringValue: status_ns];

  let _: () =
    msg_send![refs.models_download_button, setHidden: if show_download { NO } else { YES }];
  let _: () = msg_send![refs.models_cancel_button, setHidden: if show_cancel { NO } else { YES }];
  let _: () =
    msg_send![refs.models_progress_indicator, setHidden: if show_progress { NO } else { YES }];

  if show_progress {
    let _: () = msg_send![refs.models_progress_indicator, setDoubleValue: progress_value];
  }
}

/// Lay out the pinned connector chip at the top of the card. Shared by the speech and
/// conversation renderers so the chip is byte-identical in both. Empty `connector_tag`
/// hides the chip.
unsafe fn layout_connector_chip(
  refs: OverlayRefs,
  connector_tag: &str,
  connector_icon: &str,
  height: f64,
  content_width: f64,
) {
  if connector_tag.is_empty() {
    let _: () = msg_send![refs.connector_chip, setHidden: YES];
    let _: () = msg_send![refs.connector_chip_label, setHidden: YES];
    let _: () = msg_send![refs.connector_chip_icon, setHidden: YES];
    return;
  }
  let icon_image = connector_chip_icon_image(connector_icon);
  let has_icon = icon_image != nil;
  let icon_block =
    if has_icon { OVERLAY_CONNECTOR_CHIP_ICON_SIZE + OVERLAY_CONNECTOR_CHIP_ICON_GAP } else { 0.0 };

  let _: () = msg_send![
    refs.connector_chip_label,
    setStringValue: NSString::alloc(nil).init_str(connector_tag)
  ];
  let _: () = msg_send![refs.connector_chip_label, sizeToFit];
  let label_frame: NSRect = msg_send![refs.connector_chip_label, frame];
  let label_h = label_frame.size.height.min(OVERLAY_CONNECTOR_CHIP_HEIGHT);
  let chip_w =
    (OVERLAY_CONNECTOR_CHIP_PAD_X * 2.0 + icon_block + label_frame.size.width).min(content_width);
  let chip_y = height - OVERLAY_PAD_TOP - OVERLAY_CONNECTOR_CHIP_HEIGHT;
  let chip_frame = NSRect::new(
    NSPoint::new(OVERLAY_PAD_X, chip_y),
    NSSize::new(chip_w, OVERLAY_CONNECTOR_CHIP_HEIGHT),
  );
  let _: () = msg_send![refs.connector_chip, setFrame: chip_frame];
  // Unhide the container itself, not just its children: a hidden NSView renders neither
  // its own layer background nor its subviews.
  let _: () = msg_send![refs.connector_chip, setHidden: NO];

  if has_icon {
    let icon_y =
      ((OVERLAY_CONNECTOR_CHIP_HEIGHT - OVERLAY_CONNECTOR_CHIP_ICON_SIZE) * 0.5).max(0.0);
    let icon_frame = NSRect::new(
      NSPoint::new(OVERLAY_CONNECTOR_CHIP_PAD_X, icon_y),
      NSSize::new(OVERLAY_CONNECTOR_CHIP_ICON_SIZE, OVERLAY_CONNECTOR_CHIP_ICON_SIZE),
    );
    let _: () = msg_send![refs.connector_chip_icon, setImage: icon_image];
    let icon_tint = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
    set_image_view_tint_if_supported(refs.connector_chip_icon, icon_tint);
    let _: () = msg_send![refs.connector_chip_icon, setFrame: icon_frame];
    let _: () = msg_send![refs.connector_chip_icon, setHidden: NO];
  } else {
    let _: () = msg_send![refs.connector_chip_icon, setHidden: YES];
  }

  let label_x = OVERLAY_CONNECTOR_CHIP_PAD_X + icon_block;
  let label_y = ((OVERLAY_CONNECTOR_CHIP_HEIGHT - label_h) * 0.5).max(0.0);
  let label_w = (chip_w - label_x - OVERLAY_CONNECTOR_CHIP_PAD_X).max(1.0);
  let inner_label_frame =
    NSRect::new(NSPoint::new(label_x, label_y), NSSize::new(label_w, label_h));
  let _: () = msg_send![refs.connector_chip_label, setFrame: inner_label_frame];
  let _: () = msg_send![refs.connector_chip_label, setHidden: NO];
}

unsafe fn hide_conversation_views(refs: OverlayRefs) {
  for v in
    [refs.conv_query_label, refs.conv_divider, refs.conv_reply_scroll, refs.conv_status_label]
  {
    if v != nil {
      let _: () = msg_send![v, setHidden: YES];
    }
  }
}

/// Like [`fit_rendered_body_for_height`] but keeps the HEAD of the text — drops trailing
/// words and appends an ellipsis. Used for the user's query (short; the beginning matters).
unsafe fn fit_rendered_head_for_height(
  label: id,
  body_text: &str,
  width: f64,
  max_height: f64,
) -> (String, f64) {
  let trimmed = body_text.trim();
  if trimmed.is_empty() {
    return (String::new(), OVERLAY_TEXT_LINE_HEIGHT.min(max_height));
  }
  let measured_full = measure_label_height(label, trimmed, width);
  if measured_full <= max_height + 0.5 {
    return (trimmed.to_string(), measured_full);
  }

  // Byte index just past each word (a non-ws followed by ws, plus the final word).
  let mut word_ends = Vec::new();
  let mut prev_ws = true;
  for (idx, ch) in trimmed.char_indices() {
    let is_ws = ch.is_whitespace();
    if is_ws && !prev_ws {
      word_ends.push(idx);
    }
    prev_ws = is_ws;
  }
  if !prev_ws {
    word_ends.push(trimmed.len());
  }
  if word_ends.is_empty() {
    return (trimmed.to_string(), measured_full);
  }

  let mut lo = 0usize;
  let mut hi = word_ends.len() - 1;
  while lo < hi {
    let mid = (lo + hi + 1) / 2;
    let candidate = format!("{}…", trimmed[..word_ends[mid]].trim_end());
    let measured = measure_label_height(label, &candidate, width);
    if measured <= max_height + 0.5 {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  let rendered = format!("{}…", trimmed[..word_ends[lo]].trim_end());
  let measured = measure_label_height(label, &rendered, width);
  (rendered, measured)
}

unsafe fn render_overlay_conversation(
  refs: OverlayRefs,
  connector_tag: &str,
  connector_icon: &str,
  user_query: &str,
  reply: &str,
  status: ConvStatus,
  error_msg: &str,
  activity: &[f32],
  busy_phase: Option<f32>,
) {
  let current_frame: NSRect = msg_send![refs.window, frame];
  let screen = overlay_screen_frame_for_window(current_frame);
  let width = overlay_width_for_screen(screen);
  let content_width = (width - OVERLAY_PAD_X * 2.0).max(1.0);

  // Hide the speech-mode widgets conversation mode replaces. The wave is reused as a
  // bottom strip (handled below), so meter_view is NOT hidden here.
  let _: () = msg_send![refs.label, setHidden: YES];
  let _: () = msg_send![refs.raw_badge, setHidden: YES];
  let _: () = msg_send![refs.hold_badge, setHidden: YES];
  hide_overlay_notice_accessory(refs);
  if refs.autocomplete_separator != nil {
    let _: () = msg_send![refs.autocomplete_separator, setHidden: YES];
  }

  let chip_reserve = OVERLAY_CONNECTOR_CHIP_HEIGHT + OVERLAY_CONNECTOR_CHIP_GAP;
  let wave_reserve = OVERLAY_CONV_WAVE_HEIGHT + OVERLAY_CONV_WAVE_GAP;
  let max_inner =
    (OVERLAY_HEIGHT_MAX - OVERLAY_PAD_TOP - OVERLAY_PAD_BOTTOM - chip_reserve - wave_reserve)
      .max(1.0);

  let has_query = !user_query.trim().is_empty();
  let (rendered_query, query_h) = if has_query {
    fit_rendered_head_for_height(
      refs.conv_query_label,
      user_query,
      content_width,
      OVERLAY_CONV_QUERY_MAX_HEIGHT.min(max_inner),
    )
  } else {
    (String::new(), 0.0)
  };

  let show_reply =
    matches!(status, ConvStatus::Streaming | ConvStatus::Done) && !reply.trim().is_empty();
  let status_text = match status {
    ConvStatus::Thinking => "Thinking…".to_string(),
    ConvStatus::Error => {
      if error_msg.is_empty() {
        "Gateway error.".to_string()
      } else {
        error_msg.to_string()
      }
    }
    _ => String::new(),
  };
  let show_status = !status_text.is_empty();

  let divider_block = if has_query && (show_reply || show_status) {
    OVERLAY_CONV_DIVIDER_GAP + OVERLAY_CONV_DIVIDER_THICKNESS + OVERLAY_CONV_DIVIDER_GAP
  } else {
    0.0
  };

  let lower_budget = (max_inner - query_h - divider_block).max(OVERLAY_TEXT_LINE_HEIGHT);
  let mut reply_changed = false;
  let reply_h = if show_reply {
    // Update the scrolling text view only when the reply text actually changed, so idle
    // re-renders (the activity wave, meter ticks) don't reset the user's scroll position.
    let new_reply = NSString::alloc(nil).init_str(reply);
    let cur: id = msg_send![refs.conv_reply_text, string];
    let same = if cur != nil {
      let eq: bool = msg_send![cur, isEqualToString: new_reply];
      eq
    } else {
      false
    };
    if !same {
      let _: () = msg_send![refs.conv_reply_text, setString: new_reply];
      let reply_font: id = msg_send![class!(NSFont), systemFontOfSize: OVERLAY_TEXT_FONT_SIZE];
      let reply_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
        nil,
        CLAUDE_BRAND_R,
        CLAUDE_BRAND_G,
        CLAUDE_BRAND_B,
        1.0,
      );
      let _: () = msg_send![refs.conv_reply_text, setFont: reply_font];
      let _: () = msg_send![refs.conv_reply_text, setTextColor: reply_color];
      reply_changed = true;
    }
    // The card grows with the reply up to the budget; beyond that the text view scrolls.
    measure_textview_height(refs.conv_reply_text, content_width).min(lower_budget)
  } else {
    0.0
  };
  let (rendered_status, status_h) = if show_status {
    let budget = lower_budget.min(OVERLAY_TEXT_LINE_HEIGHT * 2.0);
    let measured =
      measure_label_height(refs.conv_status_label, &status_text, content_width).min(budget);
    (status_text.clone(), measured.max(OVERLAY_TEXT_LINE_HEIGHT.min(budget)))
  } else {
    (String::new(), 0.0)
  };

  let lower_h = reply_h.max(status_h);
  let inner_used = query_h + divider_block + lower_h;
  let content_height =
    OVERLAY_PAD_TOP + chip_reserve + inner_used + wave_reserve + OVERLAY_PAD_BOTTOM;
  let height = content_height.clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX);

  let default_x = screen.origin.x + (screen.size.width - width) * 0.5;
  let default_y = screen.origin.y + screen.size.height * 0.08;
  let x = if current_frame.size.width <= 0.0 { default_x } else { current_frame.origin.x };
  let y = if current_frame.size.height <= 0.0 { default_y } else { current_frame.origin.y };
  let overlay_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
  if (current_frame.origin.x - overlay_frame.origin.x).abs() > 0.05
    || (current_frame.origin.y - overlay_frame.origin.y).abs() > 0.05
    || (current_frame.size.width - overlay_frame.size.width).abs() > 0.05
    || (current_frame.size.height - overlay_frame.size.height).abs() > 0.05
  {
    let _: () = msg_send![refs.window, setFrame: overlay_frame display: YES];
  }

  let card_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
  let _: () = msg_send![refs.card_view, setFrame: card_frame];
  let card_layer: id = msg_send![refs.card_view, layer];
  if card_layer != nil {
    let card_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.02, 0.02, 0.02, 0.90);
    let card_cg: id = msg_send![card_color, CGColor];
    let _: () = msg_send![card_layer, setBackgroundColor: card_cg];
  }

  apply_busy_border_style(refs, busy_phase, width, height);
  layout_connector_chip(refs, connector_tag, connector_icon, height, content_width);

  // Stacked blocks, bottom-left origin (top -> bottom = high y -> low y).
  let chip_top = height - OVERLAY_PAD_TOP - OVERLAY_CONNECTOR_CHIP_HEIGHT;
  let query_top = chip_top - OVERLAY_CONNECTOR_CHIP_GAP;

  if has_query {
    let query_y = query_top - query_h;
    let query_frame =
      NSRect::new(NSPoint::new(OVERLAY_PAD_X, query_y), NSSize::new(content_width, query_h));
    let _: () = msg_send![refs.conv_query_label, setFrame: query_frame];
    let _: () = msg_send![refs.conv_query_label, setStringValue: NSString::alloc(nil).init_str(&rendered_query)];
    let _: () = msg_send![refs.conv_query_label, setHidden: NO];
  } else {
    let _: () = msg_send![refs.conv_query_label, setHidden: YES];
  }

  let lower_top = if divider_block > 0.0 {
    let divider_y = query_top - query_h - OVERLAY_CONV_DIVIDER_GAP - OVERLAY_CONV_DIVIDER_THICKNESS;
    let divider_frame = NSRect::new(
      NSPoint::new(OVERLAY_PAD_X, divider_y),
      NSSize::new(content_width, OVERLAY_CONV_DIVIDER_THICKNESS),
    );
    let _: () = msg_send![refs.conv_divider, setFrame: divider_frame];
    let _: () = msg_send![refs.conv_divider, setHidden: NO];
    divider_y - OVERLAY_CONV_DIVIDER_GAP
  } else {
    let _: () = msg_send![refs.conv_divider, setHidden: YES];
    query_top - query_h
  };

  if show_reply {
    let reply_y = (lower_top - reply_h).max(OVERLAY_PAD_BOTTOM + wave_reserve);
    let reply_frame =
      NSRect::new(NSPoint::new(OVERLAY_PAD_X, reply_y), NSSize::new(content_width, reply_h));
    let _: () = msg_send![refs.conv_reply_scroll, setFrame: reply_frame];
    let _: () = msg_send![refs.conv_reply_scroll, setHidden: NO];
    if reply_changed {
      // Follow the streaming tail; unchanged re-renders leave the scroll position be, so
      // the user can scroll back up to read earlier text once the reply settles.
      let _: () = msg_send![refs.conv_reply_text, scrollToEndOfDocument: nil];
    }
  } else {
    let _: () = msg_send![refs.conv_reply_scroll, setHidden: YES];
  }

  if show_status {
    let color = if matches!(status, ConvStatus::Error) {
      NSColor::colorWithCalibratedRed_green_blue_alpha_(
        nil,
        OVERLAY_CONV_ERROR_R,
        OVERLAY_CONV_ERROR_G,
        OVERLAY_CONV_ERROR_B,
        0.95,
      )
    } else {
      NSColor::colorWithCalibratedRed_green_blue_alpha_(
        nil,
        CLAUDE_BRAND_R,
        CLAUDE_BRAND_G,
        CLAUDE_BRAND_B,
        0.95,
      )
    };
    let _: () = msg_send![refs.conv_status_label, setTextColor: color];
    let status_y = (lower_top - status_h).max(OVERLAY_PAD_BOTTOM + wave_reserve);
    let status_frame =
      NSRect::new(NSPoint::new(OVERLAY_PAD_X, status_y), NSSize::new(content_width, status_h));
    let _: () = msg_send![refs.conv_status_label, setFrame: status_frame];
    let _: () = msg_send![
      refs.conv_status_label,
      setStringValue: NSString::alloc(nil).init_str(&rendered_status)
    ];
    let _: () = msg_send![refs.conv_status_label, setHidden: NO];
  } else {
    let _: () = msg_send![refs.conv_status_label, setHidden: YES];
  }

  // Voice-activity strip pinned at the bottom.
  let meter_frame = NSRect::new(
    NSPoint::new(OVERLAY_PAD_X, OVERLAY_PAD_BOTTOM),
    NSSize::new(content_width, OVERLAY_CONV_WAVE_HEIGHT),
  );
  let _: () = msg_send![refs.meter_view, setFrame: meter_frame];
  let _: () = msg_send![refs.meter_view, setHidden: NO];
  render_activity_wave(refs, activity, meter_frame.size.width, meter_frame.size.height);
}

unsafe fn render_overlay_text(
  refs: OverlayRefs,
  body_text: &str,
  activity: &[f32],
  busy_phase: Option<f32>,
  show_raw_badge: bool,
  show_hold_badge: bool,
  connector_tag: &str,
  connector_icon: &str,
) {
  let current_frame: NSRect = msg_send![refs.window, frame];
  let screen = overlay_screen_frame_for_window(current_frame);
  let width = overlay_width_for_screen(screen);
  let content_width = (width - OVERLAY_PAD_X * 2.0).max(1.0);

  // Space reserved at the top of the card for the connector chip, folded into the
  // body height budget so the transcription drops below it.
  let show_chip = !connector_tag.is_empty();
  let chip_reserve =
    if show_chip { OVERLAY_CONNECTOR_CHIP_HEIGHT + OVERLAY_CONNECTOR_CHIP_GAP } else { 0.0 };

  let max_body_height =
    (OVERLAY_HEIGHT_MAX - OVERLAY_PAD_TOP - OVERLAY_PAD_BOTTOM - chip_reserve).max(1.0);

  let (rendered_body, mut measured_body_height) =
    fit_rendered_body_for_height(refs.label, body_text, content_width, max_body_height);
  if rendered_body.is_empty() {
    measured_body_height = OVERLAY_TEXT_LINE_HEIGHT.min(max_body_height);
  }
  let body_height = measured_body_height
    .max(OVERLAY_TEXT_LINE_HEIGHT.min(max_body_height))
    .min(max_body_height);
  let is_single_line = rendered_body.is_empty()
    || (!rendered_body.contains('\n') && body_height <= OVERLAY_TEXT_LINE_HEIGHT * 1.35);
  let content_height = OVERLAY_PAD_TOP + chip_reserve + body_height + OVERLAY_PAD_BOTTOM;
  let height = content_height.clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX);

  let default_x = screen.origin.x + (screen.size.width - width) * 0.5;
  let default_y = screen.origin.y + screen.size.height * 0.08;
  let x = if current_frame.size.width <= 0.0 { default_x } else { current_frame.origin.x };
  let y = if current_frame.size.height <= 0.0 { default_y } else { current_frame.origin.y };
  let overlay_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
  if (current_frame.origin.x - overlay_frame.origin.x).abs() > 0.05
    || (current_frame.origin.y - overlay_frame.origin.y).abs() > 0.05
    || (current_frame.size.width - overlay_frame.size.width).abs() > 0.05
    || (current_frame.size.height - overlay_frame.size.height).abs() > 0.05
  {
    let _: () = msg_send![refs.window, setFrame: overlay_frame display: YES];
  }

  let card_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
  let _: () = msg_send![refs.card_view, setFrame: card_frame];
  let card_layer: id = msg_send![refs.card_view, layer];
  if card_layer != nil {
    let card_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.02, 0.02, 0.02, 0.90);
    let card_cg: id = msg_send![card_color, CGColor];
    let _: () = msg_send![card_layer, setBackgroundColor: card_cg];
  }
  let default_text = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
  let _: () = msg_send![refs.label, setTextColor: default_text];
  // Restore speech-mode label/meter visibility and styling. The history-list
  // renderer hides and re-styles `refs.label` (smaller font, left-aligned,
  // truncating line break) and `refs.meter_view`; without this restore, a turn
  // back to speech mode after using history would render an invisible draft and
  // an invisible meter — the text is captured but the user sees nothing.
  let _: () = msg_send![refs.label, setHidden: NO];
  let _: () = msg_send![refs.meter_view, setHidden: NO];
  let label_font: id = msg_send![class!(NSFont), systemFontOfSize: OVERLAY_TEXT_FONT_SIZE];
  if label_font != nil {
    let _: () = msg_send![refs.label, setFont: label_font];
  }
  let _: () = msg_send![refs.label, setUsesSingleLineMode: NO];
  let _: () = msg_send![refs.label, setLineBreakMode: 0isize];
  let _: () = msg_send![refs.label, setMaximumNumberOfLines: 0isize];

  apply_busy_border_style(refs, busy_phase, width, height);

  let available_height = (height - OVERLAY_PAD_TOP - OVERLAY_PAD_BOTTOM - chip_reserve).max(1.0);
  let body_text_height = body_height.min(available_height).max(1.0);
  let body_y = if is_single_line {
    OVERLAY_PAD_BOTTOM + ((available_height - body_text_height) * 0.5).max(0.0)
  } else {
    OVERLAY_PAD_BOTTOM
  };
  let meter_height = body_text_height.max(OVERLAY_WAVE_BG_HEIGHT).min(available_height).max(1.0);
  let meter_y = OVERLAY_PAD_BOTTOM;
  let body_frame =
    NSRect::new(NSPoint::new(OVERLAY_PAD_X, body_y), NSSize::new(content_width, body_text_height));
  let meter_frame =
    NSRect::new(NSPoint::new(OVERLAY_PAD_X, meter_y), NSSize::new(content_width, meter_height));
  let _: () = msg_send![refs.label, setFrame: body_frame];
  let _: () = msg_send![refs.meter_view, setFrame: meter_frame];

  layout_connector_chip(refs, connector_tag, connector_icon, height, content_width);

  let mut badge_right = (width - OVERLAY_RAW_BADGE_RIGHT_INSET).max(OVERLAY_RAW_BADGE_RIGHT_INSET);
  if show_raw_badge {
    let raw_badge_x = (badge_right - OVERLAY_RAW_BADGE_WIDTH).max(OVERLAY_RAW_BADGE_RIGHT_INSET);
    let raw_badge_frame = NSRect::new(
      NSPoint::new(raw_badge_x, OVERLAY_RAW_BADGE_BOTTOM_INSET),
      NSSize::new(OVERLAY_RAW_BADGE_WIDTH, OVERLAY_RAW_BADGE_HEIGHT),
    );
    let _: () = msg_send![refs.raw_badge, setFrame: raw_badge_frame];
    let _: () = msg_send![refs.raw_badge, setHidden: NO];
    badge_right = raw_badge_x - OVERLAY_BADGE_GAP;
  } else {
    let _: () = msg_send![refs.raw_badge, setHidden: YES];
  }
  if show_hold_badge {
    let hold_badge_x = (badge_right - OVERLAY_HOLD_BADGE_WIDTH).max(OVERLAY_RAW_BADGE_RIGHT_INSET);
    let hold_badge_frame = NSRect::new(
      NSPoint::new(hold_badge_x, OVERLAY_RAW_BADGE_BOTTOM_INSET),
      NSSize::new(OVERLAY_HOLD_BADGE_WIDTH, OVERLAY_HOLD_BADGE_HEIGHT),
    );
    let _: () = msg_send![refs.hold_badge, setFrame: hold_badge_frame];
    let _: () = msg_send![refs.hold_badge, setHidden: NO];
  } else {
    let _: () = msg_send![refs.hold_badge, setHidden: YES];
  }

  // Hide autocomplete views (no longer used)
  if refs.autocomplete_separator != nil {
    let _: () = msg_send![refs.autocomplete_separator, setHidden: YES];
  }
  for i in 0..AUTOCOMPLETE_MAX_ITEMS {
    if refs.autocomplete_labels[i] != nil {
      let _: () = msg_send![refs.autocomplete_labels[i], setHidden: YES];
    }
    if refs.autocomplete_bgs[i] != nil {
      let _: () = msg_send![refs.autocomplete_bgs[i], setHidden: YES];
    }
    if refs.autocomplete_expand_markers[i] != nil {
      let _: () = msg_send![refs.autocomplete_expand_markers[i], setHidden: YES];
    }
    if refs.autocomplete_ts_labels[i] != nil {
      let _: () = msg_send![refs.autocomplete_ts_labels[i], setHidden: YES];
    }
    if refs.autocomplete_char_count_labels[i] != nil {
      let _: () = msg_send![refs.autocomplete_char_count_labels[i], setHidden: YES];
    }
  }
  // History-mode search bar widgets — hidden whenever we're not in history mode.
  if refs.search_field != nil {
    let _: () = msg_send![refs.search_field, setHidden: YES];
  }
  if refs.search_icon != nil {
    let _: () = msg_send![refs.search_icon, setHidden: YES];
  }
  if refs.search_caret != nil {
    let _: () = msg_send![refs.search_caret, setHidden: YES];
  }
  // Hide conversation-mode views so a speech render after a gateway turn shows no stale
  // query/divider/reply. (One block covers all four `render_overlay_text` callers.)
  hide_conversation_views(refs);

  let _: () = msg_send![refs.label, setAlignment: 1isize];
  let _: () = msg_send![refs.label, setStringValue: NSString::alloc(nil).init_str(&rendered_body)];
  hide_overlay_notice_accessory(refs);
  render_activity_wave(refs, activity, meter_frame.size.width, meter_frame.size.height);
}

unsafe fn hide_overlay_notice_accessory(refs: OverlayRefs) {
  if refs.notice_accessory_row != nil {
    let _: () = msg_send![refs.notice_accessory_row, setHidden: YES];
  }
  for view in [refs.notice_option_key, refs.notice_space_key, refs.notice_auto_on_chip] {
    if view != nil {
      let _: () = msg_send![view, setHidden: YES];
    }
  }
  for label in [
    refs.notice_option_label,
    refs.notice_plus_label,
    refs.notice_space_label,
    refs.notice_auto_on_label,
  ] {
    if label != nil {
      let _: () = msg_send![label, setHidden: YES];
    }
  }
}

const HISTORY_BODY_FONT_SIZE: f64 = 14.0;
const HISTORY_BODY_LINE_HEIGHT: f64 = 18.0;
const HISTORY_BODY_MAX_LINES: usize = 2;
const HISTORY_ROW_PAD_Y: f64 = 6.0;
const HISTORY_ROW_GAP: f64 = 2.0;
const HISTORY_BG_X_INSET: f64 = 4.0;
// `OVERLAY_CARD_RADIUS - 8` so the inner highlight follows the outer card's
// rounded curve concentrically (the bg is inset 8 pt from the card edge:
// `OVERLAY_PAD_X (12) - HISTORY_BG_X_INSET (4) = 8`). Smaller values leave a
// visible gap between the highlight corner and the card corner; larger values
// clip past the curve.
const HISTORY_BG_RADIUS: f64 = 14.0;
// Symmetric padding inside the highlight bg between its left/right edges and
// the entry text. Keeps the text from butting against the rounded bg corner.
const HISTORY_TEXT_INNER_PAD_X: f64 = 12.0;
// Deep blue (#002EA2) at high alpha so the selected entry pops without washing
// out against the dark card.
const HISTORY_SELECTED_BG_R: f64 = 0.0;
const HISTORY_SELECTED_BG_G: f64 = 0.180;
const HISTORY_SELECTED_BG_B: f64 = 0.635;
const HISTORY_SELECTED_BG_ALPHA: f64 = 0.85;
// All entries (selected and unselected) render in the same bright off-white.
const HISTORY_TEXT_ALPHA: f64 = 0.95;
const HISTORY_EMPTY_TEXT_ALPHA: f64 = 0.40;
// Bottom-right hint labels were dropped in favour of in-row "▶" expand
// markers and a fixed search bar. Constants removed.
// Maximum slot height (used to size the fixed total card height). Real rows
// pack at their measured natural height (1-line short, 2-line tall) so single-
// line entries don't claim 2-line space.
const HISTORY_ROW_SLOT_HEIGHT_MAX: f64 =
  HISTORY_BODY_LINE_HEIGHT * HISTORY_BODY_MAX_LINES as f64 + 2.0 * HISTORY_ROW_PAD_Y;
// Card vertical budget targets 5 max-height (two-line) rows. When entries are
// shorter, the renderer greedy-fits more rows into the same fixed budget so
// the card never has unused space at the top.
const HISTORY_HEIGHT_TARGET_ROWS: f64 = 5.0;
const HISTORY_LIST_HEIGHT: f64 = OVERLAY_PAD_TOP
  + HISTORY_HEIGHT_TARGET_ROWS * HISTORY_ROW_SLOT_HEIGHT_MAX
  + (HISTORY_HEIGHT_TARGET_ROWS - 1.0) * HISTORY_ROW_GAP
  + OVERLAY_PAD_BOTTOM;

// Search bar at the bottom of the history overlay. Sized like a single-line
// row so it visually mirrors the entries above. The card grows vertically to
// host both the list area (HISTORY_LIST_HEIGHT) and the search bar.
const SEARCH_BAR_HEIGHT: f64 = 30.0;
const SEARCH_BAR_GAP: f64 = 6.0;
// Custom blinking caret that stands in for the missing native NSTextField
// caret. Drawn 2 pt wide so it reads as a confident insertion bar at any
// scale; height matches the body font's cap height roughly.
const SEARCH_CARET_WIDTH: f64 = 2.0;
const SEARCH_CARET_HEIGHT: f64 = 19.0;
// Inner field height — slightly larger than the body font's line height so
// the typed glyphs have breathing room. Reused for vertical centring inside
// `SEARCH_BAR_HEIGHT`.
const SEARCH_FIELD_HEIGHT: f64 = 20.0;
// macOS standard caret blink: 1.06 s period, 50% duty cycle.
const SEARCH_CARET_BLINK_HALF_PERIOD: f64 = 0.53;
// Search-field font intentionally matches `HISTORY_BODY_FONT_SIZE` so the
// typed query feels visually continuous with the row text above it.
// Card-local y of the search bar's bottom edge.
const SEARCH_BAR_Y: f64 = OVERLAY_PAD_BOTTOM;
// Card-local y where the row stack starts (just above the search bar + gap).
const LIST_BASE_Y: f64 = OVERLAY_PAD_BOTTOM + SEARCH_BAR_HEIGHT + SEARCH_BAR_GAP;
// Total card height: bottom pad → search bar → gap → list area.
const HISTORY_CARD_HEIGHT: f64 = HISTORY_LIST_HEIGHT + SEARCH_BAR_HEIGHT + SEARCH_BAR_GAP;
// Translucent yellow used to highlight the matched substring in each row.
const SEARCH_MATCH_BG_R: f64 = 1.0;
const SEARCH_MATCH_BG_G: f64 = 0.85;
const SEARCH_MATCH_BG_B: f64 = 0.20;
const SEARCH_MATCH_BG_ALPHA: f64 = 0.45;

// Per-row right-meta column. Three items right-aligned in a 26-pt-wide
// column inside the highlight bg, stacked top → bottom: char-count,
// time-ago, expand marker. All three use the same small font so the
// full stack fits inside a 1-line row's 30-pt-tall highlight bg —
// 9 + 1 + 9 + 1 + 9 = 29 pt with 1 pt of safety margin.
//
// Char-count format is from `transcript_history::format_char_count_compact`
// (always ≤ 4 chars: "47", "1.2k", "12k", "999k"); time-ago format is from
// `format_timestamp_compact` (always ≤ 4 chars: "5s", "12m", "1h", "999d").
// The arrow is the literal U+25B6 glyph.
const HISTORY_TS_FONT_SIZE: f64 = 8.0;
const HISTORY_TS_TEXT_ALPHA: f64 = 0.45;
const HISTORY_TS_WIDTH: f64 = 26.0;
const HISTORY_TS_GAP: f64 = 6.0;

// Char-count mirrors the time-ago font size + width so the stack reads as
// uniform siblings (visual hierarchy by position, not weight). Kept as
// named consts (rather than inlining HISTORY_TS_*) to keep
// "differentiate them later" a one-line change.
const HISTORY_CHAR_COUNT_FONT_SIZE: f64 = HISTORY_TS_FONT_SIZE;
const HISTORY_CHAR_COUNT_WIDTH: f64 = HISTORY_TS_WIDTH;
// Per-label vertical padding (label height = font + this). Tighter than the
// previous +4 so the three-item stack fits inside a 30-pt bg.
const HISTORY_META_LABEL_VPAD: f64 = 1.0;
// Vertical gap between adjacent meta items in the stack.
const HISTORY_META_STACK_GAP: f64 = 1.0;

// Per-row "▶" expand marker, anchored at the bottom of the right-meta
// stack when the row's body is truncated. Sized to match the time-ago +
// char-count font so the three-item stack fits inside a 1-line row's bg.
const HISTORY_EXPAND_MARKER_FONT_SIZE: f64 = HISTORY_TS_FONT_SIZE;
const HISTORY_EXPAND_MARKER_ALPHA: f64 = 0.55;
const HISTORY_EXPAND_MARKER_WIDTH: f64 = 14.0;

// Atomics that the renderer writes after every history-list draw so the app
// side (which initiates the next render via Up/Down/Right) can make decisions
// based on the actual fitted state — number of visible rows, the (clamped)
// top-of-window entry index, and whether the selected entry was truncated.
// Reads must happen between renders; writes happen during render.
static LAST_HISTORY_VISIBLE_COUNT: AtomicUsize = AtomicUsize::new(0);
static LAST_HISTORY_VISIBLE_START: AtomicUsize = AtomicUsize::new(0);
static LAST_HISTORY_SELECTED_TRUNCATED: AtomicBool = AtomicBool::new(false);

pub fn last_history_visible_count() -> usize {
  LAST_HISTORY_VISIBLE_COUNT.load(Ordering::Relaxed)
}

pub fn last_history_visible_start() -> usize {
  LAST_HISTORY_VISIBLE_START.load(Ordering::Relaxed)
}

pub fn last_history_selected_truncated() -> bool {
  LAST_HISTORY_SELECTED_TRUNCATED.load(Ordering::Relaxed)
}

unsafe fn render_overlay_history_list(
  refs: OverlayRefs,
  entries: &[HistoryEntryView<'_>],
  selected_index: usize,
  visible_start: usize,
  expanded: bool,
) {
  // Hide every speech-mode widget. The history list is the entire body.
  // Note: `refs.label` stays visible — it gets repurposed at the bottom of
  // this function as the "← esc" hint in the bottom-right corner.
  let _: () = msg_send![refs.meter_view, setHidden: YES];
  let _: () = msg_send![refs.raw_badge, setHidden: YES];
  let _: () = msg_send![refs.hold_badge, setHidden: YES];
  let _: () = msg_send![refs.connector_chip, setHidden: YES];
  let _: () = msg_send![refs.connector_chip_label, setHidden: YES];
  let _: () = msg_send![refs.connector_chip_icon, setHidden: YES];
  hide_conversation_views(refs);
  // Hide the busy gradient (the rotating border-pulse) but leave the mask layer
  // alone — `apply_busy_border_style` doesn't always re-show the mask, and a
  // hidden mask clips the gradient to fully-transparent alpha (the missing-
  // border-pulse bug). Hiding only the gradient is enough; the mask staying
  // visible doesn't render anything on its own.
  if refs.busy_gradient_layer != nil {
    let _: () = msg_send![refs.busy_gradient_layer, setHidden: YES];
  }
  if refs.autocomplete_separator != nil {
    let _: () = msg_send![refs.autocomplete_separator, setHidden: YES];
  }
  hide_overlay_notice_accessory(refs);

  let current_frame: NSRect = msg_send![refs.window, frame];
  let screen = overlay_screen_frame_for_window(current_frame);
  let width = overlay_width_for_screen(screen);

  // Empty-state: centered, dimmed "No transcripts" / "No matches" message
  // inside the list area, with the search bar still visible at the bottom so
  // the user can correct or clear the query that produced the empty result.
  if entries.is_empty() {
    if overlay_debug_logs_enabled() {
      eprintln!("OVERLAY_HISTORY_LIST entries=0 action=empty_state");
    }
    for i in 1..AUTOCOMPLETE_MAX_ITEMS {
      let _: () = msg_send![refs.autocomplete_labels[i], setHidden: YES];
    }
    for i in 0..AUTOCOMPLETE_MAX_ITEMS {
      let _: () = msg_send![refs.autocomplete_bgs[i], setHidden: YES];
      if refs.autocomplete_expand_markers[i] != nil {
        let _: () = msg_send![refs.autocomplete_expand_markers[i], setHidden: YES];
      }
      if refs.autocomplete_ts_labels[i] != nil {
        let _: () = msg_send![refs.autocomplete_ts_labels[i], setHidden: YES];
      }
      if refs.autocomplete_char_count_labels[i] != nil {
        let _: () = msg_send![refs.autocomplete_char_count_labels[i], setHidden: YES];
      }
    }
    let label = refs.autocomplete_labels[0];
    if label != nil {
      let empty_content_width = (width - OVERLAY_PAD_X * 2.0).max(1.0);
      configure_history_body_label(label);
      let _: () = msg_send![label, setUsesSingleLineMode: YES];
      let _: () = msg_send![label, setAlignment: 1isize]; // NSTextAlignmentCenter
      let color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
        nil,
        1.0,
        1.0,
        1.0,
        HISTORY_EMPTY_TEXT_ALPHA,
      );
      let _: () = msg_send![label, setTextColor: color];
      // Pick the message based on whether the empty result is from a search
      // miss vs. a truly empty index. The search field's current value is
      // the source of truth.
      let query_active = if refs.search_field == nil {
        false
      } else {
        let s: id = msg_send![refs.search_field, stringValue];
        if s == nil {
          false
        } else {
          let len: usize = msg_send![s, length];
          len > 0
        }
      };
      let msg = if query_active { "No matches" } else { "No transcripts" };
      let text = NSString::alloc(nil).init_str(msg);
      let _: () = msg_send![label, setStringValue: text];
      // Sit the message at the bottom of the (now-shrunken) list area,
      // just above the search bar.
      let label_y = LIST_BASE_Y + HISTORY_ROW_PAD_Y;
      let label_frame = NSRect::new(
        NSPoint::new(OVERLAY_PAD_X, label_y),
        NSSize::new(empty_content_width, HISTORY_BODY_LINE_HEIGHT),
      );
      let _: () = msg_send![label, setFrame: label_frame];
      let _: () = msg_send![label, setHidden: NO];
    }
    // Hint label still shows so the user knows Esc dismisses; "view ▶" stays
    // hidden because there's no entry to expand.
    // Empty state: shrink to just the search bar + a one-line message.
    // The dynamic-height rule below also applies here so the card
    // doesn't carry empty space above the message.
    let empty_height = LIST_BASE_Y + HISTORY_BODY_LINE_HEIGHT + OVERLAY_PAD_TOP;
    layout_history_search_bar(refs, width);
    layout_history_hints(refs, width, false, expanded);
    apply_history_window_frame(refs, current_frame, screen, width, empty_height);
    apply_history_card_frame(refs, width, empty_height);
    return;
  }

  // Bg geometry: highlight is concentric with the card's rounded corners
  // (8 pt inset from each side). Time-ago labels live INSIDE the highlight
  // on the right; "▶" expand marker also lives inside on the right but
  // the timestamp gets the far-right slot (top-aligned, above the body
  // line) and the ▶ sits to its left (centered on the body line).
  let bg_x = (OVERLAY_PAD_X - HISTORY_BG_X_INSET).max(2.0);
  let bg_w_full = (width - 2.0 * bg_x).max(1.0);
  // Text width: symmetric inner padding inside the bg.
  let label_x = bg_x + HISTORY_TEXT_INNER_PAD_X;
  let label_w_full = (bg_w_full - 2.0 * HISTORY_TEXT_INNER_PAD_X).max(1.0);

  // The card is bottom-anchored when the user is at the newest (visible_start
  // == 0) and top-anchored once they've scrolled past it: the most recently
  // scrolled-to entry sits flush with the card top so each subsequent Up
  // delivers a new top-flush entry.
  //
  // Every row reserves a thin strip on the right of its body label for
  // the time-ago timestamp; the "▶" expand marker stacks underneath the
  // timestamp in the same column so we only reserve the wider one
  // (timestamp). Widths stay consistent whether or not the marker draws.
  let is_bottom_anchored = visible_start == 0;
  let body_right_reserve = HISTORY_TS_WIDTH + HISTORY_TS_GAP;
  let label_w_full = (label_w_full - body_right_reserve).max(1.0);
  let label_w_bottom = label_w_full;

  // Greedy fit: starting from `visible_start` (or selected_index, whichever is
  // smaller — the selected entry must always be inside the window), pack rows
  // upward and add another whenever there's still at least one body line of
  // visible card height remaining. Rows are allowed to extend into the top
  // pad zone (clipped by the card's masksToBounds at the rounded edge), so
  // the card always fills bottom-to-top regardless of whether the visible mix
  // is 5 two-line rows or 8 single-line rows.
  //
  // If the fitted window doesn't include the selected entry, slide `start`
  // upward by 1 and refit until it does.
  let body_max_height = HISTORY_BODY_LINE_HEIGHT * HISTORY_BODY_MAX_LINES as f64;
  let mut start = visible_start.min(entries.len().saturating_sub(1));
  if selected_index < start {
    start = selected_index;
  }
  let mut measured: Vec<(String, f64)>;
  loop {
    measured = Vec::with_capacity(AUTOCOMPLETE_MAX_ITEMS);
    let mut used_h = 0.0;
    let mut idx = 0;
    while start + idx < entries.len() && idx < AUTOCOMPLETE_MAX_ITEMS {
      let entry_idx = start + idx;
      let label = refs.autocomplete_labels[idx];
      let row_label_w = if idx == 0 { label_w_bottom } else { label_w_full };
      let (rendered, body_h_raw) = if label == nil {
        (String::new(), HISTORY_BODY_LINE_HEIGHT)
      } else {
        configure_history_body_label(label);
        fit_history_body_with_ellipsis(label, entries[entry_idx].text, row_label_w, body_max_height)
      };
      // Pin every row to the 2-line max height regardless of how much
      // content actually fits. Mixed 1-line / 2-line heights look jarring
      // as the user cursors up the list (rows visibly resize between
      // 30 pt and 48 pt). Uniform row height eliminates the bounce — short
      // entries simply leave breathing room below their text. The
      // `body_h_raw` from `fit_history_body_with_ellipsis` is still used
      // implicitly because that helper sets the rendered text + ellipsis;
      // we just override its height suggestion here.
      let _ = body_h_raw;
      let body_h = body_max_height;
      let row_h = body_h + 2.0 * HISTORY_ROW_PAD_Y;
      // Fit budget: the cumulative row stack may reach UP TO the inner top
      // border (HISTORY_LIST_HEIGHT - OVERLAY_PAD_TOP), measured from y=0.
      // - Bottom-anchored layout starts at OVERLAY_PAD_BOTTOM and packs up;
      //   with this budget the topmost row can extend at most into the top
      //   pad zone (no clipping past the card edge).
      // - Top-anchored layout puts the topmost row's TOP at the inner top
      //   border and packs down; with this budget the bottommost row's
      //   bottom can drop at most to y=0 (the card edge, slight bottom-pad
      //   consumption — never clipped past the edge).
      // 8 single-line rows (254 pt) fit; 5 two-line rows (248 pt) fit.
      let row_budget = HISTORY_LIST_HEIGHT - OVERLAY_PAD_TOP;
      let projected = used_h + row_h + (if idx == 0 { 0.0 } else { HISTORY_ROW_GAP });
      if idx > 0 && projected > row_budget + 0.5 {
        break;
      }
      used_h += row_h + (if idx == 0 { 0.0 } else { HISTORY_ROW_GAP });
      measured.push((rendered, body_h));
      idx += 1;
    }
    let visible_count = measured.len();
    let end = start + visible_count;
    if selected_index < end || start + 1 >= entries.len() {
      break;
    }
    start += 1;
  }
  let end = start + measured.len();

  // Truncation status of the selected entry (used to gate the "view ▶" hint
  // and the right-arrow expand). Only meaningful when selected is in the
  // window — which the loop above guarantees if any entries exist.
  let selected_truncated = selected_index
    .checked_sub(start)
    .and_then(|i| measured.get(i))
    .map(|(text, _)| text.ends_with('\u{2026}'))
    .unwrap_or(false);

  // Pre-compute the expanded body for the selected row, if expand mode is on.
  // The expanded row's top stays at its list-mode top edge; its body extends
  // downward (smaller y) to fit the entry's full text. Cap so the expanded
  // row's bottom doesn't drop below the card's bottom edge — items below the
  // expanded row also shift down by `expand_delta`, so the cap is computed
  // against the row's UNEXPANDED top.
  let mut expanded_body: Option<(String, f64)> = None;
  let sel_in_window = if expanded { selected_index.checked_sub(start) } else { None };
  if let Some(sel_idx) = sel_in_window {
    if sel_idx < measured.len() {
      let row_label_w = if sel_idx == 0 { label_w_bottom } else { label_w_full };
      // Natural bottom edge of the selected row in card-local coords.
      let mut nat_bottom = LIST_BASE_Y;
      for (_, body_h) in measured.iter().take(sel_idx) {
        nat_bottom += body_h + 2.0 * HISTORY_ROW_PAD_Y + HISTORY_ROW_GAP;
      }
      let nat_top = nat_bottom + measured[sel_idx].1 + 2.0 * HISTORY_ROW_PAD_Y;
      // Two expand directions:
      // - Selected row is the bottom-most visible (sel_idx == 0): there's
      //   no room below to grow into, so the card grows UPWARD instead.
      //   Cap the body at OVERLAY_HEIGHT_MAX minus card chrome.
      // - Otherwise: row's TOP stays put; bottom extends down toward the
      //   list base. Cap by the available downward room.
      let max_expanded_height = if sel_idx == 0 {
        (OVERLAY_HEIGHT_MAX - LIST_BASE_Y - OVERLAY_PAD_TOP).max(HISTORY_BODY_LINE_HEIGHT)
      } else {
        (nat_top - LIST_BASE_Y).max(HISTORY_BODY_LINE_HEIGHT)
      };
      let max_body_for_expand = (max_expanded_height - 2.0 * HISTORY_ROW_PAD_Y).max(1.0);
      let label = refs.autocomplete_labels[sel_idx];
      if label != nil {
        configure_history_body_label(label);
        let _: () = msg_send![label, setMaximumNumberOfLines: 0isize];
        let (rendered, h) = fit_history_body_with_ellipsis(
          label,
          entries[selected_index].text,
          row_label_w,
          max_body_for_expand,
        );
        let body_h = h.max(HISTORY_BODY_LINE_HEIGHT).min(max_body_for_expand);
        expanded_body = Some((rendered, body_h));
      }
    }
  }

  // Vertical shift applied to the expanded row plus the rows on the
  // shift side (above for expand-up; at-or-below for expand-down).
  let expand_delta = match (sel_in_window, &expanded_body) {
    (Some(idx), Some((_, exp_h))) if idx < measured.len() => {
      let natural = measured[idx].1;
      (exp_h - natural).max(0.0)
    }
    _ => 0.0,
  };

  // Fixed card height in history mode: we hold the overlay at
  // HISTORY_CARD_HEIGHT regardless of how many rows fit so the search bar
  // sits in a stable position and the card doesn't visibly re-snap as the
  // user types and the result count changes. Gap-stretching above the
  // rows fills any slack between the top of the card and the highest
  // fitted row.
  //
  // Expand-up: when the selected row is the bottom-most visible (no room
  // below), the row grows UPWARD by `expand_delta` and the card's top
  // edge moves up by the same amount to make room. This is the one case
  // where the card grows beyond HISTORY_CARD_HEIGHT.
  let expand_up = sel_in_window == Some(0) && expand_delta > 0.0;
  let height = if expand_up {
    (HISTORY_CARD_HEIGHT + expand_delta).min(OVERLAY_HEIGHT_MAX)
  } else {
    HISTORY_CARD_HEIGHT
  };

  if overlay_debug_logs_enabled() {
    let row_body_heights: Vec<u32> = measured.iter().map(|(_, h)| h.round() as u32).collect();
    eprintln!(
      "OVERLAY_HISTORY_LIST entries={} selected={} visible_window=[{}..{}] \
       width={:.0} window_height={:.0} body_heights={:?} expand_delta={:.0}",
      entries.len(),
      selected_index,
      start,
      end,
      width,
      height,
      row_body_heights,
      expand_delta,
    );
  }

  apply_history_window_frame(refs, current_frame, screen, width, height);
  apply_history_card_frame(refs, width, height);

  // Layout anchor: bottom-anchor when at the newest (newest visible row sits
  // flush above the bottom pad, slack lands at top); top-anchor once scrolled
  // (oldest visible row sits flush at the inner top border AND bottom row
  // stays flush at the inner bottom border — slack distributes across the
  // gaps so neither end has a visible gap).
  let sum_rows_h: f64 = measured.iter().map(|(_, h)| h + 2.0 * HISTORY_ROW_PAD_Y).sum();
  let target_top_anchor_total = HISTORY_LIST_HEIGHT - OVERLAY_PAD_TOP - OVERLAY_PAD_BOTTOM;
  let layout_gap = if !is_bottom_anchored && measured.len() > 1 {
    // Stretch the gap so total_used (rows + gaps) reaches the inner-content
    // height — both ends flush. Floor at 0 so rows never overlap; for
    // densely-packed cases (8 single-line rows in 248 pt budget) the gap
    // shrinks below the default 2 pt so the bottom row stays flush.
    ((target_top_anchor_total - sum_rows_h) / (measured.len() - 1) as f64).max(0.0)
  } else {
    HISTORY_ROW_GAP
  };
  let _ = sum_rows_h;
  let layout_bottom_y = LIST_BASE_Y;

  // Walk rows bottom-up. `entries[start]` (newest in the visible window) sits
  // at the bottom; `entries[end-1]` (oldest in the visible window) sits at the
  // top. The cursor advances by each row's NATURAL height so rows above the
  // expanded one stay anchored to their natural positions.
  let mut nat_row_bottom = layout_bottom_y;
  for vis_idx in 0..measured.len() {
    let entry_idx = start + vis_idx;
    let (rendered, body_h) = &measured[vis_idx];
    let row_label_w = if vis_idx == 0 { label_w_bottom } else { label_w_full };
    let is_selected = entry_idx == selected_index;
    let is_selected_expanded = is_selected && expanded_body.is_some();

    // Row height: natural for non-expanded rows; expanded body + pad otherwise.
    let row_h = if is_selected_expanded {
      let exp_h = expanded_body.as_ref().map(|(_, h)| *h).unwrap_or(*body_h);
      exp_h + 2.0 * HISTORY_ROW_PAD_Y
    } else {
      body_h + 2.0 * HISTORY_ROW_PAD_Y
    };

    // Bottom y depends on the expand direction:
    // - Expand-up (selected row is the bottom-most): rows ABOVE the
    //   expanded one shift UP by `expand_delta` (the card grew upward to
    //   make room). Selected row stays at its natural bottom.
    // - Expand-down (default): rows AT-or-BELOW the expanded one shift
    //   down by `expand_delta` (off-card content gets clipped). Rows
    //   above stay anchored.
    let row_bottom_y = match sel_in_window {
      Some(sel) if expand_up && vis_idx > sel => nat_row_bottom + expand_delta,
      Some(sel) if !expand_up && vis_idx <= sel => nat_row_bottom - expand_delta,
      _ => nat_row_bottom,
    };

    let bg = refs.autocomplete_bgs[vis_idx];
    if bg != nil {
      if is_selected {
        let bg_frame = NSRect::new(NSPoint::new(bg_x, row_bottom_y), NSSize::new(bg_w_full, row_h));
        let _: () = msg_send![bg, setFrame: bg_frame];
        let bg_layer: id = msg_send![bg, layer];
        if bg_layer != nil {
          let bg_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
            nil,
            HISTORY_SELECTED_BG_R,
            HISTORY_SELECTED_BG_G,
            HISTORY_SELECTED_BG_B,
            HISTORY_SELECTED_BG_ALPHA,
          );
          let bg_cg: id = msg_send![bg_color, CGColor];
          let _: () = msg_send![bg_layer, setBackgroundColor: bg_cg];
          let _: () = msg_send![bg_layer, setCornerRadius: HISTORY_BG_RADIUS];
        }
        let _: () = msg_send![bg, setHidden: NO];
        if is_selected_expanded {
          // Promote the expanded selected row above siblings so it visually
          // covers any rows below that get pushed off the card.
          let _: () = msg_send![refs.card_view, addSubview: bg];
        }
      } else {
        let _: () = msg_send![bg, setHidden: YES];
      }
    }

    let label = refs.autocomplete_labels[vis_idx];
    if label != nil {
      configure_history_body_label(label);
      let color =
        NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, HISTORY_TEXT_ALPHA);
      let _: () = msg_send![label, setTextColor: color];
      // Match-highlight: when the rendered text starts the original entry,
      // byte offsets carry over (head-anchored truncation preserves the
      // beginning). Drop ranges that fall past the rendered cut.
      let entry = &entries[entry_idx];
      let clamp_ranges = |limit: usize| -> Vec<(usize, usize)> {
        entry
          .match_ranges
          .iter()
          .filter_map(|&(s, e)| if s < limit { Some((s, e.min(limit))) } else { None })
          .collect()
      };
      if is_selected_expanded {
        let (exp_rendered, exp_body_h) = expanded_body.as_ref().unwrap();
        let _: () = msg_send![label, setMaximumNumberOfLines: 0isize];
        // Body sits at the top of the expanded row (largest y).
        let label_y = row_bottom_y + row_h - HISTORY_ROW_PAD_Y - exp_body_h;
        let label_frame =
          NSRect::new(NSPoint::new(label_x, label_y), NSSize::new(row_label_w, *exp_body_h));
        let _: () = msg_send![label, setFrame: label_frame];
        let exp_ranges = clamp_ranges(exp_rendered.len());
        apply_history_label_text(label, exp_rendered, &exp_ranges);
        let _: () = msg_send![label, setHidden: NO];
        let _: () = msg_send![refs.card_view, addSubview: label];
      } else {
        let label_y = row_bottom_y + HISTORY_ROW_PAD_Y;
        let label_frame =
          NSRect::new(NSPoint::new(label_x, label_y), NSSize::new(row_label_w, *body_h));
        let _: () = msg_send![label, setFrame: label_frame];
        let row_ranges = clamp_ranges(rendered.len());
        apply_history_label_text(label, rendered, &row_ranges);
        let _: () = msg_send![label, setHidden: NO];
      }
    }

    // The right-meta column is a vertical stack inside the highlight bg,
    // CENTERED vertically within the bg. Top → bottom:
    //   - char-count
    //   - HISTORY_META_STACK_GAP (1 pt)
    //   - time-ago
    //   - HISTORY_META_STACK_GAP (1 pt)
    //   - "▶" expand marker (only on truncated rows)
    //
    // All three labels share the same font size and label height so the
    // 3-item stack (9 + 1 + 9 + 1 + 9 = 29 pt) fits inside a 1-line row's
    // 30-pt-tall bg with 1 pt of margin. The 2-item stack (no arrow) is
    // 19 pt and gets centered in whatever bg height the row has. On
    // selected+expanded rows all three hide so the body fills the row.
    let cc_h = HISTORY_CHAR_COUNT_FONT_SIZE + HISTORY_META_LABEL_VPAD;
    let ts_h = HISTORY_TS_FONT_SIZE + HISTORY_META_LABEL_VPAD;
    let marker_h = HISTORY_EXPAND_MARKER_FONT_SIZE + HISTORY_META_LABEL_VPAD;
    let bg_h_full = body_h + 2.0 * HISTORY_ROW_PAD_Y;
    let bg_top = row_bottom_y + bg_h_full;
    let row_truncated = rendered.ends_with('\u{2026}');
    let show_marker = row_truncated && !is_selected_expanded;
    // Stack height varies by whether the arrow is shown — center each
    // shape independently so 1-line rows (no arrow) and 2-line truncated
    // rows (3-item stack) both look balanced.
    let stack_h = cc_h
      + HISTORY_META_STACK_GAP
      + ts_h
      + if show_marker { HISTORY_META_STACK_GAP + marker_h } else { 0.0 };
    let stack_top_offset = ((bg_h_full - stack_h) / 2.0).max(0.0);
    let cc_y = bg_top - stack_top_offset - cc_h;
    let ts_y = cc_y - HISTORY_META_STACK_GAP - ts_h;
    let marker_y = ts_y - HISTORY_META_STACK_GAP - marker_h;

    // "▶" expand marker — bottom of the stack, only when this row's body
    // was truncated (rendered text ended with "…"). Hidden in expanded
    // mode for the selected row.
    let marker = refs.autocomplete_expand_markers[vis_idx];
    if marker != nil {
      if show_marker {
        let marker_y_clamped = marker_y.max(row_bottom_y);
        let marker_x = bg_x + bg_w_full - HISTORY_TEXT_INNER_PAD_X - HISTORY_EXPAND_MARKER_WIDTH;
        let frame = NSRect::new(
          NSPoint::new(marker_x, marker_y_clamped),
          NSSize::new(HISTORY_EXPAND_MARKER_WIDTH, marker_h),
        );
        let _: () = msg_send![marker, setFrame: frame];
        let _: () = msg_send![marker, setHidden: NO];
        // Bring above the highlight bg so it's always visible.
        let _: () = msg_send![refs.card_view, addSubview: marker];
      } else {
        let _: () = msg_send![marker, setHidden: YES];
      }
    }

    // Time-ago label INSIDE the highlight bg, right-aligned, stacked
    // under the char-count. Hidden when the selected row is expanded
    // (the body fills the row visually).
    let ts_label = refs.autocomplete_ts_labels[vis_idx];
    if ts_label != nil {
      if is_selected_expanded {
        let _: () = msg_send![ts_label, setHidden: YES];
      } else {
        let label_x = bg_x + bg_w_full - HISTORY_TEXT_INNER_PAD_X - HISTORY_TS_WIDTH;
        let frame = NSRect::new(NSPoint::new(label_x, ts_y), NSSize::new(HISTORY_TS_WIDTH, ts_h));
        let _: () = msg_send![ts_label, setFrame: frame];
        let s = crate::transcript_history::format_timestamp_compact(entries[entry_idx].ts_ms);
        let _: () = msg_send![ts_label, setStringValue: NSString::alloc(nil).init_str(&s)];
        let _: () = msg_send![ts_label, setHidden: NO];
        // Bring above the highlight bg so it stays visible on selected rows.
        let _: () = msg_send![refs.card_view, addSubview: ts_label];
      }
    }

    // Char-count label INSIDE the highlight bg, top-anchored to the bg.
    // Always visible unless the selected row is expanded.
    let cc_label = refs.autocomplete_char_count_labels[vis_idx];
    if cc_label != nil {
      if is_selected_expanded {
        let _: () = msg_send![cc_label, setHidden: YES];
      } else {
        let cc_x = bg_x + bg_w_full - HISTORY_TEXT_INNER_PAD_X - HISTORY_CHAR_COUNT_WIDTH;
        let frame =
          NSRect::new(NSPoint::new(cc_x, cc_y), NSSize::new(HISTORY_CHAR_COUNT_WIDTH, cc_h));
        let _: () = msg_send![cc_label, setFrame: frame];
        let s = crate::transcript_history::format_char_count_compact(entries[entry_idx].char_count);
        let _: () = msg_send![cc_label, setStringValue: NSString::alloc(nil).init_str(&s)];
        let _: () = msg_send![cc_label, setHidden: NO];
        let _: () = msg_send![refs.card_view, addSubview: cc_label];
      }
    }

    // Advance by NATURAL height + the (possibly stretched) layout gap. In
    // top-anchor mode, layout_gap stretches so total_used == budget; in
    // bottom-anchor mode it equals HISTORY_ROW_GAP and slack falls at top.
    nat_row_bottom += body_h + 2.0 * HISTORY_ROW_PAD_Y + layout_gap;
  }

  layout_history_hints(refs, width, selected_truncated, expanded);
  layout_history_search_bar(refs, width);

  // Hide unused row slots.
  for i in measured.len()..AUTOCOMPLETE_MAX_ITEMS {
    let _: () = msg_send![refs.autocomplete_labels[i], setHidden: YES];
    let _: () = msg_send![refs.autocomplete_bgs[i], setHidden: YES];
    if refs.autocomplete_expand_markers[i] != nil {
      let _: () = msg_send![refs.autocomplete_expand_markers[i], setHidden: YES];
    }
    if refs.autocomplete_ts_labels[i] != nil {
      let _: () = msg_send![refs.autocomplete_ts_labels[i], setHidden: YES];
    }
    if refs.autocomplete_char_count_labels[i] != nil {
      let _: () = msg_send![refs.autocomplete_char_count_labels[i], setHidden: YES];
    }
  }

  // Publish the fitted state so the app can make navigation decisions on the
  // next Up/Down/Right.
  LAST_HISTORY_VISIBLE_START.store(start, Ordering::Relaxed);
  LAST_HISTORY_VISIBLE_COUNT.store(measured.len(), Ordering::Relaxed);
  LAST_HISTORY_SELECTED_TRUNCATED.store(selected_truncated, Ordering::Relaxed);
}

/// Set the row body label's text, optionally highlighting a match range
/// with a translucent yellow background. The match range is in UTF-8 byte
/// offsets of `text` (the renderer's measure system uses byte offsets); we
/// convert to UTF-16 offsets here because NSAttributedString is UTF-16
/// internally.
unsafe fn apply_history_label_text(label: id, text: &str, match_ranges: &[(usize, usize)]) {
  let ns_text = NSString::alloc(nil).init_str(text);
  if match_ranges.is_empty() {
    let _: () = msg_send![label, setStringValue: ns_text];
    return;
  }
  let attr: id = msg_send![class!(NSMutableAttributedString), alloc];
  let attr: id = msg_send![attr, initWithString: ns_text];
  let highlight = NSColor::colorWithCalibratedRed_green_blue_alpha_(
    nil,
    SEARCH_MATCH_BG_R,
    SEARCH_MATCH_BG_G,
    SEARCH_MATCH_BG_B,
    SEARCH_MATCH_BG_ALPHA,
  );
  let bg_attr_name = NSString::alloc(nil).init_str("NSBackgroundColor");
  let mut applied_any = false;
  for &(start, end) in match_ranges {
    if start >= end || end > text.len() {
      continue;
    }
    let utf16_loc = byte_offset_to_utf16_count(text, start);
    let utf16_len = byte_offset_to_utf16_count(text, end) - utf16_loc;
    if utf16_len == 0 {
      continue;
    }
    let range = cocoa::foundation::NSRange::new(utf16_loc as u64, utf16_len as u64);
    let _: () = msg_send![attr, addAttribute: bg_attr_name value: highlight range: range];
    applied_any = true;
  }
  if applied_any {
    let _: () = msg_send![label, setAttributedStringValue: attr];
  } else {
    let _: () = msg_send![label, setStringValue: ns_text];
  }
}

fn byte_offset_to_utf16_count(text: &str, byte_offset: usize) -> usize {
  let cap = byte_offset.min(text.len());
  let mut count = 0usize;
  for (idx, ch) in text.char_indices() {
    if idx >= cap {
      break;
    }
    count += ch.len_utf16();
  }
  count
}

/// Hide the bottom-right hint labels (the old "view ▶ / ◀ esc" pair).
/// They were dropped in favour of in-row "▶" expand markers and a quieter
/// overall layout. Kept as a function rather than inlining the hides
/// because both empty-state and non-empty-state render paths used to share
/// the layout helper.
unsafe fn layout_history_hints(refs: OverlayRefs, _width: f64, _show_view: bool, _expanded: bool) {
  if refs.label != nil {
    let _: () = msg_send![refs.label, setHidden: YES];
  }
  if refs.hold_badge != nil {
    let _: () = msg_send![refs.hold_badge, setHidden: YES];
  }
}

/// Position and show the editable search field at the bottom of the
/// history overlay. Text is left-aligned at the same x as the row body
/// labels so the typed query is visually continuous with the rows; AppKit
/// vertically centers single-line text within the frame automatically.
/// Also positions the custom blinking caret immediately to the right of
/// the typed text.
unsafe fn layout_history_search_bar(refs: OverlayRefs, width: f64) {
  if refs.search_field == nil {
    return;
  }
  let bg_x = (OVERLAY_PAD_X - HISTORY_BG_X_INSET).max(2.0);
  let field_x = bg_x + HISTORY_TEXT_INNER_PAD_X;
  let field_w = (width - field_x - OVERLAY_PAD_X).max(1.0);
  // NSTextField with setBezeled:NO doesn't vertical-center single-line
  // text in an over-sized frame — it aligns the baseline near the bottom.
  // Shrink the frame to roughly font line-height and y-center it inside
  // the bar; that's the simplest way to get visually centred typed text.
  let field_y = SEARCH_BAR_Y + (SEARCH_BAR_HEIGHT - SEARCH_FIELD_HEIGHT) / 2.0;
  let frame =
    NSRect::new(NSPoint::new(field_x, field_y), NSSize::new(field_w, SEARCH_FIELD_HEIGHT));
  let _: () = msg_send![refs.search_field, setFrame: frame];
  let _: () = msg_send![refs.search_field, setHidden: NO];
  let _: () = msg_send![refs.card_view, addSubview: refs.search_field];

  // Caret: width of the current text with the field's font, then place
  // the 1-pt-wide caret bar immediately to the right.
  if refs.search_caret != nil {
    let s: id = msg_send![refs.search_field, stringValue];
    let font: id = msg_send![refs.search_field, font];
    let text_width = if s != nil && font != nil {
      let attrs: id = msg_send![class!(NSMutableDictionary), dictionaryWithCapacity: 1usize];
      let key = NSString::alloc(nil).init_str("NSFont");
      let _: () = msg_send![attrs, setObject: font forKey: key];
      let size: NSSize = msg_send![s, sizeWithAttributes: attrs];
      size.width as f64
    } else {
      0.0
    };
    // 1pt offset from the right edge of the text so the caret doesn't
    // overlap the last glyph.
    let caret_x = field_x + text_width + 1.0;
    let caret_y = SEARCH_BAR_Y + (SEARCH_BAR_HEIGHT - SEARCH_CARET_HEIGHT) / 2.0;
    let caret_frame = NSRect::new(
      NSPoint::new(caret_x, caret_y),
      NSSize::new(SEARCH_CARET_WIDTH, SEARCH_CARET_HEIGHT),
    );
    let _: () = msg_send![refs.search_caret, setFrame: caret_frame];
    let _: () = msg_send![refs.search_caret, setHidden: NO];
    let _: () = msg_send![refs.card_view, addSubview: refs.search_caret];
  }
}

unsafe fn configure_history_body_label(label: id) {
  let font: id = msg_send![class!(NSFont), systemFontOfSize: HISTORY_BODY_FONT_SIZE];
  if font != nil {
    let _: () = msg_send![label, setFont: font];
  }
  let _: () = msg_send![label, setUsesSingleLineMode: NO];
  let _: () = msg_send![label, setAlignment: 0isize]; // NSTextAlignmentLeft
  // 0 = NSLineBreakByWordWrapping. AppKit's tail-truncation mode treats the
  // text as a single line and won't wrap to two; we use word wrapping here
  // and manually splice "…" onto a head-anchored prefix when the entry
  // exceeds the two-line budget (see `fit_history_body_with_ellipsis`).
  let _: () = msg_send![label, setLineBreakMode: 0isize];
  let _: () = msg_send![label, setMaximumNumberOfLines: HISTORY_BODY_MAX_LINES as isize];
}

/// Word-wrap-fits the start of `body_text` into `max_height` at width
/// `width`. If the natural height exceeds `max_height`, head-anchors the
/// prefix and appends "…". Returns `(rendered_text, measured_height)`.
unsafe fn fit_history_body_with_ellipsis(
  label: id,
  body_text: &str,
  width: f64,
  max_height: f64,
) -> (String, f64) {
  let trimmed = body_text.trim();
  if trimmed.is_empty() {
    return (String::new(), HISTORY_BODY_LINE_HEIGHT);
  }
  // Lift the line cap before measuring. `configure_history_body_label` sets
  // `setMaximumNumberOfLines: 2`, which causes `cellSizeForBounds` to clamp
  // the reported height at 2 × line_height — so the "exceeds max_height"
  // branch below would never fire and the helper would never append the
  // ellipsis. Restore the cap when the caller next calls
  // `configure_history_body_label` (the layout loop does this every render).
  let _: () = msg_send![label, setMaximumNumberOfLines: 0isize];
  let measured_full = measure_label_height(label, trimmed, width);
  if measured_full <= max_height + 0.5 {
    return (trimmed.to_string(), measured_full);
  }
  // Binary-search the longest char-boundary prefix whose `prefix + "…"` fits.
  let ellipsis = "\u{2026}";
  let boundaries: Vec<usize> = trimmed
    .char_indices()
    .map(|(i, _)| i)
    .chain(std::iter::once(trimmed.len()))
    .collect();
  let mut lo = 1usize;
  let mut hi = boundaries.len() - 1;
  while lo < hi {
    let mid = lo + (hi - lo + 1) / 2;
    let end = boundaries[mid];
    let candidate = format!("{}{}", trimmed[..end].trim_end(), ellipsis);
    let h = measure_label_height(label, &candidate, width);
    if h <= max_height + 0.5 {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  let end = boundaries[lo];
  let rendered = format!("{}{}", trimmed[..end].trim_end(), ellipsis);
  let h = measure_label_height(label, &rendered, width).min(max_height);
  (rendered, h)
}

unsafe fn apply_history_window_frame(
  refs: OverlayRefs,
  current_frame: NSRect,
  screen: NSRect,
  width: f64,
  height: f64,
) {
  let default_x = screen.origin.x + (screen.size.width - width) * 0.5;
  let default_y = screen.origin.y + screen.size.height * 0.08;
  let x = if current_frame.size.width <= 0.0 { default_x } else { current_frame.origin.x };
  // Bottom-anchored: keep origin.y exactly where the speech overlay sits today.
  // Increasing height grows the top of the window upward (NSWindow uses
  // bottom-left origins) — exactly the intended behaviour.
  let y = if current_frame.size.height <= 0.0 { default_y } else { current_frame.origin.y };
  let target = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
  if (current_frame.origin.x - target.origin.x).abs() > 0.05
    || (current_frame.origin.y - target.origin.y).abs() > 0.05
    || (current_frame.size.width - target.size.width).abs() > 0.05
    || (current_frame.size.height - target.size.height).abs() > 0.05
  {
    let _: () = msg_send![refs.window, setFrame: target display: YES];
  }
}

unsafe fn apply_history_card_frame(refs: OverlayRefs, width: f64, height: f64) {
  let card_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
  let _: () = msg_send![refs.card_view, setFrame: card_frame];
  let card_layer: id = msg_send![refs.card_view, layer];
  if card_layer != nil {
    let card_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.02, 0.02, 0.02, 0.90);
    let card_cg: id = msg_send![card_color, CGColor];
    let _: () = msg_send![card_layer, setBackgroundColor: card_cg];
    // Carry the subtle outer border forward from speech mode so the history
    // overlay doesn't visually drop a pixel of definition when the user
    // pivots into it.
    let border = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.62, 0.74, 0.98, 0.22);
    let border_cg: id = msg_send![border, CGColor];
    let _: () = msg_send![card_layer, setBorderWidth: OVERLAY_BORDER_THICKNESS];
    let _: () = msg_send![card_layer, setBorderColor: border_cg];
  }
}

unsafe fn render_overlay_history_position(refs: OverlayRefs, position: &str) {
  let label = refs.autocomplete_labels[0];
  let separator = refs.autocomplete_separator;
  if label == nil {
    return;
  }
  if position.is_empty() {
    if label != nil {
      let _: () = msg_send![label, setHidden: YES];
    }
    if separator != nil {
      let _: () = msg_send![separator, setHidden: YES];
    }
    return;
  }

  let current_frame: NSRect = msg_send![refs.window, frame];
  let width = current_frame.size.width;
  let content_width = (width - OVERLAY_PAD_X * 2.0).max(1.0);

  let label_frame = NSRect::new(
    NSPoint::new(OVERLAY_PAD_X + 4.0, OVERLAY_PAD_BOTTOM - 4.0),
    NSSize::new(content_width - 8.0, AUTOCOMPLETE_ROW_HEIGHT),
  );
  let text_ns = NSString::alloc(nil).init_str(position);
  let _: () = msg_send![label, setFrame: label_frame];
  let _: () = msg_send![label, setStringValue: text_ns];
  let font: id = msg_send![class!(NSFont), systemFontOfSize: 11.0f64];
  if font != nil {
    let _: () = msg_send![label, setFont: font];
  }
  let color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.35);
  let _: () = msg_send![label, setTextColor: color];
  let _: () = msg_send![label, setAlignment: 2isize]; // NSTextAlignmentRight
  let _: () = msg_send![label, setHidden: NO];
}

fn main_screen_frame() -> NSRect {
  unsafe {
    let screen = NSScreen::mainScreen(nil);
    if screen == nil {
      NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0))
    } else {
      NSScreen::frame(screen)
    }
  }
}

fn cursor_screen_frame() -> Option<NSRect> {
  unsafe {
    let point: NSPoint = msg_send![class!(NSEvent), mouseLocation];
    screen_frame_for_point(point)
  }
}

/// The primary display — `NSScreen.screens[0]`, the one carrying the menu bar
/// and the global-coordinate origin. NOT `mainScreen` (which tracks keyboard
/// focus and would behave like ActiveWindow on a secondary display).
fn primary_screen_frame() -> NSRect {
  unsafe {
    let screens: id = msg_send![class!(NSScreen), screens];
    if screens != nil {
      let count: usize = msg_send![screens, count];
      if count > 0 {
        let screen: id = msg_send![screens, objectAtIndex: 0usize];
        if screen != nil {
          return NSScreen::frame(screen);
        }
      }
    }
    main_screen_frame()
  }
}

/// Resolve which screen the overlay should anchor to, per the user's
/// `OverlayPosition` preference. Every mode falls back to the cursor screen and
/// then the main screen so we always return a usable frame. `force_fresh`
/// (set on the first show of an utterance) bypasses the ActiveWindow AX cache.
fn resolve_overlay_target_screen(force_fresh: bool) -> NSRect {
  match overlay_position() {
    OverlayPosition::FollowCursor => cursor_screen_frame().unwrap_or_else(main_screen_frame),
    OverlayPosition::PrimaryMonitor => primary_screen_frame(),
    OverlayPosition::ActiveWindow => active_window_screen_frame_cached(force_fresh)
      .or_else(cursor_screen_frame)
      .unwrap_or_else(main_screen_frame),
  }
}

/// `ax_focused_window_screen_frame` behind a short-TTL cache so ActiveWindow
/// mode doesn't do synchronous AX IPC on every streaming reposition (~10-30 Hz).
/// On a fresh read failure we fall through (caller then uses cursor/main) and do
/// not poison the cache. `bypass` forces a fresh read (used on first show).
fn active_window_screen_frame_cached(bypass: bool) -> Option<NSRect> {
  ACTIVE_WINDOW_SCREEN_CACHE.with(|slot| {
    if !bypass {
      if let Some((frame, at)) = *slot.borrow() {
        if at.elapsed() < ACTIVE_WINDOW_SCREEN_CACHE_TTL {
          return Some(frame);
        }
      }
    }
    let fresh = ax_focused_window_screen_frame();
    if let Some(frame) = fresh {
      *slot.borrow_mut() = Some((frame, Instant::now()));
    }
    fresh
  })
}

/// Screen containing the focused window of the frontmost app, via Accessibility.
/// Returns None (caller falls back) if the app is Azad itself, AX can't read the
/// window (full-screen / sandboxed apps), or any attribute is missing.
fn ax_focused_window_screen_frame() -> Option<NSRect> {
  unsafe {
    let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
    if workspace == nil {
      return None;
    }
    let app: id = msg_send![workspace, frontmostApplication];
    if app == nil {
      return None;
    }
    let pid: i32 = msg_send![app, processIdentifier];
    // Never chase our own windows (onboarding / Settings activate Azad).
    if pid == std::process::id() as i32 {
      return None;
    }

    let app_el = AXUIElementCreateApplication(pid);
    if app_el.is_null() {
      return None;
    }
    // Cap blocking if the target app's main thread is wedged.
    let _ = AXUIElementSetMessagingTimeout(app_el, 0.15);

    let result = ax_focused_window_screen_frame_inner(app_el);
    CFRelease(app_el);
    result
  }
}

unsafe fn ax_focused_window_screen_frame_inner(app_el: *const c_void) -> Option<NSRect> {
  let window = ax_copy_element_attribute(app_el, "AXFocusedWindow")?;
  let pos = ax_copy_point_attribute(window, "AXPosition");
  let size = ax_copy_size_attribute(window, "AXSize");
  CFRelease(window);
  let (pos, size) = (pos?, size?);

  // AX is top-left-origin / Y-down in the global space; Cocoa screen frames are
  // bottom-left-origin / Y-up. Flip via NSMaxY(primary). Hit-test the window
  // CENTER so a window straddling displays maps to where most of it sits.
  let primary = primary_screen_frame();
  let cx = pos.x + size.width * 0.5;
  let cocoa_cy = (primary.origin.y + primary.size.height) - (pos.y + size.height * 0.5);
  screen_frame_for_point(NSPoint::new(cx, cocoa_cy))
}

/// Copy an AX element-valued attribute (e.g. AXFocusedWindow). Caller CFReleases.
unsafe fn ax_copy_element_attribute(
  element: *const c_void,
  attribute: &str,
) -> Option<*const c_void> {
  // `alloc/init` is +1-retained and not autoreleased; release it after the copy
  // (it's only an input) so the ActiveWindow hot path doesn't slowly leak.
  let attr = NSString::alloc(nil).init_str(attribute);
  let mut value: *const c_void = std::ptr::null();
  let status = AXUIElementCopyAttributeValue(element, attr as *const c_void, &mut value);
  let _: () = msg_send![attr, release];
  if status != 0 || value.is_null() {
    return None;
  }
  Some(value)
}

unsafe fn ax_copy_point_attribute(element: *const c_void, attribute: &str) -> Option<NSPoint> {
  let value = ax_copy_element_attribute(element, attribute)?;
  let mut point = NSPoint::new(0.0, 0.0);
  let ok =
    AXValueGetValue(value, KAX_VALUE_CG_POINT_TYPE, &mut point as *mut NSPoint as *mut c_void);
  CFRelease(value);
  if ok { Some(point) } else { None }
}

unsafe fn ax_copy_size_attribute(element: *const c_void, attribute: &str) -> Option<NSSize> {
  let value = ax_copy_element_attribute(element, attribute)?;
  let mut size = NSSize::new(0.0, 0.0);
  let ok = AXValueGetValue(value, KAX_VALUE_CG_SIZE_TYPE, &mut size as *mut NSSize as *mut c_void);
  CFRelease(value);
  if ok { Some(size) } else { None }
}

fn is_left_mouse_button_down() -> bool {
  unsafe {
    let pressed: u64 = msg_send![class!(NSEvent), pressedMouseButtons];
    (pressed & 1) != 0
  }
}

fn screen_frame_for_point(point: NSPoint) -> Option<NSRect> {
  unsafe {
    let screens: id = msg_send![class!(NSScreen), screens];
    if screens == nil {
      return None;
    }

    let count: usize = msg_send![screens, count];
    for idx in 0..count {
      let screen: id = msg_send![screens, objectAtIndex: idx];
      if screen == nil {
        continue;
      }
      let frame = NSScreen::frame(screen);
      let min_x = frame.origin.x;
      let min_y = frame.origin.y;
      let max_x = frame.origin.x + frame.size.width;
      let max_y = frame.origin.y + frame.size.height;
      if point.x >= min_x && point.x < max_x && point.y >= min_y && point.y < max_y {
        return Some(frame);
      }
    }

    None
  }
}

fn overlay_screen_frame_for_window(frame: NSRect) -> NSRect {
  if let Some(screen) = current_overlay_screen_frame(frame) {
    return screen;
  }

  cursor_screen_frame().unwrap_or_else(main_screen_frame)
}

fn current_overlay_screen_frame(frame: NSRect) -> Option<NSRect> {
  if frame.size.width > 0.0 && frame.size.height > 0.0 {
    let center = NSPoint::new(
      frame.origin.x + frame.size.width * 0.5,
      frame.origin.y + frame.size.height * 0.5,
    );
    return screen_frame_for_point(center);
  }

  None
}

fn same_screen_frame(a: NSRect, b: NSRect) -> bool {
  const EPS: f64 = 0.5;
  (a.origin.x - b.origin.x).abs() <= EPS
    && (a.origin.y - b.origin.y).abs() <= EPS
    && (a.size.width - b.size.width).abs() <= EPS
    && (a.size.height - b.size.height).abs() <= EPS
}

fn remap_origin_proportionally(
  current_origin: f64,
  source_origin: f64,
  source_size: f64,
  current_size: f64,
  target_origin: f64,
  target_size: f64,
  target_current_size: f64,
) -> f64 {
  let source_range = (source_size - current_size).max(0.0);
  let target_range = (target_size - target_current_size).max(0.0);
  if source_range <= 0.5 || target_range <= 0.5 {
    return target_origin + target_range * 0.5;
  }

  let source_ratio = ((current_origin - source_origin) / source_range).clamp(0.0, 1.0);
  target_origin + source_ratio * target_range
}

unsafe fn move_overlay_to_target_screen(refs: OverlayRefs, force_default_anchor: bool) {
  if !force_default_anchor && is_left_mouse_button_down() {
    return;
  }

  let target_screen = resolve_overlay_target_screen(force_default_anchor);
  let current_frame: NSRect = msg_send![refs.window, frame];
  let current_screen = current_overlay_screen_frame(current_frame);
  if !force_default_anchor
    && current_screen.is_some_and(|screen| same_screen_frame(screen, target_screen))
  {
    return;
  }

  let width = overlay_width_for_screen(target_screen);
  let height = if current_frame.size.height > 0.0 {
    current_frame.size.height.clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX)
  } else {
    OVERLAY_HEIGHT_MIN
  };
  let (mut x, mut y) = if force_default_anchor || current_screen.is_none() {
    (
      target_screen.origin.x + (target_screen.size.width - width) * 0.5,
      target_screen.origin.y + target_screen.size.height * 0.08,
    )
  } else {
    let source = current_screen.unwrap();
    (
      remap_origin_proportionally(
        current_frame.origin.x,
        source.origin.x,
        source.size.width,
        current_frame.size.width,
        target_screen.origin.x,
        target_screen.size.width,
        width,
      ),
      remap_origin_proportionally(
        current_frame.origin.y,
        source.origin.y,
        source.size.height,
        current_frame.size.height,
        target_screen.origin.y,
        target_screen.size.height,
        height,
      ),
    )
  };
  let max_x =
    (target_screen.origin.x + target_screen.size.width - width).max(target_screen.origin.x);
  let max_y =
    (target_screen.origin.y + target_screen.size.height - height).max(target_screen.origin.y);
  x = x.clamp(target_screen.origin.x, max_x);
  y = y.clamp(target_screen.origin.y, max_y);
  let target_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
  if (current_frame.origin.x - target_frame.origin.x).abs() > 0.05
    || (current_frame.origin.y - target_frame.origin.y).abs() > 0.05
    || (current_frame.size.width - target_frame.size.width).abs() > 0.05
    || (current_frame.size.height - target_frame.size.height).abs() > 0.05
  {
    let _: () = msg_send![refs.window, setFrame: target_frame display: YES];
  }
}

fn overlay_width_for_screen(frame: NSRect) -> f64 {
  (frame.size.width * 0.44).clamp(OVERLAY_WIDTH_MIN, OVERLAY_WIDTH_MAX)
}

unsafe fn fit_rendered_body_for_height(
  label: id,
  body_text: &str,
  width: f64,
  max_height: f64,
) -> (String, f64) {
  let trimmed = body_text.trim();
  if trimmed.is_empty() {
    return (String::new(), OVERLAY_TEXT_LINE_HEIGHT.min(max_height));
  }

  let measured_full = measure_label_height(label, trimmed, width);
  if measured_full <= max_height + 0.5 {
    return (trimmed.to_string(), measured_full);
  }

  let mut word_starts = Vec::new();
  let mut prev_ws = true;
  for (idx, ch) in trimmed.char_indices() {
    let is_ws = ch.is_whitespace();
    if !is_ws && prev_ws {
      word_starts.push(idx);
    }
    prev_ws = is_ws;
  }

  if word_starts.is_empty() {
    return (trimmed.to_string(), measured_full);
  }

  let mut lo = 0usize;
  let mut hi = word_starts.len() - 1;
  while lo < hi {
    let mid = (lo + hi) / 2;
    let candidate = trimmed[word_starts[mid]..].trim_start();
    let measured = measure_label_height(label, candidate, width);
    if measured <= max_height + 0.5 {
      hi = mid;
    } else {
      lo = mid + 1;
    }
  }

  let rendered = trimmed[word_starts[lo]..].trim_start().to_string();
  let measured = measure_label_height(label, &rendered, width);
  (rendered, measured)
}

/// Natural height of an NSTextView's current text wrapped to `width`, including the
/// vertical text-container inset. Used to size the scrolling reply region.
unsafe fn measure_textview_height(text: id, width: f64) -> f64 {
  let container: id = msg_send![text, textContainer];
  if container == nil {
    return OVERLAY_TEXT_LINE_HEIGHT;
  }
  let _: () = msg_send![container, setContainerSize: NSSize::new(width.max(1.0), 10_000_000.0)];
  let lm: id = msg_send![text, layoutManager];
  if lm == nil {
    return OVERLAY_TEXT_LINE_HEIGHT;
  }
  let _: () = msg_send![lm, ensureLayoutForTextContainer: container];
  let used: NSRect = msg_send![lm, usedRectForTextContainer: container];
  (used.size.height + 4.0).max(OVERLAY_TEXT_LINE_HEIGHT)
}

unsafe fn measure_label_height(label: id, text: &str, width: f64) -> f64 {
  let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str(text)];
  let cell: id = msg_send![label, cell];
  if cell == nil {
    return OVERLAY_TEXT_LINE_HEIGHT;
  }
  let probe_bounds =
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width.max(1.0), OVERLAY_HEIGHT_MAX * 4.0));
  let measured_size: NSSize = msg_send![cell, cellSizeForBounds: probe_bounds];
  (measured_size.height + 2.0).max(1.0)
}

unsafe fn render_activity_wave(refs: OverlayRefs, activity: &[f32], width: f64, height: f64) {
  let view_layer: id = msg_send![refs.meter_view, layer];
  if view_layer != nil {
    let bg = NSColor::clearColor(nil);
    let bg_cg: id = msg_send![bg, CGColor];
    let _: () = msg_send![view_layer, setBackgroundColor: bg_cg];
    let _: () = msg_send![view_layer, setCornerRadius: 0.0f64];
    let _: () = msg_send![view_layer, setMasksToBounds: YES];
  }

  let count = refs.wave_bars.len().max(1);
  let spacing = (width / count as f64).max(1.0);
  let bar_width = (spacing * 0.82).clamp(1.0, 6.0);
  let max_h = height.max(1.0);
  let samples_len = activity.len();

  for (i, bar) in refs.wave_bars.iter().enumerate() {
    if *bar == nil {
      continue;
    }
    let level = if samples_len == 0 {
      0.0
    } else {
      let sample_idx = ((i as f64 / (count.saturating_sub(1).max(1) as f64))
        * (samples_len - 1) as f64)
        .round() as usize;
      activity[sample_idx].clamp(0.0, 1.0)
    };
    let normalized = level as f64;
    let shaped = ((normalized - 0.08) / 0.92).clamp(0.0, 1.0).powf(1.8);
    let dramatic = shaped.powf(0.44);
    let bar_h = (OVERLAY_WAVE_BAR_MIN_HEIGHT + dramatic * (max_h - OVERLAY_WAVE_BAR_MIN_HEIGHT))
      .clamp(OVERLAY_WAVE_BAR_MIN_HEIGHT, max_h);
    let y = (max_h - bar_h) * 0.5;
    let x = i as f64 * spacing + (spacing - bar_width) * 0.5;
    let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(bar_width, bar_h));
    let _: () = msg_send![*bar, setFrame: frame];
    let _: () = msg_send![*bar, setHidden: if samples_len == 0 { YES } else { NO }];

    let hue = 0.48 + (dramatic * 0.10);
    let alpha = 0.024 + (dramatic * 0.09);
    let color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
      nil,
      (0.10 + hue * 0.08).clamp(0.0, 1.0),
      (0.56 + dramatic * 0.22).clamp(0.0, 1.0),
      (0.72 + dramatic * 0.18).clamp(0.0, 1.0),
      alpha.clamp(0.0, 1.0),
    );
    let cg_color: id = msg_send![color, CGColor];
    let layer: id = msg_send![*bar, layer];
    if layer != nil {
      let _: () = msg_send![layer, setBackgroundColor: cg_color];
      let _: () = msg_send![layer, setCornerRadius: (bar_width * 0.5).max(0.5)];
    }
  }
}

fn listen_toggle_notice_activity(enabled: bool, progress: f32) -> Vec<f32> {
  let p = progress.clamp(0.0, 1.0);
  let mut samples = vec![0.0; OVERLAY_WAVE_BAR_COUNT];
  if samples.is_empty() {
    return samples;
  }

  let center = if enabled { 0.46 + (p as f64 * 0.24) } else { 0.54 - (p as f64 * 0.24) };
  let pulse = ((1.0 - ((p as f64 - 0.38).abs() / 0.40)).clamp(0.0, 1.0)).powf(0.75);
  let energy = 0.22 + pulse * 0.45;

  for (idx, sample) in samples.iter_mut().enumerate() {
    let x = idx as f64 / (OVERLAY_WAVE_BAR_COUNT.saturating_sub(1).max(1) as f64);
    let dist = (x - center).abs();
    let envelope = (1.0 - dist / 0.62).clamp(0.0, 1.0).powf(1.5);
    let ripple = ((x * 18.0 + p as f64 * 10.0).sin() * 0.5 + 0.5).powf(0.9);
    let v = 0.08 + envelope * (0.20 + energy * (0.42 + ripple * 0.58));
    *sample = v.clamp(0.0, 1.0) as f32;
  }

  samples
}

fn mix(a: f64, b: f64, t: f64) -> f64 {
  a + (b - a) * t.clamp(0.0, 1.0)
}

unsafe fn apply_overlay_notice_style(refs: OverlayRefs, style: OverlayNoticeStyle) {
  let card_layer: id = msg_send![refs.card_view, layer];
  if card_layer == nil {
    return;
  }

  match style {
    OverlayNoticeStyle::Standard => {}
    OverlayNoticeStyle::ListenToggle { enabled, progress } => {
      let p = progress.clamp(0.0, 1.0) as f64;
      let pulse = ((1.0 - ((p - 0.38).abs() / 0.40)).clamp(0.0, 1.0)).powf(0.75);

      let (base_r, base_g, base_b, glow_r, glow_g, glow_b) = if enabled {
        (0.03, 0.07, 0.08, 0.15, 0.80, 0.70)
      } else {
        (0.08, 0.05, 0.03, 0.98, 0.54, 0.16)
      };
      let bg_mix = 0.20 + pulse * 0.22;
      let bg = NSColor::colorWithCalibratedRed_green_blue_alpha_(
        nil,
        mix(base_r, glow_r, bg_mix).clamp(0.0, 1.0),
        mix(base_g, glow_g, bg_mix).clamp(0.0, 1.0),
        mix(base_b, glow_b, bg_mix).clamp(0.0, 1.0),
        LISTEN_NOTICE_CARD_ALPHA,
      );
      let bg_cg: id = msg_send![bg, CGColor];
      let _: () = msg_send![card_layer, setBackgroundColor: bg_cg];

      let border = NSColor::colorWithCalibratedRed_green_blue_alpha_(
        nil,
        mix(0.52, glow_r, 0.65).clamp(0.0, 1.0),
        mix(0.64, glow_g, 0.65).clamp(0.0, 1.0),
        mix(0.90, glow_b, 0.65).clamp(0.0, 1.0),
        (0.34 + pulse * 0.46).clamp(0.0, 1.0),
      );
      let border_cg: id = msg_send![border, CGColor];
      let _: () = msg_send![card_layer, setBorderColor: border_cg];

      let text = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.98);
      let _: () = msg_send![refs.label, setTextColor: text];

      let wave_alpha =
        (LISTEN_NOTICE_WAVE_BASE_ALPHA + pulse * LISTEN_NOTICE_WAVE_PEAK_ALPHA).clamp(0.0, 1.0);
      for bar in refs.wave_bars {
        if bar == nil {
          continue;
        }
        let hidden: i8 = msg_send![bar, isHidden];
        if hidden != 0 {
          continue;
        }
        let layer: id = msg_send![bar, layer];
        if layer == nil {
          continue;
        }
        let tinted = NSColor::colorWithCalibratedRed_green_blue_alpha_(
          nil,
          glow_r.clamp(0.0, 1.0),
          glow_g.clamp(0.0, 1.0),
          glow_b.clamp(0.0, 1.0),
          wave_alpha,
        );
        let tinted_cg: id = msg_send![tinted, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: tinted_cg];
      }

      if refs.busy_gradient_layer != nil {
        let _: () = msg_send![refs.busy_gradient_layer, setHidden: YES];
      }
    }
  }
}

unsafe fn apply_busy_border_style(
  refs: OverlayRefs,
  busy_phase: Option<f32>,
  width: f64,
  height: f64,
) {
  let card_layer: id = msg_send![refs.card_view, layer];
  if card_layer == nil {
    return;
  }

  let subtle = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.62, 0.74, 0.98, 0.22);
  let subtle_cg: id = msg_send![subtle, CGColor];
  let _: () = msg_send![card_layer, setBorderWidth: OVERLAY_BORDER_THICKNESS];
  let _: () = msg_send![card_layer, setBorderColor: subtle_cg];

  if refs.busy_gradient_layer == nil || refs.busy_mask_layer == nil {
    return;
  }

  let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width.max(1.0), height.max(1.0)));
  let _: () = msg_send![refs.busy_gradient_layer, setFrame: frame];
  let _: () = msg_send![refs.busy_mask_layer, setFrame: frame];
  let _: () = msg_send![refs.busy_mask_layer, setCornerRadius: OVERLAY_CARD_RADIUS];
  let _: () = msg_send![refs.busy_mask_layer, setBorderWidth: OVERLAY_BUSY_RING_THICKNESS];
  // Always un-hide the mask. If something (e.g. the history-list renderer) hid it,
  // CALayer.mask still gets sampled — a hidden mask renders as fully transparent
  // alpha, which clips the gradient to invisibility. This is the missing-border-
  // pulse symptom.
  let _: () = msg_send![refs.busy_mask_layer, setHidden: NO];

  let Some(phase) = busy_phase else {
    let _: () = msg_send![refs.busy_gradient_layer, setHidden: YES];
    return;
  };

  let brightness = 0.98 + 0.02 * (phase as f64 * 1.4).sin().abs();
  let _: () = msg_send![refs.busy_gradient_layer, setHidden: NO];
  let _: () = msg_send![refs.busy_gradient_layer, setOpacity: brightness as f32];

  let angle = (phase as f64).rem_euclid(std::f64::consts::TAU);
  let dx = angle.cos();
  let dy = angle.sin();
  let center = NSPoint::new(0.5, 0.5);
  let end = NSPoint::new(0.5 + dx * 0.5, 0.5 + dy * 0.5);
  let _: () = msg_send![refs.busy_gradient_layer, setStartPoint: center];
  let _: () = msg_send![refs.busy_gradient_layer, setEndPoint: end];
}

unsafe fn create_settings_window() -> SettingsWindowRefs {
  let frame = main_screen_frame();
  let x = frame.origin.x + (frame.size.width - SETTINGS_WINDOW_WIDTH) * 0.5;
  let y = frame.origin.y + (frame.size.height - SETTINGS_WINDOW_HEIGHT) * 0.5;
  let window_frame =
    NSRect::new(NSPoint::new(x, y), NSSize::new(SETTINGS_WINDOW_WIDTH, SETTINGS_WINDOW_HEIGHT));

  let style = NSWindowStyleMask::NSTitledWindowMask
    | NSWindowStyleMask::NSClosableWindowMask
    | NSWindowStyleMask::NSMiniaturizableWindowMask;
  let window: id = msg_send![class!(NSWindow), alloc];
  let window: id = msg_send![window, initWithContentRect: window_frame
                                                styleMask: style
                                                  backing: NSBackingStoreType::NSBackingStoreBuffered
                                                    defer: NO];
  let _: () = msg_send![window, setReleasedWhenClosed: NO];
  let _: () = msg_send![window, setTitle: NSString::alloc(nil).init_str("Azad Settings")];

  let content_view: id = msg_send![window, contentView];

  let body_frame = NSRect::new(
    NSPoint::new(SETTINGS_INSET_X, SETTINGS_INSET_X),
    NSSize::new(
      SETTINGS_WINDOW_WIDTH - (SETTINGS_INSET_X * 2.0),
      SETTINGS_WINDOW_HEIGHT - (SETTINGS_INSET_X * 2.0),
    ),
  );
  let body_view: id = msg_send![class!(NSView), alloc];
  let body_view: id = msg_send![body_view, initWithFrame: body_frame];
  let _: () = msg_send![
      body_view,
      setAutoresizingMask: (NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE)
  ];
  let _: () = msg_send![content_view, addSubview: body_view];

  let sidebar_height = body_frame.size.height.max(220.0);
  let sidebar_frame =
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SETTINGS_SIDEBAR_WIDTH, sidebar_height));
  let sidebar_scroll: id = msg_send![class!(NSScrollView), alloc];
  let sidebar_scroll: id = msg_send![sidebar_scroll, initWithFrame: sidebar_frame];
  let _: () = msg_send![sidebar_scroll, setHasVerticalScroller: NO];
  let _: () = msg_send![sidebar_scroll, setBorderType: 0usize];
  let _: () = msg_send![sidebar_scroll, setDrawsBackground: NO];
  let _: () = msg_send![body_view, addSubview: sidebar_scroll];

  let tab_list_view: id = msg_send![class!(NSTableView), alloc];
  let tab_list_view: id = msg_send![tab_list_view, initWithFrame: sidebar_frame];
  let _: () = msg_send![tab_list_view, setHeaderView: nil];
  let _: () = msg_send![tab_list_view, setUsesAlternatingRowBackgroundColors: NO];
  let _: () = msg_send![tab_list_view, setAllowsMultipleSelection: NO];
  let _: () = msg_send![tab_list_view, setAllowsEmptySelection: NO];
  let _: () = msg_send![tab_list_view, setRowHeight: SETTINGS_SIDEBAR_ROW_HEIGHT];
  let _: () = msg_send![tab_list_view, setIntercellSpacing: NSSize::new(0.0, 2.0)];
  let _: () = msg_send![tab_list_view, setBackgroundColor: NSColor::clearColor(nil)];
  let supports_style: i8 = msg_send![tab_list_view, respondsToSelector: sel!(setStyle:)];
  if supports_style != 0 {
    // NSTableViewStyleSourceList
    let _: () = msg_send![tab_list_view, setStyle: 3usize];
  } else {
    // NSTableViewSelectionHighlightStyleSourceList
    let _: () = msg_send![tab_list_view, setSelectionHighlightStyle: 1isize];
  }

  let tab_column_identifier = NSString::alloc(nil).init_str("azad-settings-tabs-column");
  let tab_column: id = msg_send![class!(NSTableColumn), alloc];
  let tab_column: id = msg_send![tab_column, initWithIdentifier: tab_column_identifier];
  let _: () = msg_send![tab_column, setWidth: SETTINGS_SIDEBAR_WIDTH];
  let _: () = msg_send![tab_column, setMinWidth: SETTINGS_SIDEBAR_WIDTH];
  let _: () = msg_send![tab_column, setMaxWidth: SETTINGS_SIDEBAR_WIDTH];
  let _: () = msg_send![tab_list_view, addTableColumn: tab_column];

  let content_origin_x = SETTINGS_SIDEBAR_WIDTH + SETTINGS_SIDEBAR_TO_CONTENT_GAP;
  let content_height = body_frame.size.height.max(220.0);
  let content_width = (body_frame.size.width - content_origin_x).max(420.0);
  let content_frame =
    NSRect::new(NSPoint::new(content_origin_x, 0.0), NSSize::new(content_width, content_height));

  let general_container: id = msg_send![class!(NSView), alloc];
  let general_container: id = msg_send![general_container, initWithFrame: content_frame];
  let _: () = msg_send![
      general_container,
      setAutoresizingMask: (NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE)
  ];
  let models_container: id = msg_send![class!(NSView), alloc];
  let models_container: id = msg_send![models_container, initWithFrame: content_frame];
  let _: () = msg_send![
      models_container,
      setAutoresizingMask: (NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE)
  ];
  let permissions_container: id = msg_send![class!(NSView), alloc];
  let permissions_container: id = msg_send![permissions_container, initWithFrame: content_frame];
  let _: () = msg_send![
      permissions_container,
      setAutoresizingMask: (NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE)
  ];
  let perm_delegate = STATUS_DELEGATE_REF.with(|slot| *slot.borrow()).unwrap_or(nil);
  let perm_top = content_height - SETTINGS_TOP_MARGIN - SETTINGS_CONTROL_HEIGHT;
  let perm_accessibility_status = make_onboarding_permission_row(
    permissions_container,
    perm_delegate,
    "Accessibility",
    perm_top,
    0,
  );
  let perm_microphone_status = make_onboarding_permission_row(
    permissions_container,
    perm_delegate,
    "Microphone",
    perm_top - 38.0,
    1,
  );
  let perm_hint_frame =
    NSRect::new(NSPoint::new(0.0, perm_top - 76.0), NSSize::new(content_width, 18.0));
  let perm_hint = make_onboarding_label(
    "Required to capture audio and insert text. Click Open Settings to grant.",
    perm_hint_frame,
    11.0,
    false,
  );
  let _: () = msg_send![permissions_container, addSubview: perm_hint];
  let debug_container: id = msg_send![class!(NSView), alloc];
  let debug_container: id = msg_send![debug_container, initWithFrame: content_frame];
  let _: () = msg_send![
      debug_container,
      setAutoresizingMask: (NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE)
  ];

  let connectors_container: id = msg_send![class!(NSView), alloc];
  let connectors_container: id = msg_send![connectors_container, initWithFrame: content_frame];
  let _: () = msg_send![
      connectors_container,
      setAutoresizingMask: (NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE)
  ];
  let connectors_top_y = content_height - SETTINGS_TOP_MARGIN - SETTINGS_CONTROL_HEIGHT;
  let connectors_hint_frame = NSRect::new(
    NSPoint::new(0.0, connectors_top_y),
    NSSize::new(content_width, SETTINGS_CONTROL_HEIGHT),
  );
  let connectors_hint: id = msg_send![class!(NSTextField), alloc];
  let connectors_hint: id = msg_send![connectors_hint, initWithFrame: connectors_hint_frame];
  let _: () = msg_send![
      connectors_hint,
      setStringValue: NSString::alloc(nil).init_str(
        "Open an utterance with a connector\u{2019}s phrase (e.g. \u{201c}hey claude\u{201d}) to tag it."
      )
  ];
  let _: () = msg_send![connectors_hint, setBezeled: NO];
  let _: () = msg_send![connectors_hint, setDrawsBackground: NO];
  let _: () = msg_send![connectors_hint, setEditable: NO];
  let _: () = msg_send![connectors_hint, setSelectable: NO];
  let _: () = msg_send![connectors_hint, setAlignment: 0isize];
  let _: () = msg_send![connectors_container, addSubview: connectors_hint];

  let connectors_checkboxes_height =
    (connectors_top_y - SETTINGS_CONTROL_VERTICAL_GAP).max(SETTINGS_CONTROL_HEIGHT);
  let connectors_checkboxes_frame =
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(content_width, connectors_checkboxes_height));
  let connectors_checkboxes_view: id = msg_send![class!(NSView), alloc];
  let connectors_checkboxes_view: id =
    msg_send![connectors_checkboxes_view, initWithFrame: connectors_checkboxes_frame];
  let _: () = msg_send![connectors_container, addSubview: connectors_checkboxes_view];

  let general_top_y = content_height - SETTINGS_TOP_MARGIN - SETTINGS_CONTROL_HEIGHT;
  let run_on_startup_frame = NSRect::new(
    NSPoint::new(0.0, general_top_y),
    NSSize::new(content_width, SETTINGS_CONTROL_HEIGHT),
  );
  let run_on_startup_checkbox: id = msg_send![class!(NSButton), alloc];
  let run_on_startup_checkbox: id =
    msg_send![run_on_startup_checkbox, initWithFrame: run_on_startup_frame];
  let _: () = msg_send![run_on_startup_checkbox, setButtonType: 3usize];
  let _: () = msg_send![
      run_on_startup_checkbox,
      setTitle: NSString::alloc(nil).init_str("Run Azad on startup")
  ];
  let _: () = msg_send![
      run_on_startup_checkbox,
      setAction: sel!(settingsToggleRunOnStartup:)
  ];
  let _: () = msg_send![general_container, addSubview: run_on_startup_checkbox];

  let paste_method_y = general_top_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let paste_method_label_frame = NSRect::new(
    NSPoint::new(0.0, paste_method_y),
    NSSize::new(SETTINGS_LABEL_WIDTH, SETTINGS_CONTROL_HEIGHT),
  );
  let paste_method_label: id = msg_send![class!(NSTextField), alloc];
  let paste_method_label: id =
    msg_send![paste_method_label, initWithFrame: paste_method_label_frame];
  let _: () =
    msg_send![paste_method_label, setStringValue: NSString::alloc(nil).init_str("Insert method")];
  let _: () = msg_send![paste_method_label, setBezeled: NO];
  let _: () = msg_send![paste_method_label, setDrawsBackground: NO];
  let _: () = msg_send![paste_method_label, setEditable: NO];
  let _: () = msg_send![paste_method_label, setSelectable: NO];
  let _: () = msg_send![paste_method_label, setAlignment: 0isize];
  let _: () = msg_send![general_container, addSubview: paste_method_label];

  let paste_method_popup_x = SETTINGS_LABEL_WIDTH + 10.0;
  let paste_method_popup_frame = NSRect::new(
    NSPoint::new(paste_method_popup_x, paste_method_y - 2.0),
    NSSize::new(SETTINGS_POPUP_WIDTH, SETTINGS_CONTROL_HEIGHT + 4.0),
  );
  let paste_method_popup: id = msg_send![class!(NSPopUpButton), alloc];
  let paste_method_popup: id =
    msg_send![paste_method_popup, initWithFrame: paste_method_popup_frame pullsDown: NO];
  let _: () =
    msg_send![paste_method_popup, addItemWithTitle: NSString::alloc(nil).init_str("Paste")];
  let _: () =
    msg_send![paste_method_popup, addItemWithTitle: NSString::alloc(nil).init_str("Direct")];
  let _: () = msg_send![paste_method_popup, addItemWithTitle: NSString::alloc(nil).init_str("Direct + copy to clipboard")];
  let _: () = msg_send![paste_method_popup, setAction: sel!(settingsSelectPasteMethod:)];
  let _: () = msg_send![general_container, addSubview: paste_method_popup];

  let auto_submit_y = paste_method_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let auto_submit_label_frame = NSRect::new(
    NSPoint::new(0.0, auto_submit_y),
    NSSize::new(SETTINGS_LABEL_WIDTH, SETTINGS_CONTROL_HEIGHT),
  );
  let auto_submit_label: id = msg_send![class!(NSTextField), alloc];
  let auto_submit_label: id = msg_send![auto_submit_label, initWithFrame: auto_submit_label_frame];
  let _: () =
    msg_send![auto_submit_label, setStringValue: NSString::alloc(nil).init_str("Auto submit")];
  let _: () = msg_send![auto_submit_label, setBezeled: NO];
  let _: () = msg_send![auto_submit_label, setDrawsBackground: NO];
  let _: () = msg_send![auto_submit_label, setEditable: NO];
  let _: () = msg_send![auto_submit_label, setSelectable: NO];
  let _: () = msg_send![auto_submit_label, setAlignment: 0isize];
  let _: () = msg_send![general_container, addSubview: auto_submit_label];

  let auto_submit_popup_frame = NSRect::new(
    NSPoint::new(paste_method_popup_x, auto_submit_y - 2.0),
    NSSize::new(SETTINGS_POPUP_WIDTH, SETTINGS_CONTROL_HEIGHT + 4.0),
  );
  let auto_submit_popup: id = msg_send![class!(NSPopUpButton), alloc];
  let auto_submit_popup: id =
    msg_send![auto_submit_popup, initWithFrame: auto_submit_popup_frame pullsDown: NO];
  let _: () = msg_send![auto_submit_popup, addItemWithTitle: NSString::alloc(nil).init_str("Off")];
  let _: () =
    msg_send![auto_submit_popup, addItemWithTitle: NSString::alloc(nil).init_str("Enter")];
  let _: () =
    msg_send![auto_submit_popup, addItemWithTitle: NSString::alloc(nil).init_str("Ctrl+Enter")];
  let _: () =
    msg_send![auto_submit_popup, addItemWithTitle: NSString::alloc(nil).init_str("Shift+Enter")];
  let _: () = msg_send![auto_submit_popup, setAction: sel!(settingsSelectAutoSubmit:)];
  let _: () = msg_send![general_container, addSubview: auto_submit_popup];

  let overlay_position_y = auto_submit_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let overlay_position_label_frame = NSRect::new(
    NSPoint::new(0.0, overlay_position_y),
    NSSize::new(SETTINGS_LABEL_WIDTH, SETTINGS_CONTROL_HEIGHT),
  );
  let overlay_position_label: id = msg_send![class!(NSTextField), alloc];
  let overlay_position_label: id =
    msg_send![overlay_position_label, initWithFrame: overlay_position_label_frame];
  let _: () = msg_send![
      overlay_position_label,
      setStringValue: NSString::alloc(nil).init_str("Overlay position")
  ];
  let _: () = msg_send![overlay_position_label, setBezeled: NO];
  let _: () = msg_send![overlay_position_label, setDrawsBackground: NO];
  let _: () = msg_send![overlay_position_label, setEditable: NO];
  let _: () = msg_send![overlay_position_label, setSelectable: NO];
  let _: () = msg_send![overlay_position_label, setAlignment: 0isize];
  let _: () = msg_send![general_container, addSubview: overlay_position_label];

  let overlay_position_popup_frame = NSRect::new(
    NSPoint::new(paste_method_popup_x, overlay_position_y - 2.0),
    NSSize::new(SETTINGS_POPUP_WIDTH, SETTINGS_CONTROL_HEIGHT + 4.0),
  );
  let overlay_position_popup: id = msg_send![class!(NSPopUpButton), alloc];
  let overlay_position_popup: id =
    msg_send![overlay_position_popup, initWithFrame: overlay_position_popup_frame pullsDown: NO];
  let _: () = msg_send![overlay_position_popup, addItemWithTitle: NSString::alloc(nil).init_str("Follow cursor")];
  let _: () = msg_send![overlay_position_popup, addItemWithTitle: NSString::alloc(nil).init_str("Primary display")];
  let _: () = msg_send![overlay_position_popup, addItemWithTitle: NSString::alloc(nil).init_str("Active window")];
  let _: () = msg_send![overlay_position_popup, setAction: sel!(settingsSelectOverlayPosition:)];
  let _: () = msg_send![general_container, addSubview: overlay_position_popup];

  let append_trailing_space_y =
    overlay_position_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let append_trailing_space_frame = NSRect::new(
    NSPoint::new(0.0, append_trailing_space_y),
    NSSize::new(content_width, SETTINGS_CONTROL_HEIGHT),
  );
  let append_trailing_space_checkbox: id = msg_send![class!(NSButton), alloc];
  let append_trailing_space_checkbox: id = msg_send![
      append_trailing_space_checkbox,
      initWithFrame: append_trailing_space_frame
  ];
  let _: () = msg_send![append_trailing_space_checkbox, setButtonType: 3usize];
  let _: () = msg_send![
      append_trailing_space_checkbox,
      setTitle: NSString::alloc(nil).init_str("Append trailing space after paste")
  ];
  let _: () = msg_send![
      append_trailing_space_checkbox,
      setAction: sel!(settingsToggleAppendTrailingSpace:)
  ];
  let _: () = msg_send![general_container, addSubview: append_trailing_space_checkbox];

  // -- Removed words --
  let removed_words_y =
    append_trailing_space_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let removed_words_label_frame = NSRect::new(
    NSPoint::new(0.0, removed_words_y),
    NSSize::new(SETTINGS_LABEL_WIDTH, SETTINGS_CONTROL_HEIGHT),
  );
  let removed_words_label: id = msg_send![class!(NSTextField), alloc];
  let removed_words_label: id =
    msg_send![removed_words_label, initWithFrame: removed_words_label_frame];
  let _: () = msg_send![
      removed_words_label,
      setStringValue: NSString::alloc(nil).init_str("Removed words")
  ];
  let _: () = msg_send![removed_words_label, setBezeled: NO];
  let _: () = msg_send![removed_words_label, setDrawsBackground: NO];
  let _: () = msg_send![removed_words_label, setEditable: NO];
  let _: () = msg_send![removed_words_label, setSelectable: NO];
  let _: () = msg_send![removed_words_label, setAlignment: 0isize];
  let _: () = msg_send![general_container, addSubview: removed_words_label];

  let tags_x = SETTINGS_LABEL_WIDTH + 10.0;
  let tags_width = content_width - tags_x;
  let removed_words_tags_frame = NSRect::new(
    NSPoint::new(tags_x, removed_words_y),
    NSSize::new(tags_width, SETTINGS_CONTROL_HEIGHT),
  );
  let removed_words_tags_view: id = msg_send![class!(NSView), alloc];
  let removed_words_tags_view: id =
    msg_send![removed_words_tags_view, initWithFrame: removed_words_tags_frame];
  let _: () = msg_send![general_container, addSubview: removed_words_tags_view];

  let input_y = removed_words_y - SETTINGS_CONTROL_HEIGHT - 6.0;
  let input_width = 160.0f64;
  let removed_words_input_frame =
    NSRect::new(NSPoint::new(tags_x, input_y), NSSize::new(input_width, SETTINGS_CONTROL_HEIGHT));
  let removed_words_input: id = msg_send![class!(NSTextField), alloc];
  let removed_words_input: id =
    msg_send![removed_words_input, initWithFrame: removed_words_input_frame];
  let _: () = msg_send![
      removed_words_input,
      setPlaceholderString: NSString::alloc(nil).init_str("Enter word")
  ];
  let _: () = msg_send![general_container, addSubview: removed_words_input];

  let add_button_frame = NSRect::new(
    NSPoint::new(tags_x + input_width + 8.0, input_y),
    NSSize::new(60.0, SETTINGS_CONTROL_HEIGHT),
  );
  let removed_words_add_button: id = msg_send![class!(NSButton), alloc];
  let removed_words_add_button: id =
    msg_send![removed_words_add_button, initWithFrame: add_button_frame];
  let _: () = msg_send![removed_words_add_button, setBezelStyle: 1usize];
  let _: () = msg_send![
      removed_words_add_button,
      setTitle: NSString::alloc(nil).init_str("Add")
  ];
  let _: () = msg_send![removed_words_add_button, setAction: sel!(settingsAddRemovedWord:)];
  let _: () = msg_send![general_container, addSubview: removed_words_add_button];

  // -- Models tab content --
  let models_top_y = content_height - SETTINGS_TOP_MARGIN - SETTINGS_CONTROL_HEIGHT;

  let models_name_frame = NSRect::new(
    NSPoint::new(0.0, models_top_y),
    NSSize::new(content_width, SETTINGS_CONTROL_HEIGHT),
  );
  let models_name_label: id = msg_send![class!(NSTextField), alloc];
  let models_name_label: id = msg_send![models_name_label, initWithFrame: models_name_frame];
  let _: () = msg_send![
      models_name_label,
      setStringValue: NSString::alloc(nil).init_str("Parakeet v1")
  ];
  let _: () = msg_send![models_name_label, setBezeled: NO];
  let _: () = msg_send![models_name_label, setDrawsBackground: NO];
  let _: () = msg_send![models_name_label, setEditable: NO];
  let _: () = msg_send![models_name_label, setSelectable: NO];
  let bold_font: id = msg_send![class!(NSFont), boldSystemFontOfSize: 14.0f64];
  if bold_font != nil {
    let _: () = msg_send![models_name_label, setFont: bold_font];
  }
  let _: () = msg_send![models_container, addSubview: models_name_label];

  let models_desc_y = models_top_y - SETTINGS_CONTROL_HEIGHT - 4.0;
  let models_desc_frame = NSRect::new(
    NSPoint::new(0.0, models_desc_y),
    NSSize::new(content_width, SETTINGS_CONTROL_HEIGHT),
  );
  let models_desc_label: id = msg_send![class!(NSTextField), alloc];
  let models_desc_label: id = msg_send![models_desc_label, initWithFrame: models_desc_frame];
  let _: () = msg_send![
      models_desc_label,
      setStringValue: NSString::alloc(nil).init_str("Silero VAD + Parakeet streaming/finalization ASR")
  ];
  let _: () = msg_send![models_desc_label, setBezeled: NO];
  let _: () = msg_send![models_desc_label, setDrawsBackground: NO];
  let _: () = msg_send![models_desc_label, setEditable: NO];
  let _: () = msg_send![models_desc_label, setSelectable: NO];
  let secondary_color: id = msg_send![class!(NSColor), secondaryLabelColor];
  let _: () = msg_send![models_desc_label, setTextColor: secondary_color];
  let _: () = msg_send![models_container, addSubview: models_desc_label];

  let models_status_y = models_desc_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let models_status_frame = NSRect::new(
    NSPoint::new(0.0, models_status_y),
    NSSize::new(content_width, SETTINGS_CONTROL_HEIGHT),
  );
  let models_status_label: id = msg_send![class!(NSTextField), alloc];
  let models_status_label: id = msg_send![models_status_label, initWithFrame: models_status_frame];
  let _: () = msg_send![
      models_status_label,
      setStringValue: NSString::alloc(nil).init_str("Checking...")
  ];
  let _: () = msg_send![models_status_label, setBezeled: NO];
  let _: () = msg_send![models_status_label, setDrawsBackground: NO];
  let _: () = msg_send![models_status_label, setEditable: NO];
  let _: () = msg_send![models_status_label, setSelectable: NO];
  let _: () = msg_send![models_container, addSubview: models_status_label];

  let models_progress_y = models_status_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let models_progress_frame =
    NSRect::new(NSPoint::new(0.0, models_progress_y + 4.0), NSSize::new(content_width, 16.0));
  let models_progress_indicator: id = msg_send![class!(NSProgressIndicator), alloc];
  let models_progress_indicator: id =
    msg_send![models_progress_indicator, initWithFrame: models_progress_frame];
  let _: () = msg_send![models_progress_indicator, setStyle: 0isize]; // NSProgressIndicatorStyleBar
  let _: () = msg_send![models_progress_indicator, setIndeterminate: NO];
  let _: () = msg_send![models_progress_indicator, setMinValue: 0.0f64];
  let _: () = msg_send![models_progress_indicator, setMaxValue: 100.0f64];
  let _: () = msg_send![models_progress_indicator, setDoubleValue: 0.0f64];
  let _: () = msg_send![models_progress_indicator, setHidden: YES];
  let _: () = msg_send![models_container, addSubview: models_progress_indicator];

  let models_button_y = models_progress_y - SETTINGS_CONTROL_HEIGHT - SETTINGS_CONTROL_VERTICAL_GAP;
  let models_download_frame =
    NSRect::new(NSPoint::new(0.0, models_button_y), NSSize::new(120.0, SETTINGS_CONTROL_HEIGHT));
  let models_download_button: id = msg_send![class!(NSButton), alloc];
  let models_download_button: id =
    msg_send![models_download_button, initWithFrame: models_download_frame];
  let _: () = msg_send![models_download_button, setBezelStyle: 1usize];
  let _: () = msg_send![
      models_download_button,
      setTitle: NSString::alloc(nil).init_str("Download")
  ];
  let _: () = msg_send![models_download_button, setAction: sel!(settingsDownloadModel:)];
  let _: () = msg_send![models_download_button, setTag: 0isize];
  let _: () = msg_send![models_container, addSubview: models_download_button];

  let models_cancel_frame =
    NSRect::new(NSPoint::new(130.0, models_button_y), NSSize::new(90.0, SETTINGS_CONTROL_HEIGHT));
  let models_cancel_button: id = msg_send![class!(NSButton), alloc];
  let models_cancel_button: id =
    msg_send![models_cancel_button, initWithFrame: models_cancel_frame];
  let _: () = msg_send![models_cancel_button, setBezelStyle: 1usize];
  let _: () = msg_send![
      models_cancel_button,
      setTitle: NSString::alloc(nil).init_str("Cancel")
  ];
  let _: () = msg_send![models_cancel_button, setAction: sel!(settingsCancelDownload:)];
  let _: () = msg_send![models_cancel_button, setHidden: YES];
  let _: () = msg_send![models_container, addSubview: models_cancel_button];

  // -- Debug tab content --
  let debug_top_y = content_height - SETTINGS_TOP_MARGIN - SETTINGS_CONTROL_HEIGHT;
  let debug_checkbox_frame =
    NSRect::new(NSPoint::new(0.0, debug_top_y), NSSize::new(320.0, SETTINGS_CONTROL_HEIGHT));
  let debug_checkbox: id = msg_send![class!(NSButton), alloc];
  let debug_checkbox: id = msg_send![debug_checkbox, initWithFrame: debug_checkbox_frame];
  let _: () = msg_send![debug_checkbox, setButtonType: 3usize];
  let _: () = msg_send![
      debug_checkbox,
      setTitle: NSString::alloc(nil).init_str("Enable debug statistics")
  ];
  let _: () = msg_send![debug_checkbox, setAction: sel!(settingsToggleDebug:)];

  let refresh_x = content_width - SETTINGS_REFRESH_WIDTH;
  let refresh_frame = NSRect::new(
    NSPoint::new(refresh_x, debug_top_y),
    NSSize::new(SETTINGS_REFRESH_WIDTH, SETTINGS_CONTROL_HEIGHT),
  );
  let refresh_button: id = msg_send![class!(NSButton), alloc];
  let refresh_button: id = msg_send![refresh_button, initWithFrame: refresh_frame];
  let _: () = msg_send![refresh_button, setBezelStyle: 1usize];
  let _: () = msg_send![refresh_button, setTitle: NSString::alloc(nil).init_str("Refresh")];
  let _: () = msg_send![refresh_button, setAction: sel!(settingsRefresh:)];

  let metrics_height = (debug_top_y - SETTINGS_METRICS_TOP_GAP).max(SETTINGS_CONTROL_HEIGHT * 2.0);
  let scroll_frame =
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(content_width, metrics_height));
  let scroll_view: id = msg_send![class!(NSScrollView), alloc];
  let scroll_view: id = msg_send![scroll_view, initWithFrame: scroll_frame];
  let _: () = msg_send![scroll_view, setHasVerticalScroller: YES];

  let text_frame = NSRect::new(
    NSPoint::new(0.0, 0.0),
    NSSize::new(scroll_frame.size.width, scroll_frame.size.height),
  );
  let metrics_text_view: id = msg_send![class!(NSTextView), alloc];
  let metrics_text_view: id = msg_send![metrics_text_view, initWithFrame: text_frame];
  let _: () = msg_send![metrics_text_view, setEditable: NO];
  let _: () = msg_send![metrics_text_view, setSelectable: YES];
  let _: () = msg_send![metrics_text_view, setRichText: NO];
  let mono_font: id = msg_send![class!(NSFont), userFixedPitchFontOfSize: 12.0f64];
  if mono_font != nil {
    let _: () = msg_send![metrics_text_view, setFont: mono_font];
  }
  let _: () = msg_send![metrics_text_view, setString: NSString::alloc(nil).init_str("")];
  let _: () = msg_send![scroll_view, setDocumentView: metrics_text_view];

  if let Some(delegate) = STATUS_DELEGATE_REF.with(|slot| *slot.borrow()) {
    let _: () = msg_send![window, setDelegate: delegate];
    let _: () = msg_send![tab_list_view, setDelegate: delegate];
    let _: () = msg_send![tab_list_view, setDataSource: delegate];
    let _: () = msg_send![run_on_startup_checkbox, setTarget: delegate];
    let _: () = msg_send![paste_method_popup, setTarget: delegate];
    let _: () = msg_send![auto_submit_popup, setTarget: delegate];
    let _: () = msg_send![overlay_position_popup, setTarget: delegate];
    let _: () = msg_send![append_trailing_space_checkbox, setTarget: delegate];
    let _: () = msg_send![removed_words_add_button, setTarget: delegate];
    let _: () = msg_send![models_download_button, setTarget: delegate];
    let _: () = msg_send![models_cancel_button, setTarget: delegate];
    let _: () = msg_send![debug_checkbox, setTarget: delegate];
    let _: () = msg_send![refresh_button, setTarget: delegate];
  }

  let _: () = msg_send![sidebar_scroll, setDocumentView: tab_list_view];
  let _: () = msg_send![body_view, addSubview: general_container];
  let _: () = msg_send![body_view, addSubview: models_container];
  let _: () = msg_send![body_view, addSubview: permissions_container];
  let _: () = msg_send![debug_container, addSubview: debug_checkbox];
  let _: () = msg_send![debug_container, addSubview: refresh_button];
  let _: () = msg_send![debug_container, addSubview: scroll_view];
  let _: () = msg_send![body_view, addSubview: debug_container];
  let _: () = msg_send![body_view, addSubview: connectors_container];
  let _: () = msg_send![tab_list_view, reloadData];

  // Build-info footer (bottom-right corner of the window). Tiny dim text so
  // it sits visually behind the controls, selectable so the user can copy
  // the git SHA when filing reports. Values come from `build.rs` via
  // `cargo:rustc-env`. Anchored in `content_view` (not `body_view`) so it
  // sits in the window's bottom inset margin and doesn't compete for space
  // with the tab content.
  let build_info_text = format!("{} · {}", env!("AZAD_BUILD_GIT_SHA"), env!("AZAD_BUILD_TIME"));
  let build_info_str = NSString::alloc(nil).init_str(&build_info_text);
  let build_info_label: id = msg_send![class!(NSTextField), alloc];
  let build_info_w: f64 = 280.0;
  let build_info_h: f64 = 14.0;
  let build_info_frame = NSRect::new(
    NSPoint::new(SETTINGS_WINDOW_WIDTH - build_info_w - 8.0, 4.0),
    NSSize::new(build_info_w, build_info_h),
  );
  let build_info_label: id = msg_send![build_info_label, initWithFrame: build_info_frame];
  let _: () = msg_send![build_info_label, setStringValue: build_info_str];
  let _: () = msg_send![build_info_label, setBezeled: NO];
  let _: () = msg_send![build_info_label, setDrawsBackground: NO];
  let _: () = msg_send![build_info_label, setEditable: NO];
  let _: () = msg_send![build_info_label, setSelectable: YES];
  let _: () = msg_send![build_info_label, setAlignment: 2isize]; // right
  let build_info_font: id = msg_send![class!(NSFont), systemFontOfSize: 10.0f64];
  let _: () = msg_send![build_info_label, setFont: build_info_font];
  let build_info_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.5, 0.5, 0.5, 1.0);
  let _: () = msg_send![build_info_label, setTextColor: build_info_color];
  let _: () = msg_send![
      build_info_label,
      setAutoresizingMask: NS_VIEW_MIN_X_MARGIN | NS_VIEW_MAX_Y_MARGIN
  ];
  let _: () = msg_send![content_view, addSubview: build_info_label];

  let refs = SettingsWindowRefs {
    window,
    tab_list_view,
    general_container,
    models_container,
    permissions_container,
    perm_accessibility_status,
    perm_microphone_status,
    debug_container,
    connectors_container,
    connectors_checkboxes_view,
    run_on_startup_checkbox,
    paste_method_popup,
    auto_submit_popup,
    overlay_position_popup,
    append_trailing_space_checkbox,
    removed_words_tags_view,
    removed_words_input,
    removed_words_add_button,
    debug_checkbox,
    metrics_text_view,
    models_status_label,
    models_progress_indicator,
    models_download_button,
    models_cancel_button,
  };
  apply_settings_selected_tab(refs, SettingsTab::General);
  refs
}

unsafe fn create_overlay_window(read_only: bool) -> OverlayRefs {
  let frame = cursor_screen_frame().unwrap_or_else(main_screen_frame);

  let overlay_width = overlay_width_for_screen(frame);
  let overlay_height = OVERLAY_HEIGHT_MIN;
  let x = frame.origin.x + (frame.size.width - overlay_width) * 0.5;
  let y = frame.origin.y + frame.size.height * 0.08;

  let overlay_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(overlay_width, overlay_height));

  let overlay_class = register_overlay_window_class();
  let window: id = msg_send![overlay_class, alloc];
  // NSWindowStyleMaskNonactivatingPanel = 1 << 7. Combined with borderless,
  // lets the panel become key without activating Azad.
  let style_mask: u64 = NSWindowStyleMask::NSBorderlessWindowMask.bits() | (1u64 << 7);
  let window: id = msg_send![window, initWithContentRect: overlay_frame
                                                styleMask: style_mask
                                                  backing: NSBackingStoreType::NSBackingStoreBuffered
                                                    defer: NO];

  window.setReleasedWhenClosed_(NO);
  window.setOpaque_(NO);
  window.setHasShadow_(YES);
  window.setIgnoresMouseEvents_(if read_only { YES } else { NO });
  window.setMovableByWindowBackground_(if read_only { NO } else { YES });
  window.setLevel_((NSMainMenuWindowLevel + 1) as i64);
  window.setCollectionBehavior_(
    NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
      | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary,
  );
  window.setBackgroundColor_(NSColor::clearColor(nil));

  let card_view: id = msg_send![class!(NSView), alloc];
  let card_view: id = msg_send![card_view, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(overlay_width, overlay_height)
  )];
  let _: () = msg_send![card_view, setWantsLayer: YES];
  let card_layer: id = msg_send![card_view, layer];
  let card_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.02, 0.02, 0.02, 0.9);
  let cg_color: id = msg_send![card_color, CGColor];
  let _: () = msg_send![card_layer, setBackgroundColor: cg_color];
  let _: () = msg_send![card_layer, setCornerRadius: OVERLAY_CARD_RADIUS];
  let _: () = msg_send![card_layer, setMasksToBounds: YES];
  let subtle_border =
    NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 0.62, 0.74, 0.98, 0.20);
  let subtle_border_cg: id = msg_send![subtle_border, CGColor];
  let _: () = msg_send![card_layer, setBorderWidth: OVERLAY_BORDER_THICKNESS];
  let _: () = msg_send![card_layer, setBorderColor: subtle_border_cg];
  window.setContentView_(card_view);

  let label_frame = NSRect::new(
    NSPoint::new(OVERLAY_PAD_X, OVERLAY_PAD_BOTTOM),
    NSSize::new(overlay_width - OVERLAY_PAD_X * 2.0, 1.0),
  );

  let meter_view: id = msg_send![class!(NSView), alloc];
  let meter_view: id = msg_send![meter_view, initWithFrame: label_frame];
  let _: () = msg_send![meter_view, setWantsLayer: YES];
  let _: () = msg_send![card_view, addSubview: meter_view];

  let mut wave_bars = [nil; OVERLAY_WAVE_BAR_COUNT];
  for bar in &mut wave_bars {
    let v: id = msg_send![class!(NSView), alloc];
    let v: id = msg_send![v, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(1.0, 1.0)
    )];
    let _: () = msg_send![v, setWantsLayer: YES];
    let _: () = msg_send![v, setHidden: YES];
    let _: () = msg_send![meter_view, addSubview: v];
    *bar = v;
  }

  let label: id = msg_send![class!(NSTextField), alloc];
  let label: id = msg_send![label, initWithFrame: label_frame];
  let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str("")];
  let _: () = msg_send![label, setBezeled: NO];
  let _: () = msg_send![label, setDrawsBackground: NO];
  let _: () = msg_send![label, setEditable: NO];
  let _: () = msg_send![label, setSelectable: NO];
  let _: () = msg_send![label, setAlignment: 1isize];
  let _: () = msg_send![label, setLineBreakMode: 0isize];
  let _: () = msg_send![label, setUsesSingleLineMode: NO];
  let _: () = msg_send![label, setMaximumNumberOfLines: 0isize];
  let font: id = msg_send![class!(NSFont), systemFontOfSize: OVERLAY_TEXT_FONT_SIZE];
  let _: () = msg_send![label, setFont: font];
  let text_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
  let _: () = msg_send![label, setTextColor: text_color];
  let _: () = msg_send![card_view, addSubview: label];

  let raw_badge: id = msg_send![class!(NSTextField), alloc];
  let raw_badge: id = msg_send![raw_badge, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_RAW_BADGE_WIDTH, OVERLAY_RAW_BADGE_HEIGHT)
  )];
  let _: () = msg_send![raw_badge, setStringValue: NSString::alloc(nil).init_str("raw")];
  let _: () = msg_send![raw_badge, setBezeled: NO];
  let _: () = msg_send![raw_badge, setDrawsBackground: NO];
  let _: () = msg_send![raw_badge, setEditable: NO];
  let _: () = msg_send![raw_badge, setSelectable: NO];
  let _: () = msg_send![raw_badge, setUsesSingleLineMode: YES];
  let _: () = msg_send![raw_badge, setLineBreakMode: 2isize];
  let _: () = msg_send![raw_badge, setAlignment: 2isize];
  let raw_font: id = msg_send![class!(NSFont), systemFontOfSize: OVERLAY_RAW_BADGE_FONT_SIZE];
  let _: () = msg_send![raw_badge, setFont: raw_font];
  let raw_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.48);
  let _: () = msg_send![raw_badge, setTextColor: raw_color];
  let _: () = msg_send![raw_badge, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: raw_badge];

  let hold_badge: id = msg_send![class!(NSTextField), alloc];
  let hold_badge: id = msg_send![hold_badge, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_HOLD_BADGE_WIDTH, OVERLAY_HOLD_BADGE_HEIGHT)
  )];
  let _: () = msg_send![hold_badge, setStringValue: NSString::alloc(nil).init_str("hold")];
  let _: () = msg_send![hold_badge, setBezeled: NO];
  let _: () = msg_send![hold_badge, setDrawsBackground: NO];
  let _: () = msg_send![hold_badge, setEditable: NO];
  let _: () = msg_send![hold_badge, setSelectable: NO];
  let _: () = msg_send![hold_badge, setUsesSingleLineMode: YES];
  let _: () = msg_send![hold_badge, setLineBreakMode: 2isize];
  let _: () = msg_send![hold_badge, setAlignment: 2isize];
  let hold_font: id = msg_send![class!(NSFont), systemFontOfSize: OVERLAY_RAW_BADGE_FONT_SIZE];
  let _: () = msg_send![hold_badge, setFont: hold_font];
  let hold_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 0.58, 0.22, 0.72);
  let _: () = msg_send![hold_badge, setTextColor: hold_color];
  let _: () = msg_send![hold_badge, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: hold_badge];

  // Connector tag chip — a card-level subview (like the raw/hold badges) so it
  // survives `hide_overlay_notice_accessory`, which the AUTO ON chip does not.
  let connector_chip: id = msg_send![class!(NSView), alloc];
  let connector_chip: id = msg_send![connector_chip, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(0.0, OVERLAY_CONNECTOR_CHIP_HEIGHT)
  )];
  let _: () = msg_send![connector_chip, setWantsLayer: YES];
  let connector_chip_layer: id = msg_send![connector_chip, layer];
  if connector_chip_layer != nil {
    let chip_bg = NSColor::colorWithCalibratedRed_green_blue_alpha_(
      nil,
      CLAUDE_BRAND_R,
      CLAUDE_BRAND_G,
      CLAUDE_BRAND_B,
      0.90,
    );
    let chip_bg_cg: id = msg_send![chip_bg, CGColor];
    let _: () = msg_send![connector_chip_layer, setBackgroundColor: chip_bg_cg];
    let _: () = msg_send![connector_chip_layer, setCornerRadius: OVERLAY_CONNECTOR_CHIP_RADIUS];
  }
  let _: () = msg_send![connector_chip, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: connector_chip];

  let connector_chip_label: id = msg_send![class!(NSTextField), alloc];
  let connector_chip_label: id = msg_send![connector_chip_label, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(0.0, OVERLAY_CONNECTOR_CHIP_HEIGHT)
  )];
  let _: () = msg_send![connector_chip_label, setBezeled: NO];
  let _: () = msg_send![connector_chip_label, setDrawsBackground: NO];
  let _: () = msg_send![connector_chip_label, setEditable: NO];
  let _: () = msg_send![connector_chip_label, setSelectable: NO];
  let _: () = msg_send![connector_chip_label, setAlignment: 1isize];
  let _: () = msg_send![connector_chip_label, setUsesSingleLineMode: YES];
  let connector_chip_font: id =
    msg_send![class!(NSFont), boldSystemFontOfSize: OVERLAY_CONNECTOR_CHIP_FONT_SIZE];
  let _: () = msg_send![connector_chip_label, setFont: connector_chip_font];
  let connector_chip_text =
    NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
  let _: () = msg_send![connector_chip_label, setTextColor: connector_chip_text];
  let _: () = msg_send![connector_chip_label, setHidden: YES];
  let _: () = msg_send![connector_chip, addSubview: connector_chip_label];

  let connector_chip_icon: id = msg_send![class!(NSImageView), alloc];
  let connector_chip_icon: id = msg_send![connector_chip_icon, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_CONNECTOR_CHIP_ICON_SIZE, OVERLAY_CONNECTOR_CHIP_ICON_SIZE)
  )];
  let _: () = msg_send![connector_chip_icon, setImageScaling: 3isize];
  let _: () = msg_send![connector_chip_icon, setHidden: YES];
  let _: () = msg_send![connector_chip, addSubview: connector_chip_icon];

  // Conversation-mode views: the user's query (white), a divider, the streaming reply
  // (terracotta), and a status/error line. All card-level subviews, started hidden.
  let conv_text_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
  let conv_body_font: id = msg_send![class!(NSFont), systemFontOfSize: OVERLAY_TEXT_FONT_SIZE];

  let conv_query_label: id = msg_send![class!(NSTextField), alloc];
  let conv_query_label: id = msg_send![conv_query_label, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, OVERLAY_TEXT_LINE_HEIGHT)
  )];
  let _: () = msg_send![conv_query_label, setStringValue: NSString::alloc(nil).init_str("")];
  let _: () = msg_send![conv_query_label, setBezeled: NO];
  let _: () = msg_send![conv_query_label, setDrawsBackground: NO];
  let _: () = msg_send![conv_query_label, setEditable: NO];
  let _: () = msg_send![conv_query_label, setSelectable: NO];
  let _: () = msg_send![conv_query_label, setAlignment: 0isize];
  let _: () = msg_send![conv_query_label, setLineBreakMode: 0isize];
  let _: () = msg_send![conv_query_label, setUsesSingleLineMode: NO];
  let _: () = msg_send![conv_query_label, setMaximumNumberOfLines: 0isize];
  let _: () = msg_send![conv_query_label, setFont: conv_body_font];
  let _: () = msg_send![conv_query_label, setTextColor: conv_text_color];
  let _: () = msg_send![conv_query_label, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: conv_query_label];

  let conv_divider: id = msg_send![class!(NSView), alloc];
  let conv_divider: id = msg_send![conv_divider, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, OVERLAY_CONV_DIVIDER_THICKNESS)
  )];
  let _: () = msg_send![conv_divider, setWantsLayer: YES];
  let conv_divider_layer: id = msg_send![conv_divider, layer];
  if conv_divider_layer != nil {
    let divider_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
      nil,
      1.0,
      1.0,
      1.0,
      OVERLAY_CONV_DIVIDER_ALPHA,
    );
    let divider_cg: id = msg_send![divider_color, CGColor];
    let _: () = msg_send![conv_divider_layer, setBackgroundColor: divider_cg];
  }
  let _: () = msg_send![conv_divider, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: conv_divider];

  let conv_reply_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
    nil,
    CLAUDE_BRAND_R,
    CLAUDE_BRAND_G,
    CLAUDE_BRAND_B,
    1.0,
  );
  let conv_reply_scroll: id = msg_send![class!(NSScrollView), alloc];
  let conv_reply_scroll: id = msg_send![conv_reply_scroll, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, OVERLAY_TEXT_LINE_HEIGHT)
  )];
  let _: () = msg_send![conv_reply_scroll, setDrawsBackground: NO];
  let _: () = msg_send![conv_reply_scroll, setBorderType: 0isize]; // NSNoBorder
  let _: () = msg_send![conv_reply_scroll, setHasVerticalScroller: YES];
  let _: () = msg_send![conv_reply_scroll, setHasHorizontalScroller: NO];
  let _: () = msg_send![conv_reply_scroll, setAutohidesScrollers: YES];
  let _: () = msg_send![conv_reply_scroll, setScrollerStyle: 1isize]; // NSScrollerStyleOverlay
  let _: () = msg_send![conv_reply_scroll, setHidden: YES];

  let conv_reply_text: id = msg_send![class!(NSTextView), alloc];
  let conv_reply_text: id = msg_send![conv_reply_text, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, OVERLAY_TEXT_LINE_HEIGHT)
  )];
  let _: () = msg_send![conv_reply_text, setEditable: NO];
  let _: () = msg_send![conv_reply_text, setSelectable: NO];
  let _: () = msg_send![conv_reply_text, setRichText: NO];
  let _: () = msg_send![conv_reply_text, setDrawsBackground: NO];
  let _: () = msg_send![conv_reply_text, setFont: conv_body_font];
  let _: () = msg_send![conv_reply_text, setTextColor: conv_reply_color];
  let _: () = msg_send![conv_reply_text, setTextContainerInset: NSSize::new(0.0, 2.0)];
  let _: () = msg_send![conv_reply_text, setVerticallyResizable: YES];
  let _: () = msg_send![conv_reply_text, setHorizontallyResizable: NO];
  let _: () = msg_send![conv_reply_text, setMinSize: NSSize::new(0.0, 0.0)];
  let _: () = msg_send![conv_reply_text, setMaxSize: NSSize::new(10_000_000.0, 10_000_000.0)];
  let reply_container: id = msg_send![conv_reply_text, textContainer];
  if reply_container != nil {
    let _: () = msg_send![reply_container, setLineFragmentPadding: 0.0f64];
    let _: () = msg_send![reply_container, setWidthTracksTextView: NO];
  }
  let _: () = msg_send![conv_reply_scroll, setDocumentView: conv_reply_text];
  let _: () = msg_send![card_view, addSubview: conv_reply_scroll];

  let conv_status_label: id = msg_send![class!(NSTextField), alloc];
  let conv_status_label: id = msg_send![conv_status_label, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, OVERLAY_TEXT_LINE_HEIGHT)
  )];
  let _: () = msg_send![conv_status_label, setStringValue: NSString::alloc(nil).init_str("")];
  let _: () = msg_send![conv_status_label, setBezeled: NO];
  let _: () = msg_send![conv_status_label, setDrawsBackground: NO];
  let _: () = msg_send![conv_status_label, setEditable: NO];
  let _: () = msg_send![conv_status_label, setSelectable: NO];
  let _: () = msg_send![conv_status_label, setAlignment: 0isize];
  let _: () = msg_send![conv_status_label, setLineBreakMode: 0isize];
  let _: () = msg_send![conv_status_label, setUsesSingleLineMode: NO];
  let _: () = msg_send![conv_status_label, setMaximumNumberOfLines: 0isize];
  let conv_status_font: id =
    msg_send![class!(NSFont), systemFontOfSize: OVERLAY_CONV_STATUS_FONT_SIZE];
  let _: () = msg_send![conv_status_label, setFont: conv_status_font];
  let _: () = msg_send![conv_status_label, setTextColor: conv_reply_color];
  let _: () = msg_send![conv_status_label, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: conv_status_label];

  // Inline autocomplete: separator + rows
  let autocomplete_separator: id = msg_send![class!(NSView), alloc];
  let autocomplete_separator: id = msg_send![autocomplete_separator, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, AUTOCOMPLETE_SEPARATOR_HEIGHT)
  )];
  let _: () = msg_send![autocomplete_separator, setWantsLayer: YES];
  let sep_layer: id = msg_send![autocomplete_separator, layer];
  if sep_layer != nil {
    let sep_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.15);
    let sep_cg: id = msg_send![sep_color, CGColor];
    let _: () = msg_send![sep_layer, setBackgroundColor: sep_cg];
  }
  let _: () = msg_send![autocomplete_separator, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: autocomplete_separator];

  let mut autocomplete_labels = [nil; AUTOCOMPLETE_MAX_ITEMS];
  let mut autocomplete_bgs = [nil; AUTOCOMPLETE_MAX_ITEMS];
  let mut autocomplete_expand_markers = [nil; AUTOCOMPLETE_MAX_ITEMS];
  let mut autocomplete_ts_labels = [nil; AUTOCOMPLETE_MAX_ITEMS];
  let mut autocomplete_char_count_labels = [nil; AUTOCOMPLETE_MAX_ITEMS];
  let ac_font: id = msg_send![class!(NSFont), systemFontOfSize: 14.0f64];
  let expand_marker_font: id =
    msg_send![class!(NSFont), systemFontOfSize: HISTORY_EXPAND_MARKER_FONT_SIZE];
  let expand_marker_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
    nil,
    1.0,
    1.0,
    1.0,
    HISTORY_EXPAND_MARKER_ALPHA,
  );
  let ts_font: id = msg_send![class!(NSFont), systemFontOfSize: HISTORY_TS_FONT_SIZE];
  let ts_color =
    NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, HISTORY_TS_TEXT_ALPHA);
  for i in 0..AUTOCOMPLETE_MAX_ITEMS {
    let bg_view: id = msg_send![class!(NSView), alloc];
    let bg_view: id = msg_send![bg_view, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(1.0, AUTOCOMPLETE_ROW_HEIGHT)
    )];
    let _: () = msg_send![bg_view, setWantsLayer: YES];
    let bg_layer: id = msg_send![bg_view, layer];
    if bg_layer != nil {
      let bg_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
        nil,
        1.0,
        1.0,
        1.0,
        AUTOCOMPLETE_FOCUSED_BG_ALPHA,
      );
      let bg_cg: id = msg_send![bg_color, CGColor];
      let _: () = msg_send![bg_layer, setBackgroundColor: bg_cg];
      let _: () = msg_send![bg_layer, setCornerRadius: 4.0f64];
    }
    let _: () = msg_send![bg_view, setHidden: YES];
    let _: () = msg_send![card_view, addSubview: bg_view];
    autocomplete_bgs[i] = bg_view;

    let ac_label: id = msg_send![class!(NSTextField), alloc];
    let ac_label: id = msg_send![ac_label, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(1.0, AUTOCOMPLETE_ROW_HEIGHT)
    )];
    let _: () = msg_send![ac_label, setStringValue: NSString::alloc(nil).init_str("")];
    let _: () = msg_send![ac_label, setBezeled: NO];
    let _: () = msg_send![ac_label, setDrawsBackground: NO];
    let _: () = msg_send![ac_label, setEditable: NO];
    let _: () = msg_send![ac_label, setSelectable: NO];
    let _: () = msg_send![ac_label, setAlignment: 0isize];
    let _: () = msg_send![ac_label, setLineBreakMode: 5isize];
    let _: () = msg_send![ac_label, setUsesSingleLineMode: YES];
    if ac_font != nil {
      let _: () = msg_send![ac_label, setFont: ac_font];
    }
    let ac_text_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(
      nil,
      1.0,
      1.0,
      1.0,
      AUTOCOMPLETE_TEXT_ALPHA,
    );
    let _: () = msg_send![ac_label, setTextColor: ac_text_color];
    let _: () = msg_send![ac_label, setHidden: YES];
    let _: () = msg_send![card_view, addSubview: ac_label];
    autocomplete_labels[i] = ac_label;

    let marker: id = msg_send![class!(NSTextField), alloc];
    let marker: id = msg_send![marker, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(HISTORY_EXPAND_MARKER_WIDTH, HISTORY_EXPAND_MARKER_FONT_SIZE + 4.0)
    )];
    let _: () = msg_send![marker, setBezeled: NO];
    let _: () = msg_send![marker, setDrawsBackground: NO];
    let _: () = msg_send![marker, setEditable: NO];
    let _: () = msg_send![marker, setSelectable: NO];
    let _: () = msg_send![marker, setUsesSingleLineMode: YES];
    let _: () = msg_send![marker, setAlignment: 1isize]; // center
    if expand_marker_font != nil {
      let _: () = msg_send![marker, setFont: expand_marker_font];
    }
    let _: () = msg_send![marker, setTextColor: expand_marker_color];
    let _: () = msg_send![marker, setStringValue: NSString::alloc(nil).init_str("\u{25B6}")];
    let _: () = msg_send![marker, setHidden: YES];
    let _: () = msg_send![card_view, addSubview: marker];
    autocomplete_expand_markers[i] = marker;

    let ts_label: id = msg_send![class!(NSTextField), alloc];
    let ts_label: id = msg_send![ts_label, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(HISTORY_TS_WIDTH, HISTORY_TS_FONT_SIZE + 4.0)
    )];
    let _: () = msg_send![ts_label, setBezeled: NO];
    let _: () = msg_send![ts_label, setDrawsBackground: NO];
    let _: () = msg_send![ts_label, setEditable: NO];
    let _: () = msg_send![ts_label, setSelectable: NO];
    let _: () = msg_send![ts_label, setUsesSingleLineMode: YES];
    let _: () = msg_send![ts_label, setAlignment: 2isize]; // right
    let _: () = msg_send![ts_label, setLineBreakMode: 4isize]; // truncate tail
    if ts_font != nil {
      let _: () = msg_send![ts_label, setFont: ts_font];
    }
    let _: () = msg_send![ts_label, setTextColor: ts_color];
    let _: () = msg_send![ts_label, setHidden: YES];
    let _: () = msg_send![card_view, addSubview: ts_label];
    autocomplete_ts_labels[i] = ts_label;

    // Char-count label: same column / font / colour as the timestamp,
    // stacked above it during render. We reuse the timestamp's font and
    // colour objects for parity (single source of visual truth).
    let cc_label: id = msg_send![class!(NSTextField), alloc];
    let cc_label: id = msg_send![cc_label, initWithFrame: NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(HISTORY_CHAR_COUNT_WIDTH, HISTORY_CHAR_COUNT_FONT_SIZE + 4.0)
    )];
    let _: () = msg_send![cc_label, setBezeled: NO];
    let _: () = msg_send![cc_label, setDrawsBackground: NO];
    let _: () = msg_send![cc_label, setEditable: NO];
    let _: () = msg_send![cc_label, setSelectable: NO];
    let _: () = msg_send![cc_label, setUsesSingleLineMode: YES];
    let _: () = msg_send![cc_label, setAlignment: 2isize]; // right
    let _: () = msg_send![cc_label, setLineBreakMode: 4isize]; // truncate tail
    if ts_font != nil {
      let _: () = msg_send![cc_label, setFont: ts_font];
    }
    let _: () = msg_send![cc_label, setTextColor: ts_color];
    let _: () = msg_send![cc_label, setHidden: YES];
    let _: () = msg_send![card_view, addSubview: cc_label];
    autocomplete_char_count_labels[i] = cc_label;
  }

  let notice_accessory_row: id = msg_send![class!(NSView), alloc];
  let notice_accessory_row: id = msg_send![notice_accessory_row, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, OVERLAY_NOTICE_KEYCAP_HEIGHT)
  )];
  let _: () = msg_send![notice_accessory_row, setWantsLayer: YES];
  let _: () = msg_send![notice_accessory_row, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: notice_accessory_row];

  let notice_option_key: id = msg_send![class!(NSView), alloc];
  let notice_option_key: id = msg_send![notice_option_key, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_NOTICE_KEYCAP_OPTION_WIDTH, OVERLAY_NOTICE_KEYCAP_HEIGHT)
  )];
  let _: () = msg_send![notice_option_key, setWantsLayer: YES];
  let _: () = msg_send![notice_option_key, setHidden: YES];
  let _: () = msg_send![notice_accessory_row, addSubview: notice_option_key];

  let notice_option_label: id = msg_send![class!(NSTextField), alloc];
  let notice_option_label: id = msg_send![notice_option_label, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_NOTICE_KEYCAP_OPTION_WIDTH, OVERLAY_NOTICE_KEYCAP_HEIGHT)
  )];
  let _: () = msg_send![notice_option_label, setStringValue: NSString::alloc(nil).init_str("⌥")];
  let _: () = msg_send![notice_option_label, setBezeled: NO];
  let _: () = msg_send![notice_option_label, setDrawsBackground: NO];
  let _: () = msg_send![notice_option_label, setEditable: NO];
  let _: () = msg_send![notice_option_label, setSelectable: NO];
  let _: () = msg_send![notice_option_label, setAlignment: 1isize];
  let notice_key_font: id =
    msg_send![class!(NSFont), systemFontOfSize: OVERLAY_NOTICE_KEYCAP_FONT_SIZE];
  let _: () = msg_send![notice_option_label, setFont: notice_key_font];
  let _: () = msg_send![notice_option_label, setHidden: YES];
  let _: () = msg_send![notice_option_key, addSubview: notice_option_label];

  let notice_plus_label: id = msg_send![class!(NSTextField), alloc];
  let notice_plus_label: id = msg_send![notice_plus_label, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_NOTICE_KEYCAP_PLUS_WIDTH, OVERLAY_NOTICE_KEYCAP_HEIGHT)
  )];
  let _: () = msg_send![notice_plus_label, setStringValue: NSString::alloc(nil).init_str("+")];
  let _: () = msg_send![notice_plus_label, setBezeled: NO];
  let _: () = msg_send![notice_plus_label, setDrawsBackground: NO];
  let _: () = msg_send![notice_plus_label, setEditable: NO];
  let _: () = msg_send![notice_plus_label, setSelectable: NO];
  let _: () = msg_send![notice_plus_label, setAlignment: 1isize];
  let notice_plus_font: id =
    msg_send![class!(NSFont), systemFontOfSize: OVERLAY_NOTICE_KEYCAP_FONT_SIZE];
  let _: () = msg_send![notice_plus_label, setFont: notice_plus_font];
  let _: () = msg_send![notice_plus_label, setHidden: YES];
  let _: () = msg_send![notice_accessory_row, addSubview: notice_plus_label];

  let notice_space_key: id = msg_send![class!(NSView), alloc];
  let notice_space_key: id = msg_send![notice_space_key, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_NOTICE_KEYCAP_SPACE_WIDTH, OVERLAY_NOTICE_KEYCAP_HEIGHT)
  )];
  let _: () = msg_send![notice_space_key, setWantsLayer: YES];
  let _: () = msg_send![notice_space_key, setHidden: YES];
  let _: () = msg_send![notice_accessory_row, addSubview: notice_space_key];

  let notice_space_label: id = msg_send![class!(NSTextField), alloc];
  let notice_space_label: id = msg_send![notice_space_label, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_NOTICE_KEYCAP_SPACE_WIDTH, OVERLAY_NOTICE_KEYCAP_HEIGHT)
  )];
  let _: () = msg_send![notice_space_label, setStringValue: NSString::alloc(nil).init_str("Space")];
  let _: () = msg_send![notice_space_label, setBezeled: NO];
  let _: () = msg_send![notice_space_label, setDrawsBackground: NO];
  let _: () = msg_send![notice_space_label, setEditable: NO];
  let _: () = msg_send![notice_space_label, setSelectable: NO];
  let _: () = msg_send![notice_space_label, setAlignment: 1isize];
  let notice_space_font: id =
    msg_send![class!(NSFont), systemFontOfSize: OVERLAY_NOTICE_KEYCAP_FONT_SIZE];
  let _: () = msg_send![notice_space_label, setFont: notice_space_font];
  let _: () = msg_send![notice_space_label, setHidden: YES];
  let _: () = msg_send![notice_space_key, addSubview: notice_space_label];

  let notice_auto_on_chip: id = msg_send![class!(NSView), alloc];
  let notice_auto_on_chip: id = msg_send![notice_auto_on_chip, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_NOTICE_AUTO_ON_CHIP_WIDTH, OVERLAY_NOTICE_AUTO_ON_CHIP_HEIGHT)
  )];
  let _: () = msg_send![notice_auto_on_chip, setWantsLayer: YES];
  let _: () = msg_send![notice_auto_on_chip, setHidden: YES];
  let _: () = msg_send![notice_accessory_row, addSubview: notice_auto_on_chip];

  let notice_auto_on_label: id = msg_send![class!(NSTextField), alloc];
  let notice_auto_on_label: id = msg_send![notice_auto_on_label, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(OVERLAY_NOTICE_AUTO_ON_CHIP_WIDTH, OVERLAY_NOTICE_AUTO_ON_CHIP_HEIGHT)
  )];
  let _: () =
    msg_send![notice_auto_on_label, setStringValue: NSString::alloc(nil).init_str("AUTO ON")];
  let _: () = msg_send![notice_auto_on_label, setBezeled: NO];
  let _: () = msg_send![notice_auto_on_label, setDrawsBackground: NO];
  let _: () = msg_send![notice_auto_on_label, setEditable: NO];
  let _: () = msg_send![notice_auto_on_label, setSelectable: NO];
  let _: () = msg_send![notice_auto_on_label, setAlignment: 1isize];
  let _: () = msg_send![notice_auto_on_label, setUsesSingleLineMode: YES];
  let notice_on_font: id =
    msg_send![class!(NSFont), boldSystemFontOfSize: OVERLAY_NOTICE_AUTO_ON_FONT_SIZE];
  let _: () = msg_send![notice_auto_on_label, setFont: notice_on_font];
  let _: () = msg_send![notice_auto_on_label, setHidden: YES];
  let _: () = msg_send![notice_auto_on_chip, addSubview: notice_auto_on_label];

  let busy_gradient_layer: id = msg_send![class!(CAGradientLayer), layer];
  let busy_mask_layer: id = msg_send![class!(CALayer), layer];
  if busy_gradient_layer != nil && busy_mask_layer != nil {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(overlay_width, overlay_height));
    let _: () = msg_send![busy_gradient_layer, setFrame: frame];
    let _: () = msg_send![busy_gradient_layer, setHidden: YES];
    let _: () = msg_send![busy_gradient_layer, setOpacity: 1.0f32];
    let _: () = msg_send![busy_gradient_layer, setCornerRadius: OVERLAY_CARD_RADIUS];
    let _: () = msg_send![busy_gradient_layer, setMasksToBounds: YES];
    let _: () = msg_send![busy_gradient_layer, setNeedsDisplayOnBoundsChange: YES];
    let _: () = msg_send![busy_gradient_layer, setType: NSString::alloc(nil).init_str("conic")];
    let _: () = msg_send![busy_gradient_layer, setStartPoint: NSPoint::new(0.5, 0.5)];
    let _: () = msg_send![busy_gradient_layer, setEndPoint: NSPoint::new(1.0, 0.5)];
    let _: () = msg_send![busy_gradient_layer, setZPosition: 32.0f64];

    let locations: id = msg_send![class!(NSMutableArray), arrayWithCapacity: 5usize];
    for point in [0.0f64, 0.04, 0.10, 0.18, 1.0] {
      let number: id = msg_send![class!(NSNumber), numberWithDouble: point];
      let _: () = msg_send![locations, addObject: number];
    }
    let _: () = msg_send![busy_gradient_layer, setLocations: locations];

    let colors: id = msg_send![class!(NSMutableArray), arrayWithCapacity: 5usize];
    for (r, g, b, a) in [
      (0.88, 0.97, 1.0, 1.0),
      (0.62, 0.88, 1.0, 0.98),
      (0.34, 0.74, 1.0, 0.62),
      (0.14, 0.52, 0.98, 0.16),
      (0.88, 0.97, 1.0, 1.0),
    ] {
      let color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, r, g, b, a);
      let cg_color: id = msg_send![color, CGColor];
      let _: () = msg_send![colors, addObject: cg_color];
    }
    let _: () = msg_send![busy_gradient_layer, setColors: colors];

    let _: () = msg_send![busy_mask_layer, setFrame: frame];
    let _: () = msg_send![busy_mask_layer, setCornerRadius: OVERLAY_CARD_RADIUS];
    let _: () = msg_send![busy_mask_layer, setBorderWidth: OVERLAY_BUSY_RING_THICKNESS];
    let white = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 1.0);
    let white_cg: id = msg_send![white, CGColor];
    let _: () = msg_send![busy_mask_layer, setBorderColor: white_cg];
    let _: () = msg_send![busy_mask_layer, setNeedsDisplayOnBoundsChange: YES];

    let _: () = msg_send![busy_gradient_layer, setMask: busy_mask_layer];
    let _: () = msg_send![card_layer, addSublayer: busy_gradient_layer];
  }

  // History-mode search bar widgets: editable text field + magnifier icon.
  // Hidden in speech mode; positioned and shown by render_overlay_history_list.
  let search_field: id = msg_send![class!(NSTextField), alloc];
  let search_field: id = msg_send![search_field, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(1.0, SEARCH_BAR_HEIGHT)
  )];
  let _: () = msg_send![search_field, setBezeled: NO];
  let _: () = msg_send![search_field, setBordered: NO];
  let _: () = msg_send![search_field, setDrawsBackground: NO];
  let _: () = msg_send![search_field, setEditable: YES];
  let _: () = msg_send![search_field, setSelectable: YES];
  let _: () = msg_send![search_field, setUsesSingleLineMode: YES];
  let _: () = msg_send![search_field, setLineBreakMode: 4isize]; // truncating tail
  let _: () = msg_send![search_field, setFocusRingType: 1isize]; // None
  let _: () = msg_send![search_field, setAlignment: 0isize]; // left
  let search_font: id = msg_send![class!(NSFont), systemFontOfSize: HISTORY_BODY_FONT_SIZE];
  if search_font != nil {
    let _: () = msg_send![search_field, setFont: search_font];
  }
  let search_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.9);
  let _: () = msg_send![search_field, setTextColor: search_color];
  let _: () = msg_send![search_field, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: search_field];

  let search_delegate_class = register_search_field_delegate_class();
  let search_delegate: id = msg_send![search_delegate_class, new];
  let _: () = msg_send![search_field, setDelegate: search_delegate];
  SEARCH_FIELD_DELEGATE_REF.with(|r| {
    r.borrow_mut().replace(search_delegate);
  });

  // No magnifier icon. Field stays in `OverlayRefs` for binary
  // compatibility but is left as nil; layout helpers skip when nil.
  let search_icon: id = nil;

  // Blinking caret. CALayer-backed NSView with a white fill, hidden by
  // default. The blink is a CABasicAnimation on `opacity` attached once at
  // construction so it runs continuously while the view is visible.
  let search_caret: id = msg_send![class!(NSView), alloc];
  let search_caret: id = msg_send![search_caret, initWithFrame: NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(SEARCH_CARET_WIDTH, SEARCH_CARET_HEIGHT)
  )];
  let _: () = msg_send![search_caret, setWantsLayer: YES];
  let caret_layer: id = msg_send![search_caret, layer];
  if caret_layer != nil {
    let white_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
    let cg: id = msg_send![white_color, CGColor];
    let _: () = msg_send![caret_layer, setBackgroundColor: cg];
    let anim: id = msg_send![class!(CABasicAnimation),
                             animationWithKeyPath: NSString::alloc(nil).init_str("opacity")];
    if anim != nil {
      let from_v: id = msg_send![class!(NSNumber), numberWithDouble: 1.0f64];
      let to_v: id = msg_send![class!(NSNumber), numberWithDouble: 0.0f64];
      let _: () = msg_send![anim, setFromValue: from_v];
      let _: () = msg_send![anim, setToValue: to_v];
      let _: () = msg_send![anim, setDuration: SEARCH_CARET_BLINK_HALF_PERIOD];
      let _: () = msg_send![anim, setAutoreverses: YES];
      let _: () = msg_send![anim, setRepeatCount: f32::INFINITY];
      let _: () = msg_send![caret_layer,
                            addAnimation: anim
                            forKey: NSString::alloc(nil).init_str("blink")];
    }
  }
  let _: () = msg_send![search_caret, setHidden: YES];
  let _: () = msg_send![card_view, addSubview: search_caret];

  let refs = OverlayRefs {
    window,
    card_view,
    label,
    hold_badge,
    raw_badge,
    connector_chip,
    connector_chip_label,
    connector_chip_icon,
    conv_query_label,
    conv_divider,
    conv_reply_scroll,
    conv_reply_text,
    conv_status_label,
    meter_view,
    wave_bars,
    busy_gradient_layer,
    busy_mask_layer,
    notice_accessory_row,
    notice_option_key,
    notice_option_label,
    notice_plus_label,
    notice_space_key,
    notice_space_label,
    notice_auto_on_chip,
    notice_auto_on_label,
    autocomplete_separator,
    autocomplete_labels,
    autocomplete_bgs,
    autocomplete_expand_markers,
    autocomplete_ts_labels,
    autocomplete_char_count_labels,
    search_field,
    search_icon,
    search_caret,
  };
  render_overlay_text(refs, "", &[], None, false, false, "", "");
  refs
}

/// Build our MOD_* mask from the live CGEventFlags booleans (tap thread).
fn current_mod_mask(is_option: bool, is_shift: bool, is_command: bool, is_control: bool) -> u8 {
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

/// Translate our MOD_* mask to global_hotkey Modifiers for the Carbon fallback.
fn modifiers_for_mask(mask: u8) -> Modifiers {
  let mut mods = Modifiers::empty();
  if mask & MOD_SHIFT != 0 {
    mods |= Modifiers::SHIFT;
  }
  if mask & MOD_CONTROL != 0 {
    mods |= Modifiers::CONTROL;
  }
  if mask & MOD_OPTION != 0 {
    mods |= Modifiers::ALT;
  }
  if mask & MOD_COMMAND != 0 {
    mods |= Modifiers::META;
  }
  mods
}

pub fn listen_modifiers() -> u8 {
  LISTEN_MODIFIERS.load(Ordering::Relaxed)
}

pub fn overlay_position() -> OverlayPosition {
  OverlayPosition::from_ui_index(OVERLAY_POSITION.load(Ordering::Relaxed) as i64)
}

/// Set the overlay target-display mode. The positioner reads the atomic on the
/// next show / content update, so the change applies live. Caller persists.
pub fn set_overlay_position(pos: OverlayPosition) {
  OVERLAY_POSITION.store(pos.ui_index() as u8, Ordering::Release);
}

/// Apply a new listen-modifier mask live. The HID tap reads the atomic on the
/// next keystroke (no re-arm); the Carbon fallback is re-registered so the OLD
/// chord stops triggering too (only fires when Accessibility is denied, but
/// must still be correct). No-op for an empty mask. Caller persists.
pub fn set_listen_modifiers(mask: u8) {
  if mask == 0 {
    return;
  }
  let old = LISTEN_MODIFIERS.swap(mask, Ordering::Release);
  if old != mask {
    relink_listen_hotkey_fallback(old, mask);
  }
}

fn relink_listen_hotkey_fallback(old_mask: u8, new_mask: u8) {
  HOTKEY_MANAGER_REF.with(|slot| {
    let borrow = slot.borrow();
    let Some(manager) = borrow.as_ref() else {
      return;
    };
    let old = HotKey::new(Some(modifiers_for_mask(old_mask)), HOLD_HOTKEY_KEY);
    let old_id = old.id();
    let _ = manager.unregister(old);
    let new = HotKey::new(Some(modifiers_for_mask(new_mask)), HOLD_HOTKEY_KEY);
    let new_id = new.id();
    match manager.register(new) {
      Ok(()) => HOTKEY_LISTEN_ID.store(new_id, Ordering::Relaxed),
      Err(err) => {
        eprintln!("Azad: failed to re-register listen hotkey fallback: {err}");
        // Roll back so a working chord remains registered.
        let rollback = HotKey::new(Some(modifiers_for_mask(old_mask)), HOLD_HOTKEY_KEY);
        let _ = manager.register(rollback);
        HOTKEY_LISTEN_ID.store(old_id, Ordering::Relaxed);
      }
    }
  });
}

fn install_global_hotkeys() {
  let manager = match GlobalHotKeyManager::new() {
    Ok(manager) => manager,
    Err(err) => {
      eprintln!("Azad: failed to initialize global hotkey manager: {}", err);
      return;
    }
  };

  let listen_mods = modifiers_for_mask(LISTEN_MODIFIERS.load(Ordering::Relaxed));
  let hotkey = HotKey::new(Some(listen_mods), HOLD_HOTKEY_KEY);
  let hotkey_id = hotkey.id();

  if let Err(err) = manager.register(hotkey) {
    eprintln!("Azad: failed to register listen hotkey (might be in use): {}", err);
    return;
  }

  HOTKEY_LISTEN_ID.store(hotkey_id, Ordering::Relaxed);

  let escape_hotkey = HotKey::new(None, Code::Escape);
  let _ = HOTKEY_ESCAPE_ID.set(escape_hotkey.id());
  let enter_hotkey = HotKey::new(None, Code::Enter);
  let _ = HOTKEY_ENTER_ID.set(enter_hotkey.id());
  let enter_option_hotkey = HotKey::new(Some(Modifiers::ALT), Code::Enter);
  let _ = HOTKEY_ENTER_OPTION_ID.set(enter_option_hotkey.id());
  let numpad_enter_hotkey = HotKey::new(None, Code::NumpadEnter);
  let _ = HOTKEY_NUMPAD_ENTER_ID.set(numpad_enter_hotkey.id());
  let numpad_enter_option_hotkey = HotKey::new(Some(Modifiers::ALT), Code::NumpadEnter);
  let _ = HOTKEY_NUMPAD_ENTER_OPTION_ID.set(numpad_enter_option_hotkey.id());
  let arrow_up_hotkey = HotKey::new(None, Code::ArrowUp);
  let _ = HOTKEY_ARROW_UP_ID.set(arrow_up_hotkey.id());
  let arrow_down_hotkey = HotKey::new(None, Code::ArrowDown);
  let _ = HOTKEY_ARROW_DOWN_ID.set(arrow_down_hotkey.id());
  let arrow_left_hotkey = HotKey::new(None, Code::ArrowLeft);
  let _ = HOTKEY_ARROW_LEFT_ID.set(arrow_left_hotkey.id());
  let arrow_right_hotkey = HotKey::new(None, Code::ArrowRight);
  let _ = HOTKEY_ARROW_RIGHT_ID.set(arrow_right_hotkey.id());

  GlobalHotKeyEvent::set_event_handler(Some(|event| {
    handle_global_hotkey_event(event);
  }));

  HOTKEY_MANAGER_REF.with(|slot| {
    slot.borrow_mut().replace(manager);
  });
}

/// Install a low-level HID-tap that claims the hotkeys Azad cares about before any foreground
/// app (VNC viewers, remote-desktop clients, etc.) can intercept them. Runs on a dedicated
/// thread so slow work on the main thread never causes macOS to disable the tap for timeout.
///
/// Authorized by **Accessibility**, which Azad already requires: this is an *active*
/// tap (`kCGEventTapOptionDefault`) that can consume the hotkey, and active taps are
/// gated on Accessibility — not Input Monitoring (that gates listen-only taps). So once
/// Accessibility is granted the tap succeeds, including over screen-sharing. If it still
/// fails to create we fall back silently to the Carbon `RegisterEventHotKey` path — which
/// works in most apps but gets swallowed by VNC clients that install their own HID tap.
fn install_hotkey_event_tap() {
  std::thread::Builder::new()
    .name("azad-hotkey-tap".to_string())
    .spawn(|| unsafe {
      let events_of_interest = (1u64 << KCG_EVENT_KEY_DOWN) | (1u64 << KCG_EVENT_KEY_UP);

      let tap = CGEventTapCreate(
        KCG_HID_EVENT_TAP,
        KCG_HEAD_INSERT_EVENT_TAP,
        KCG_EVENT_TAP_OPTION_DEFAULT,
        events_of_interest,
        event_tap_callback,
        std::ptr::null_mut(),
      );
      if tap.is_null() {
        eprintln!(
          "Azad: couldn't install HID event tap — grant Accessibility in System Settings so the \
           active tap can claim hotkeys over VNC / screen-sharing clients. Falling back to Carbon."
        );
        return;
      }
      EVENT_TAP_PORT.store(tap, Ordering::Release);

      let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
      if source.is_null() {
        eprintln!("Azad: failed to create run-loop source for HID event tap");
        CFRelease(tap.cast());
        EVENT_TAP_PORT.store(std::ptr::null_mut(), Ordering::Release);
        return;
      }

      CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
      CGEventTapEnable(tap, true);
      CFRelease(source.cast());

      // CFRunLoopRun blocks forever, driving the tap callback on this thread.
      CFRunLoopRun();
    })
    .expect("spawn hotkey-tap thread");
}

extern "C" fn event_tap_callback(
  _proxy: *mut c_void,
  event_type: u32,
  event: *mut c_void,
  _user_info: *mut c_void,
) -> *mut c_void {
  // macOS disables the tap if the callback is too slow or if the user triggers certain input
  // sequences. Re-enable and pass the event through.
  if event_type == KCG_EVENT_TAP_DISABLED_BY_TIMEOUT
    || event_type == KCG_EVENT_TAP_DISABLED_BY_USER_INPUT
  {
    let tap = EVENT_TAP_PORT.load(Ordering::Acquire);
    if !tap.is_null() {
      unsafe { CGEventTapEnable(tap, true) };
    }
    return event;
  }

  if event_type != KCG_EVENT_KEY_DOWN && event_type != KCG_EVENT_KEY_UP {
    return event;
  }

  // Skip events Azad itself synthesized (Cmd+V, auto-submit Enter, etc.). Without this check
  // the tap would swallow our own Enter and auto-submit would never reach the focused app.
  let user_data = unsafe { CGEventGetIntegerValueField(event, KCG_EVENT_SOURCE_USER_DATA_FIELD) };
  if user_data == AZAD_SYNTHETIC_MARKER {
    return event;
  }

  let keycode = unsafe { CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_KEYCODE_FIELD) };
  let autorepeat =
    unsafe { CGEventGetIntegerValueField(event, KCG_KEYBOARD_EVENT_AUTOREPEAT_FIELD) };
  let flags = unsafe { CGEventGetFlags(event) };
  let is_option = (flags & CGEventFlags::CGEventFlagAlternate.bits()) != 0;
  let is_shift = (flags & CGEventFlags::CGEventFlagShift.bits()) != 0;
  let is_command = (flags & CGEventFlags::CGEventFlagCommand.bits()) != 0;
  let is_control = (flags & CGEventFlags::CGEventFlagControl.bits()) != 0;
  let is_keydown = event_type == KCG_EVENT_KEY_DOWN;
  let is_autorepeat = autorepeat != 0;

  if claim_tap_hotkey(
    keycode as u16,
    is_option,
    is_shift,
    is_command,
    is_control,
    is_keydown,
    is_autorepeat,
  ) {
    return std::ptr::null_mut();
  }
  // History search bar: when active, intercept printable characters and
  // backspace so they fill the search field instead of leaking through to
  // the focused app. AppKit's responder chain doesn't help here — the
  // overlay is a borderless NSWindow that won't take key without
  // activating Azad — so we capture at the HID layer like every other
  // overlay-mode hotkey.
  if claim_tap_search_input(event, keycode as u16, is_keydown, is_autorepeat, flags) {
    return std::ptr::null_mut();
  }
  event
}

fn claim_tap_hotkey(
  keycode: u16,
  is_option: bool,
  is_shift: bool,
  is_command: bool,
  is_control: bool,
  is_keydown: bool,
  is_autorepeat: bool,
) -> bool {
  // Option+Space: hold-to-talk.
  //
  // Dispatch `HotkeyPressed` on **every** non-autorepeat Opt+Space keydown (no edge-debounce),
  // because if the prior keyup was ever dropped (tap disabled mid-hold, screen lock, another
  // app briefly taking exclusive keyboard focus, etc.), the previous approach of gating on
  // `SPACE_HOLD_CLAIMED.swap(true)` would return `was_held=true` forever and silently swallow
  // every subsequent press — appearing to the user as "Azad is stuck; restart fixes it". OS
  // auto-repeat keydowns are filtered via `kCGKeyboardEventAutorepeat` so this doesn't flood
  // the state machine with repeated presses while you're just holding the key.
  //
  // `SPACE_HOLD_CLAIMED` is still used to decide whether to *claim* (swallow) the subsequent
  // keyup — a bare Space keyup not preceded by Opt+Space should pass through normally — but it
  // no longer gates dispatch.
  if keycode == KEYCODE_SPACE {
    // Listen hotkey: Space plus the user-configured modifier set. Superset-match
    // (all wanted modifiers held; extras OK) so the default (Option) is identical
    // to the old `is_option` check. `wanted != 0` guards against a corrupt empty
    // mask turning bare Space into a global trigger.
    let wanted = LISTEN_MODIFIERS.load(Ordering::Acquire);
    let live = current_mod_mask(is_option, is_shift, is_command, is_control);
    let mods_match = wanted != 0 && (live & wanted) == wanted;
    if is_keydown && mods_match {
      if is_autorepeat {
        // Claim the event so the remote VNC never gets a flood of spaces, but don't dispatch
        // — the state machine already knows we're holding.
        return true;
      }
      SPACE_HOLD_CLAIMED.store(true, Ordering::Release);
      crate::app::send_event(AppEvent::HotkeyPressed);
      return true;
    }
    if !is_keydown && SPACE_HOLD_CLAIMED.swap(false, Ordering::AcqRel) {
      crate::app::send_event(AppEvent::HotkeyReleased);
      return true;
    }
    return false;
  }

  // Overlay-only hotkeys. Claim both keydown and keyup so the underlying app never sees either
  // half of the chord. Event dispatch only fires on keydown.
  if HOTKEY_ESCAPE_REGISTERED.load(Ordering::Relaxed) && keycode == KEYCODE_ESCAPE {
    if is_keydown {
      crate::app::send_event(AppEvent::OverlayCancel);
    }
    return true;
  }

  if HOTKEY_ENTER_REGISTERED.load(Ordering::Relaxed)
    && (keycode == KEYCODE_RETURN || keycode == KEYCODE_NUMPAD_ENTER)
  {
    if is_shift {
      // Shift+Enter is the user's "soft return / newline in the app underneath"
      // escape hatch. Don't claim, don't dispatch — let the OS deliver the chord
      // to whichever app has keyboard focus.
      return false;
    }
    if is_keydown {
      crate::app::send_event(AppEvent::FinalizeHotkeyPressed { raw_requested: is_option });
    }
    return true;
  }

  if HOTKEY_ARROWS_REGISTERED.load(Ordering::Relaxed) {
    if keycode == KEYCODE_ARROW_UP {
      if is_keydown {
        crate::app::send_event(AppEvent::ArrowNavigate(-1));
      }
      return true;
    }
    if keycode == KEYCODE_ARROW_DOWN {
      if is_keydown {
        crate::app::send_event(AppEvent::ArrowNavigate(1));
      }
      return true;
    }
  }

  if HOTKEY_ARROW_LEFT_REGISTERED.load(Ordering::Relaxed) && keycode == KEYCODE_ARROW_LEFT {
    if is_keydown {
      crate::app::send_event(AppEvent::HistoryCollapse);
    }
    return true;
  }

  if HOTKEY_ARROW_RIGHT_REGISTERED.load(Ordering::Relaxed) && keycode == KEYCODE_ARROW_RIGHT {
    if is_keydown {
      crate::app::send_event(AppEvent::HistoryExpand);
    }
    return true;
  }

  false
}

const KEYCODE_DELETE: u16 = 51; // backspace

/// When history-mode key capture is active, claim printable-character
/// keydowns, backspace (with Cmd/Option modifiers), and Enter — feeding
/// them into the search-field flow or directly into the paste-from-history
/// flow via app events. Returns true when the event was consumed (so the
/// focused app never sees the keydown). Keyups are allowed to pass
/// through — focused apps tolerate orphan keyups for keys whose keydowns
/// we consumed.
fn claim_tap_search_input(
  event: *mut c_void,
  keycode: u16,
  is_keydown: bool,
  _is_autorepeat: bool,
  flags: u64,
) -> bool {
  if !OVERLAY_ACCEPTS_KEY_INPUT.load(Ordering::Relaxed) {
    return false;
  }
  if !is_keydown {
    return false;
  }
  let is_option = (flags & CGEventFlags::CGEventFlagAlternate.bits()) != 0;
  let is_command = (flags & CGEventFlags::CGEventFlagCommand.bits()) != 0;
  let is_shift = (flags & CGEventFlags::CGEventFlagShift.bits()) != 0;

  // Enter short-circuit: paste the selected history entry. Done here as a
  // belt-and-suspenders fix — `claim_tap_hotkey` should already handle
  // Enter, but its dispatch is gated on `HOTKEY_ENTER_REGISTERED`, and
  // when the panel is key + the search field is first responder there
  // were occurrences of Enter not pasting. This direct claim is
  // independent of that gate.
  //
  // Shift+Enter still passes through here too, mirroring the bypass in
  // `claim_tap_hotkey` so a soft-return chord lands in the focused app even
  // when the search field is first responder.
  if keycode == KEYCODE_RETURN || keycode == KEYCODE_NUMPAD_ENTER {
    if is_shift {
      return false;
    }
    crate::app::send_event(AppEvent::FinalizeHotkeyPressed { raw_requested: is_option });
    return true;
  }

  if keycode == KEYCODE_DELETE {
    let event = if is_command {
      AppEvent::HistorySearchClear
    } else if is_option {
      AppEvent::HistorySearchDeleteWord
    } else {
      AppEvent::HistorySearchBackspace
    };
    crate::app::send_event(event);
    return true;
  }
  // Read the actual unicode the keystroke produces (respects layout, shift/
  // option, dead keys, etc.).
  let mut buf = [0u16; 8];
  let mut actual_len: u64 = 0;
  unsafe {
    CGEventKeyboardGetUnicodeString(event, buf.len() as u64, &mut actual_len, buf.as_mut_ptr());
  }
  if actual_len == 0 {
    return false;
  }
  let chars: String = match String::from_utf16(&buf[..actual_len as usize]) {
    Ok(s) => s,
    Err(_) => return false,
  };
  if chars.is_empty() || chars.chars().any(|c| c.is_control()) {
    return false;
  }
  crate::app::send_event(AppEvent::HistorySearchAppend(chars));
  true
}

fn handle_global_hotkey_event(event: GlobalHotKeyEvent) {
  let listen_id = HOTKEY_LISTEN_ID.load(Ordering::Relaxed);
  if listen_id != 0 && event.id == listen_id {
    match event.state {
      HotKeyState::Pressed => crate::app::send_event(AppEvent::HotkeyPressed),
      HotKeyState::Released => crate::app::send_event(AppEvent::HotkeyReleased),
    }
    return;
  }

  if let Some(escape_id) = HOTKEY_ESCAPE_ID.get().copied() {
    if event.id == escape_id
      && HOTKEY_ESCAPE_REGISTERED.load(Ordering::Relaxed)
      && matches!(event.state, HotKeyState::Pressed)
    {
      crate::app::send_event(AppEvent::OverlayCancel);
      return;
    }
  }

  let is_enter_hotkey = HOTKEY_ENTER_ID.get().is_some_and(|id| event.id == *id)
    || HOTKEY_ENTER_OPTION_ID.get().is_some_and(|id| event.id == *id)
    || HOTKEY_NUMPAD_ENTER_ID.get().is_some_and(|id| event.id == *id)
    || HOTKEY_NUMPAD_ENTER_OPTION_ID.get().is_some_and(|id| event.id == *id);
  let is_option_enter_hotkey = HOTKEY_ENTER_OPTION_ID.get().is_some_and(|id| event.id == *id)
    || HOTKEY_NUMPAD_ENTER_OPTION_ID.get().is_some_and(|id| event.id == *id);
  if is_enter_hotkey
    && HOTKEY_ENTER_REGISTERED.load(Ordering::Relaxed)
    && matches!(event.state, HotKeyState::Pressed)
  {
    crate::app::send_event(AppEvent::FinalizeHotkeyPressed {
      raw_requested: is_option_enter_hotkey,
    });
    return;
  }

  if HOTKEY_ARROWS_REGISTERED.load(Ordering::Relaxed) && matches!(event.state, HotKeyState::Pressed)
  {
    if HOTKEY_ARROW_UP_ID.get().is_some_and(|id| event.id == *id) {
      crate::app::send_event(AppEvent::ArrowNavigate(-1));
      return;
    }
    if HOTKEY_ARROW_DOWN_ID.get().is_some_and(|id| event.id == *id) {
      crate::app::send_event(AppEvent::ArrowNavigate(1));
      return;
    }
  }

  if HOTKEY_ARROW_LEFT_REGISTERED.load(Ordering::Relaxed)
    && matches!(event.state, HotKeyState::Pressed)
    && HOTKEY_ARROW_LEFT_ID.get().is_some_and(|id| event.id == *id)
  {
    crate::app::send_event(AppEvent::HistoryCollapse);
    return;
  }

  if HOTKEY_ARROW_RIGHT_REGISTERED.load(Ordering::Relaxed)
    && matches!(event.state, HotKeyState::Pressed)
    && HOTKEY_ARROW_RIGHT_ID.get().is_some_and(|id| event.id == *id)
  {
    crate::app::send_event(AppEvent::HistoryExpand);
  }
}

fn set_escape_hotkey_enabled(enabled: bool) {
  let currently_enabled = HOTKEY_ESCAPE_REGISTERED.load(Ordering::Relaxed);
  if currently_enabled == enabled {
    return;
  }

  HOTKEY_MANAGER_REF.with(|slot| {
    let mut manager_slot = slot.borrow_mut();
    let Some(manager) = manager_slot.as_mut() else {
      return;
    };

    let escape_hotkey = HotKey::new(None, Code::Escape);
    let result =
      if enabled { manager.register(escape_hotkey) } else { manager.unregister(escape_hotkey) };

    match result {
      Ok(()) => {
        HOTKEY_ESCAPE_REGISTERED.store(enabled, Ordering::Relaxed);
      }
      Err(err) => {
        eprintln!(
          "Azad: failed to {} Escape hotkey: {}",
          if enabled { "register" } else { "unregister" },
          err
        );
      }
    }
  });
}

fn set_enter_hotkey_enabled(enabled: bool) {
  let currently_enabled = HOTKEY_ENTER_REGISTERED.load(Ordering::Relaxed);
  if currently_enabled == enabled {
    return;
  }

  HOTKEY_MANAGER_REF.with(|slot| {
    let mut manager_slot = slot.borrow_mut();
    let Some(manager) = manager_slot.as_mut() else {
      return;
    };

    let enter_hotkey = HotKey::new(None, Code::Enter);
    let enter_option_hotkey = HotKey::new(Some(Modifiers::ALT), Code::Enter);
    let numpad_enter_hotkey = HotKey::new(None, Code::NumpadEnter);
    let numpad_enter_option_hotkey = HotKey::new(Some(Modifiers::ALT), Code::NumpadEnter);

    if enabled {
      match manager.register(enter_hotkey) {
        Ok(()) => {
          HOTKEY_ENTER_REGISTERED.store(true, Ordering::Relaxed);
        }
        Err(err) => {
          eprintln!("Azad: failed to register Enter hotkey: {}", err);
          return;
        }
      }

      if let Err(err) = manager.register(enter_option_hotkey) {
        eprintln!("Azad: failed to register Option+Enter hotkey: {}", err);
      }
      if let Err(err) = manager.register(numpad_enter_hotkey) {
        eprintln!("Azad: failed to register NumpadEnter hotkey: {}", err);
      }
      if let Err(err) = manager.register(numpad_enter_option_hotkey) {
        eprintln!("Azad: failed to register Option+NumpadEnter hotkey: {}", err);
      }
      return;
    }

    if let Err(err) = manager.unregister(enter_hotkey) {
      eprintln!("Azad: failed to unregister Enter hotkey: {}", err);
    }
    if let Err(err) = manager.unregister(enter_option_hotkey) {
      eprintln!("Azad: failed to unregister Option+Enter hotkey: {}", err);
    }
    if let Err(err) = manager.unregister(numpad_enter_hotkey) {
      eprintln!("Azad: failed to unregister NumpadEnter hotkey: {}", err);
    }
    if let Err(err) = manager.unregister(numpad_enter_option_hotkey) {
      eprintln!("Azad: failed to unregister Option+NumpadEnter hotkey: {}", err);
    }
    HOTKEY_ENTER_REGISTERED.store(false, Ordering::Relaxed);
  });
}

fn set_arrow_hotkeys_enabled(enabled: bool) {
  let currently_enabled = HOTKEY_ARROWS_REGISTERED.load(Ordering::Relaxed);
  if currently_enabled == enabled {
    return;
  }

  HOTKEY_MANAGER_REF.with(|slot| {
    let mut manager_slot = slot.borrow_mut();
    let Some(manager) = manager_slot.as_mut() else {
      return;
    };

    let arrow_up = HotKey::new(None, Code::ArrowUp);
    let arrow_down = HotKey::new(None, Code::ArrowDown);

    if enabled {
      if let Err(err) = manager.register(arrow_up) {
        eprintln!("Azad: failed to register ArrowUp hotkey: {}", err);
      }
      if let Err(err) = manager.register(arrow_down) {
        eprintln!("Azad: failed to register ArrowDown hotkey: {}", err);
      }
      HOTKEY_ARROWS_REGISTERED.store(true, Ordering::Relaxed);
    } else {
      if let Err(err) = manager.unregister(arrow_up) {
        eprintln!("Azad: failed to unregister ArrowUp hotkey: {}", err);
      }
      if let Err(err) = manager.unregister(arrow_down) {
        eprintln!("Azad: failed to unregister ArrowDown hotkey: {}", err);
      }
      HOTKEY_ARROWS_REGISTERED.store(false, Ordering::Relaxed);
    }
  });
}

pub fn set_overlay_debug_logs_enabled(enabled: bool) {
  OVERLAY_DEBUG_LOGS_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Snapshot the current mouse-button state so the next call to
/// `poll_click_outside_overlay` doesn't fire on a pre-existing held button.
/// Called from `enter_history_mode`.
pub fn reset_click_outside_tracker() {
  let buttons: u64 = unsafe { msg_send![class!(NSEvent), pressedMouseButtons] };
  MOUSE_BUTTON_PREV_STATE.store(buttons, std::sync::atomic::Ordering::Relaxed);
}

/// Returns true on the tick where a fresh mouse-button press lands OUTSIDE
/// the overlay's frame. Caller should dismiss whatever overlay-driven mode
/// is active. Designed to be called from the app's `on_tick` while history
/// mode is active.
pub fn poll_click_outside_overlay() -> bool {
  unsafe {
    let buttons: u64 = msg_send![class!(NSEvent), pressedMouseButtons];
    let prev = MOUSE_BUTTON_PREV_STATE.swap(buttons, std::sync::atomic::Ordering::Relaxed);
    let new_press = buttons & !prev;
    if new_press == 0 {
      return false;
    }
    let Some(refs) = current_overlay() else {
      return false;
    };
    let visible: bool = msg_send![refs.window, isVisible];
    if !visible {
      return false;
    }
    let frame: NSRect = msg_send![refs.window, frame];
    let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
    let inside = mouse.x >= frame.origin.x
      && mouse.x <= frame.origin.x + frame.size.width
      && mouse.y >= frame.origin.y
      && mouse.y <= frame.origin.y + frame.size.height;
    !inside
  }
}

fn overlay_debug_logs_enabled() -> bool {
  OVERLAY_DEBUG_LOGS_ENABLED.load(Ordering::Relaxed)
}

pub fn set_arrow_left_hotkey_enabled(enabled: bool) {
  let currently_enabled = HOTKEY_ARROW_LEFT_REGISTERED.load(Ordering::Relaxed);
  if currently_enabled == enabled {
    return;
  }

  HOTKEY_MANAGER_REF.with(|slot| {
    let mut manager_slot = slot.borrow_mut();
    let Some(manager) = manager_slot.as_mut() else {
      return;
    };

    let arrow_left = HotKey::new(None, Code::ArrowLeft);
    if enabled {
      if let Err(err) = manager.register(arrow_left) {
        eprintln!("Azad: failed to register ArrowLeft hotkey: {}", err);
      }
      HOTKEY_ARROW_LEFT_REGISTERED.store(true, Ordering::Relaxed);
    } else {
      if let Err(err) = manager.unregister(arrow_left) {
        eprintln!("Azad: failed to unregister ArrowLeft hotkey: {}", err);
      }
      HOTKEY_ARROW_LEFT_REGISTERED.store(false, Ordering::Relaxed);
    }
  });
}

pub fn set_arrow_right_hotkey_enabled(enabled: bool) {
  let currently_enabled = HOTKEY_ARROW_RIGHT_REGISTERED.load(Ordering::Relaxed);
  if currently_enabled == enabled {
    return;
  }

  HOTKEY_MANAGER_REF.with(|slot| {
    let mut manager_slot = slot.borrow_mut();
    let Some(manager) = manager_slot.as_mut() else {
      return;
    };

    let arrow_right = HotKey::new(None, Code::ArrowRight);
    if enabled {
      if let Err(err) = manager.register(arrow_right) {
        eprintln!("Azad: failed to register ArrowRight hotkey: {}", err);
      }
      HOTKEY_ARROW_RIGHT_REGISTERED.store(true, Ordering::Relaxed);
    } else {
      if let Err(err) = manager.unregister(arrow_right) {
        eprintln!("Azad: failed to unregister ArrowRight hotkey: {}", err);
      }
      HOTKEY_ARROW_RIGHT_REGISTERED.store(false, Ordering::Relaxed);
    }
  });
}

unsafe fn send_direct_text_input(text: &str) -> bool {
  let source = match CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
    Ok(source) => source,
    Err(_) => return false,
  };
  release_modifiers(&source);

  // Dispatch per-character Unicode key events. Some targets appear to ignore or
  // truncate multi-character Unicode payloads in a single CGEvent.
  // Use a neutral printable keycode while attaching Unicode payload so we avoid
  // posting modifier/function-key events into terminal protocols.
  for ch in text.chars() {
    let mut one = String::new();
    one.push(ch);

    let Ok(key_down) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_DIRECT_INPUT, true)
    else {
      return false;
    };
    key_down.set_string(&one);
    key_down.post(CGEventTapLocation::HID);

    let Ok(key_up) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_DIRECT_INPUT, false)
    else {
      return false;
    };
    key_up.post(CGEventTapLocation::HID);
  }
  true
}

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

  // Stamp every event with our synthetic marker so our HID tap (if installed) passes them
  // through instead of re-dispatching them as a user hotkey press.
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

// Synthesize Cmd+V via `enigo`. Hand-rolled CGEvent posting (even carefully matched to what
// `enigo` emits byte-for-byte) doesn't survive forwarding through macOS Screen Sharing — the
// Cmd modifier gets stripped and only a bare `V` arrives on the remote. `enigo` works, so
// we delegate this single chord to it and leave the rest of the input path on CGEvent.
unsafe fn send_command_v() {
  use enigo::{Direction, Enigo, Key, Keyboard, Settings};

  let mut enigo = match Enigo::new(&Settings::default()) {
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
  // `Key::Other(9)` is the physical `V` keycode on macOS. Using it instead of `Key::Unicode('v')`
  // keeps the paste working on non-US layouts (where `v` might be at a different position).
  if let Err(err) = enigo.key(Key::Other(9), Direction::Click) {
    eprintln!("Azad: enigo V click failed: {err}");
  }
  std::thread::sleep(Duration::from_millis(PASTE_CHORD_HOLD_MS));
  if let Err(err) = enigo.key(Key::Meta, Direction::Release) {
    eprintln!("Azad: enigo Cmd up failed: {err}");
  }
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

// Write through `writeObjects:` rather than `setString:forType:`. NSString conforms to
// NSPasteboardWriting, so this single call populates every compatible representation
// (UTF-8, UTF-16, plain-text, …) in one shot. Matches what arboard, Handy, and any
// first-party Cocoa app does; avoids leaving the pasteboard in a partially-populated
// state that some observers don't latch onto.
unsafe fn write_pasteboard_string(text: &str) -> bool {
  let pasteboard = NSPasteboard::generalPasteboard(nil);
  let _: usize = msg_send![pasteboard, clearContents];
  let ns_text = NSString::alloc(nil).init_str(text);
  let array: id = msg_send![class!(NSArray), arrayWithObject: ns_text];
  let ok: i8 = msg_send![pasteboard, writeObjects: array];
  ok != 0
}

fn is_accessibility_trusted() -> bool {
  unsafe { AXIsProcessTrusted() }
}

fn maybe_request_accessibility_permission_once() {
  if OPENED_ACCESSIBILITY_SETTINGS
    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
    .is_ok()
  {
    // Ask macOS to surface the Accessibility trust flow.
    // If that does not trigger UI, fall back to opening the settings pane directly.
    let prompted = unsafe { request_accessibility_prompt() };
    if !prompted {
      let _ = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
    }
  }
}

// AXValueType tags from <ApplicationServices/.../AXValue.h>.
const KAX_VALUE_CG_POINT_TYPE: u32 = 1;
const KAX_VALUE_CG_SIZE_TYPE: u32 = 2;

unsafe extern "C" {
  fn AXIsProcessTrusted() -> bool;
  fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
  fn AXUIElementCreateApplication(pid: i32) -> *const c_void;
  fn AXUIElementCopyAttributeValue(
    element: *const c_void,
    attribute: *const c_void,
    value: *mut *const c_void,
  ) -> i32;
  fn AXUIElementSetMessagingTimeout(element: *const c_void, timeout: f32) -> i32;
  fn AXValueGetValue(value: *const c_void, the_type: u32, value_ptr: *mut c_void) -> bool;
}

type CGEventTapCallBack = extern "C" fn(
  proxy: *mut c_void,
  event_type: u32,
  event: *mut c_void,
  user_info: *mut c_void,
) -> *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
  fn CGEventTapCreate(
    tap: u32,
    place: u32,
    options: u32,
    events_of_interest: u64,
    callback: CGEventTapCallBack,
    user_info: *mut c_void,
  ) -> *mut c_void;

  fn CGEventTapEnable(tap: *mut c_void, enable: bool);

  fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
  fn CGEventGetFlags(event: *mut c_void) -> u64;
  fn CGEventKeyboardGetUnicodeString(
    event: *mut c_void,
    max_string_length: u64,
    actual_string_length: *mut u64,
    unicode_string: *mut u16,
  );

  fn CFMachPortCreateRunLoopSource(
    allocator: *const c_void,
    port: *mut c_void,
    order: isize,
  ) -> *mut c_void;

  fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);

  fn CFRunLoopGetCurrent() -> *mut c_void;
  fn CFRunLoopRun();

  fn CFRelease(cf: *const c_void);

  static kCFRunLoopCommonModes: *const c_void;
}

unsafe fn request_accessibility_prompt() -> bool {
  let key = NSString::alloc(nil).init_str("AXTrustedCheckOptionPrompt");
  let value: id = msg_send![class!(NSNumber), numberWithBool: YES];
  let options: id = msg_send![class!(NSDictionary), dictionaryWithObject: value forKey: key];

  if options == nil {
    return false;
  }

  AXIsProcessTrustedWithOptions(options as *const c_void)
}

/// Live macOS privacy-permission state for a single permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionStatus {
  Granted,
  Denied,
  #[default]
  NotDetermined,
}

/// Accessibility is effectively binary (trusted or not); there is no
/// "not determined" state to report.
pub fn accessibility_authorization() -> PermissionStatus {
  if is_accessibility_trusted() { PermissionStatus::Granted } else { PermissionStatus::Denied }
}

pub fn microphone_authorization() -> PermissionStatus {
  // AVAuthorizationStatus: 0 NotDetermined, 1 Restricted, 2 Denied, 3 Authorized.
  let status: i64 = unsafe {
    msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio]
  };
  match status {
    3 => PermissionStatus::Granted,
    0 => PermissionStatus::NotDetermined,
    _ => PermissionStatus::Denied,
  }
}

pub fn input_monitoring_authorization() -> PermissionStatus {
  // IOHIDAccessType: 0 Granted, 1 Denied, 2 Unknown. The HID event tap that
  // claims hotkeys over screen-sharing needs this; without it we fall back to
  // Carbon hotkeys, so it is optional.
  let access = unsafe { IOHIDCheckAccess(KIOHID_REQUEST_TYPE_LISTEN_EVENT) };
  match access {
    0 => PermissionStatus::Granted,
    1 => PermissionStatus::Denied,
    _ => PermissionStatus::NotDetermined,
  }
}

// IOHIDRequestType (IOKit hidsystem/IOHIDLib.h) is a C enum with implicit values:
// kIOHIDRequestTypePostEvent is the FIRST member (0), kIOHIDRequestTypeListenEvent
// is the SECOND (1). Input Monitoring is the listen-event access, so query 1;
// querying 0 (post) returns Granted spuriously.
const KIOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
  static AVMediaTypeAudio: id;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
  fn IOHIDCheckAccess(request: u32) -> u32;
}

unsafe fn assign_status_icon(status_item: id) {
  let button: id = msg_send![status_item, button];
  if button == nil {
    return;
  }

  // Use a template status item icon so AppKit automatically handles
  // light/dark mode and highlighted menu bar states.
  let template_icon = load_icon("azad-black.png");
  let fallback_icon = load_icon("azad-white.png");
  let icon = if template_icon != nil { template_icon } else { fallback_icon };

  if icon != nil {
    let _: () = msg_send![icon, setTemplate: YES];
    let _: () = msg_send![button, setImage: icon];
  } else {
    let _: () = msg_send![button, setTitle: NSString::alloc(nil).init_str("Azad")];
  }
}

/// Loads the connector chip icon (an `assets/` file, e.g. an SVG) as a cached
/// template image so it tints to the chip text color. `nil` if the name is empty
/// or the file can't be loaded — callers fall back to a text-only chip.
unsafe fn connector_chip_icon_image(name: &str) -> id {
  if name.is_empty() {
    return nil;
  }
  let cached = CONNECTOR_ICON_CACHE
    .with(|c| c.borrow().as_ref().filter(|(n, _)| n == name).map(|(_, img)| *img));
  if let Some(img) = cached {
    return img;
  }
  let img = load_icon(name);
  if img != nil {
    let _: () = msg_send![img, setTemplate: YES];
    CONNECTOR_ICON_CACHE.with(|c| *c.borrow_mut() = Some((name.to_string(), img)));
  }
  img
}

unsafe fn load_icon(name: &str) -> id {
  for base_dir in icon_base_dirs() {
    let path = base_dir.join(name);
    if !path.exists() {
      continue;
    }

    let ns_path = NSString::alloc(nil).init_str(&path.to_string_lossy());
    let image = NSImage::alloc(nil).initByReferencingFile_(ns_path);
    if image == nil {
      continue;
    }

    let _: () = msg_send![image, setSize: NSSize::new(18.0, 18.0)];
    return image;
  }

  nil
}

fn icon_base_dirs() -> Vec<PathBuf> {
  let mut out = Vec::new();

  if let Some(dir) = std::env::var_os("AZAD_ASSETS_DIR") {
    out.push(PathBuf::from(dir));
  }

  if let Some(bundle_dir) = unsafe { bundle_resources_dir() } {
    out.push(bundle_dir);
  }

  out.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"));
  out
}

unsafe fn bundle_resources_dir() -> Option<PathBuf> {
  let bundle: id = msg_send![class!(NSBundle), mainBundle];
  if bundle == nil {
    return None;
  }

  let resource_path: id = msg_send![bundle, resourcePath];
  nsstring_to_path(resource_path)
}

// Screen Sharing only syncs the local pasteboard to the remote Mac when its window receives a
// fresh NSApplicationDidBecomeActive event (classic VNC ClientCutText-on-focus pattern). While
// Screen Sharing stays frontmost, subsequent local clipboard writes don't propagate — the
// remote keeps pasting whatever value last synced. This helper briefly activates the current
// app (us, a menu-bar accessory) and then reactivates Screen Sharing. That round-trip fires
// DidResignActive → DidBecomeActive on Screen Sharing, which re-reads the local pasteboard
// and pushes ClientCutText to the remote. Cost: ~160 ms of added paste latency and a brief
// menu-bar flicker. Only runs when Screen Sharing is actually frontmost.
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

  // NSApplicationActivateIgnoringOtherApps = 1 << 1. Using the raw value because the
  // objc2_app_kit enum isn't in scope in this file.
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

unsafe fn nsstring_to_path(value: id) -> Option<PathBuf> {
  if value == nil {
    return None;
  }

  let ptr: *const c_char = msg_send![value, UTF8String];
  if ptr.is_null() {
    return None;
  }

  Some(PathBuf::from(CStr::from_ptr(ptr).to_string_lossy().into_owned()))
}

unsafe fn nsstring_to_string(value: id) -> Option<String> {
  if value == nil {
    return None;
  }

  let ptr: *const c_char = msg_send![value, UTF8String];
  if ptr.is_null() {
    return None;
  }

  Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
}
