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
  NSMainMenuWindowLevel, NSMenu, NSMenuItem, NSScreen, NSStatusBar, NSStatusItem,
  NSVariableStatusItemLength, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use core_graphics::event::CGEventFlags;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc::Encode;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use crate::app::AppEvent;
use crate::gateway::ConvStatus;
use crate::settings::OverlayPosition;
pub use crate::ui_model::{ConnectorRowVM, OnboardingViewModel, SettingsTab, SettingsViewModel};

mod hotkeys;
mod paste;
mod permissions;

use hotkeys::{SpaceHotkeyAction, current_mod_mask, space_hotkey_decision};
pub use paste::{PasteResult, insert_text, send_auto_submit};
pub use permissions::{
  PermissionStatus, accessibility_authorization, check_required_permissions_on_startup,
  ensure_accessibility_for_auto_paste, input_monitoring_authorization, microphone_authorization,
};

const KEYCODE_RETURN: u16 = 0x24;
// Virtual keycodes consumed by the HID event tap (Claim-on-press hotkeys).
const KEYCODE_SPACE: u16 = 0x31;
const KEYCODE_ESCAPE: u16 = 0x35;
const KEYCODE_NUMPAD_ENTER: u16 = 0x4C;
const KEYCODE_ARROW_UP: u16 = 0x7E;
const KEYCODE_ARROW_DOWN: u16 = 0x7D;
const KEYCODE_ARROW_LEFT: u16 = 0x7B;
const KEYCODE_ARROW_RIGHT: u16 = 0x7C;
const OVERLAY_WIDTH_MIN: f64 = 300.0;
const OVERLAY_WIDTH_MAX: f64 = 680.0;
const OVERLAY_HEIGHT_MIN: f64 = 64.0;
const OVERLAY_HEIGHT_MAX: f64 = 540.0;
const OVERLAY_STACK_GAP: f64 = 10.0;
const OVERLAY_CARD_RADIUS: f64 = 33.0;
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
const OVERLAY_WAVE_EDGE_INSET: f64 = OVERLAY_CARD_RADIUS - OVERLAY_PAD_X;
const OVERLAY_RAW_BADGE_FONT_SIZE: f64 = 12.0;
const OVERLAY_RAW_BADGE_WIDTH: f64 = 44.0;
const OVERLAY_RAW_BADGE_HEIGHT: f64 = 16.0;
const OVERLAY_RAW_BADGE_RIGHT_INSET: f64 = 14.0;
const OVERLAY_RAW_BADGE_BOTTOM_INSET: f64 = 9.0;
const OVERLAY_HOLD_BADGE_WIDTH: f64 = 46.0;
const OVERLAY_HOLD_BADGE_HEIGHT: f64 = 16.0;
const OVERLAY_BADGE_GAP: f64 = 8.0;
const DEVICE_HEADER_MIN_WIDTH: f64 = 220.0;
const DEVICE_HEADER_WIDTH: f64 = DEVICE_HEADER_MIN_WIDTH;
const DEVICE_HEADER_HEIGHT: f64 = 28.0;
const DEVICE_HEADER_TEXT_LEADING: f64 = 14.0;
const DEVICE_HEADER_ICON_SIZE: f64 = 16.0;
const DEVICE_HEADER_ICON_TO_LABEL_GAP: f64 = 2.0;
const DEVICE_HEADER_TRAILING: f64 = 12.0;
const DEVICE_HEADER_CHEVRON_SIZE: f64 = 10.0;
const DEVICE_HEADER_LABEL_TO_CHEVRON_GAP: f64 = 8.0;
const DEVICE_HEADER_LABEL_HEIGHT: f64 = 18.0;
const DEVICE_MENU_ROW_HEIGHT: f64 = 24.0;
const DEVICE_MENU_LABEL_DOWN_OFFSET: f64 = 1.0;
const DEVICE_HEADER_EXTRA_TOP_PADDING: f64 = 1.0;
const DEVICE_HEADER_EXTRA_SIDE_MARGIN: f64 = 2.0;
const ALWAYS_LISTENING_ROW_HEIGHT: f64 = 30.0;
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
const DEVICE_MENU_WIDTH_RIGHT_PADDING_RATIO: f64 = 0.10;
const DEVICE_MENU_SCREEN_EDGE_MARGIN: f64 = 24.0;
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
static DEVICE_ROW_VIEW_CLASS: OnceLock<&'static Class> = OnceLock::new();
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
// tap after macOS times it out. `SPACE_HOLD_CLAIMED` tracks whether we consumed a Space keydown
// for the listen hotkey. Once claimed, Azad owns that physical Space hold until keyup, even if
// the user releases the modifier first.
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
    static STATUS_ITEM_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static STATUS_MENU_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static STATUS_DELEGATE_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static SEARCH_FIELD_DELEGATE_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static ALWAYS_LISTENING_VIEW_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static ALWAYS_LISTENING_TRACK_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static ALWAYS_LISTENING_THUMB_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_VIEW_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_BUTTON_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_HIGHLIGHT_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_ICON_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_LABEL_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_CHEVRON_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_OPEN_MAX_WIDTH: RefCell<Option<f64>> = const { RefCell::new(None) };
    static DEVICE_MENU_OPEN_MAX_WIDTH: RefCell<Option<f64>> = const { RefCell::new(None) };
    static DEVICE_ROW_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static DEVICE_ROW_VIEW_REFS: RefCell<Vec<id>> = const { RefCell::new(Vec::new()) };
    static DEVICE_MENU_MODEL: RefCell<DeviceMenuModel> = RefCell::new(DeviceMenuModel::default());
    static HOTKEY_MANAGER_REF: RefCell<Option<GlobalHotKeyManager>> = const { RefCell::new(None) };
    // Short-TTL cache of the active-window display frame so ActiveWindow mode
    // doesn't do synchronous Accessibility IPC on every streaming reposition.
    static ACTIVE_WINDOW_SCREEN_CACHE: RefCell<Option<(NSRect, Instant)>> =
      const { RefCell::new(None) };
}

static STATUS_ITEM_VISIBLE: AtomicBool = AtomicBool::new(true);

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

pub fn set_device_menu(model: DeviceMenuModel) {
  DEVICE_MENU_MODEL.with(|slot| {
    slot.borrow_mut().clone_from(&model);
  });
  rebuild_status_menu();
}

pub fn set_status_item_visible(visible: bool) {
  STATUS_ITEM_VISIBLE.store(visible, Ordering::Release);
  unsafe {
    if let Some(status_item) = STATUS_ITEM_REF.with(|slot| *slot.borrow()) {
      apply_status_item_visibility(status_item, visible);
    }
  }
}

pub fn show_settings_window(model: SettingsViewModel) {
  crate::ui_bridge::show_settings_window(&model);
}

pub fn show_onboarding_window(model: OnboardingViewModel) {
  crate::ui_bridge::show_onboarding_window(&model);
}

pub fn sync_onboarding_listen_modifiers(mask: u8) {
  crate::ui_bridge::sync_listen_modifiers(mask);
}

pub fn sync_settings_listen_modifiers(mask: u8) {
  crate::ui_bridge::sync_listen_modifiers(mask);
}

pub fn close_onboarding_window() {
  crate::ui_bridge::close_onboarding_window();
}

pub fn update_onboarding_window(model: OnboardingViewModel) {
  crate::ui_bridge::update_onboarding_window(&model);
}

unsafe fn design_color(r: f64, g: f64, b: f64, a: f64) -> id {
  NSColor::colorWithCalibratedRed_green_blue_alpha_(
    nil,
    r.clamp(0.0, 1.0),
    g.clamp(0.0, 1.0),
    b.clamp(0.0, 1.0),
    a.clamp(0.0, 1.0),
  )
}

pub fn update_settings_window(model: SettingsViewModel) {
  crate::ui_bridge::update_settings_window(&model);
}

pub fn settings_window_is_open() -> bool {
  crate::ui_bridge::settings_window_is_open()
}

pub fn refresh_settings_permissions(accessibility: PermissionStatus, microphone: PermissionStatus) {
  crate::ui_bridge::refresh_settings_permissions(
    permission_status_for_ui(accessibility),
    permission_status_for_ui(microphone),
  );
}

fn permission_status_for_ui(status: PermissionStatus) -> crate::ui_model::UiPermissionStatus {
  match status {
    PermissionStatus::Granted => crate::ui_model::UiPermissionStatus::Granted,
    PermissionStatus::Denied | PermissionStatus::NotDetermined => {
      crate::ui_model::UiPermissionStatus::NotGranted
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

#[allow(clippy::too_many_arguments)]
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
#[allow(clippy::too_many_arguments)]
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

fn register_device_row_view_class() -> &'static Class {
  DEVICE_ROW_VIEW_CLASS.get_or_init(|| unsafe {
    let superclass = register_device_header_view_class();
    let mut decl =
      ClassDecl::new("AzadDeviceRowView", superclass).expect("failed to declare device row view");

    decl.add_ivar::<id>("highlightView");
    decl.add_ivar::<id>("checkView");
    decl.add_ivar::<id>("titleLabel");
    decl.add_ivar::<id>("trackingArea");
    decl.add_ivar::<i64>("deviceTag");
    decl.add_ivar::<u8>("rowEnabled");

    decl.add_method(
      sel!(hitTest:),
      device_row_view_hit_test as extern "C" fn(&Object, Sel, NSPoint) -> id,
    );
    decl.add_method(
      sel!(updateTrackingAreas),
      device_row_view_update_tracking_areas as extern "C" fn(&Object, Sel),
    );
    decl.add_method(
      sel!(mouseEntered:),
      device_row_view_mouse_entered as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(
      sel!(mouseExited:),
      device_row_view_mouse_exited as extern "C" fn(&Object, Sel, id),
    );
    decl.add_method(sel!(mouseUp:), device_row_view_mouse_up as extern "C" fn(&Object, Sel, id));

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

extern "C" fn device_row_view_hit_test(this: &Object, _: Sel, point: NSPoint) -> id {
  unsafe {
    if *this.get_ivar::<u8>("rowEnabled") == 0 {
      return nil;
    }

    let view_id = this as *const Object as id;
    let bounds: NSRect = msg_send![view_id, bounds];
    if point_is_in_rect(point, bounds) { view_id } else { nil }
  }
}

extern "C" fn device_row_view_update_tracking_areas(this: &Object, _: Sel) {
  unsafe {
    let view_id = this as *const Object as id;
    let _: () = msg_send![super(view_id, class!(NSView)), updateTrackingAreas];

    let existing: id = *this.get_ivar("trackingArea");
    if existing != nil {
      let _: () = msg_send![view_id, removeTrackingArea: existing];
    }

    if *this.get_ivar::<u8>("rowEnabled") == 0 {
      set_objc_ivar(view_id, "trackingArea", nil as id);
      return;
    }

    let bounds: NSRect = msg_send![view_id, bounds];
    let options: u64 = 1 | 128 | 512; // entered/exited, activeAlways, inVisibleRect
    let area: id = msg_send![class!(NSTrackingArea), alloc];
    let area: id = msg_send![
      area,
      initWithRect: bounds
      options: options
      owner: view_id
      userInfo: nil
    ];
    let _: () = msg_send![view_id, addTrackingArea: area];
    set_objc_ivar(view_id, "trackingArea", area);
  }
}

extern "C" fn device_row_view_mouse_entered(this: &Object, _: Sel, _: id) {
  unsafe {
    let view_id = this as *const Object as id;
    set_device_row_highlighted(view_id, true);
  }
}

extern "C" fn device_row_view_mouse_exited(this: &Object, _: Sel, _: id) {
  unsafe {
    let view_id = this as *const Object as id;
    set_device_row_highlighted(view_id, false);
  }
}

extern "C" fn device_row_view_mouse_up(this: &Object, _: Sel, _: id) {
  unsafe {
    if *this.get_ivar::<u8>("rowEnabled") == 0 {
      return;
    }
    let tag = *this.get_ivar::<i64>("deviceTag");
    select_device_by_tag(tag);
  }
}

fn point_is_in_rect(point: NSPoint, rect: NSRect) -> bool {
  point.x >= rect.origin.x
    && point.x < rect.origin.x + rect.size.width
    && point.y >= rect.origin.y
    && point.y < rect.origin.y + rect.size.height
}

unsafe fn set_objc_ivar<T: Encode>(obj: id, name: &str, value: T) {
  let class = &*objc::runtime::object_getClass(obj);
  let ivar = class
    .instance_variable(name)
    .unwrap_or_else(|| panic!("Ivar {name} not found on class {:?}", class));
  assert!(ivar.type_encoding() == T::encode());
  let ptr = (obj as *mut u8).offset(ivar.offset()) as *mut T;
  std::ptr::write(ptr, value);
}

unsafe fn set_device_row_highlighted(view: id, highlighted: bool) {
  if view == nil {
    return;
  }

  let obj = &*view;
  if *obj.get_ivar::<u8>("rowEnabled") == 0 {
    return;
  }

  let highlight_view: id = *obj.get_ivar("highlightView");
  if highlight_view != nil {
    if highlighted {
      let layer: id = msg_send![highlight_view, layer];
      if layer != nil {
        let highlight_color = resolved_menu_highlight_color_for_view(highlight_view);
        let highlight_cg_color: id = msg_send![highlight_color, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: highlight_cg_color];
      }
    }
    let _: () = msg_send![highlight_view, setHidden: if highlighted { NO } else { YES }];
  }

  let color: id = if highlighted {
    msg_send![class!(NSColor), selectedMenuItemTextColor]
  } else {
    msg_send![class!(NSColor), labelColor]
  };

  let check_view: id = *obj.get_ivar("checkView");
  if check_view != nil {
    set_image_view_tint_if_supported(check_view, color);
  }
  let title_label: id = *obj.get_ivar("titleLabel");
  if title_label != nil {
    let _: () = msg_send![title_label, setTextColor: color];
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

extern "C" fn select_device(_: &Object, _: Sel, sender: id) {
  unsafe {
    let tag: i64 = msg_send![sender, tag];
    select_device_by_tag(tag);
  }
}

fn select_device_by_tag(tag: i64) {
  if tag < 0 {
    return;
  }

  let selected = DEVICE_MENU_MODEL
    .with(|slot| slot.borrow().rows.get(tag as usize).map(|row| (row.id.clone(), row.checked)));
  if let Some((device_id, checked)) = selected {
    if checked {
      cancel_status_menu_tracking();
      return;
    }
    crate::app::send_event(AppEvent::MenuSelectDevice(device_id));
    cancel_status_menu_tracking();
  }
}

fn cancel_status_menu_tracking() {
  unsafe {
    if let Some(menu) = STATUS_MENU_REF.with(|slot| *slot.borrow()) {
      let _: () = msg_send![menu, cancelTracking];
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
  apply_status_item_visibility(status_item, STATUS_ITEM_VISIBLE.load(Ordering::Acquire));

  STATUS_ITEM_REF.with(|slot| {
    slot.borrow_mut().replace(status_item);
  });
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

  let delegate_obj = &mut *delegate;
  delegate_obj.set_ivar("statusItem", status_item);

  rebuild_status_menu();
}

unsafe fn apply_status_item_visibility(status_item: id, visible: bool) {
  let can_set_visible: i8 = msg_send![status_item, respondsToSelector: sel!(setVisible:)];
  if can_set_visible != 0 {
    let visible = if visible { YES } else { NO };
    let _: () = msg_send![status_item, setVisible: visible];
  }
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
  let min_width = DEVICE_HEADER_MIN_WIDTH.min(max_width);
  let adjusted = base_width.max(min_width).min(max_width);
  if adjusted.is_finite() && adjusted > 0.0 { adjusted } else { min_width }
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
      let view = (*slot.borrow())?;

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

  unsafe {
    ALWAYS_LISTENING_VIEW_REF.with(|slot| {
      if let Some(view) = *slot.borrow() {
        let _: () = msg_send![
            view,
            setFrame: NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(view_width, ALWAYS_LISTENING_ROW_HEIGHT)
            )
        ];
      }
    });

    DEVICE_HEADER_VIEW_REF.with(|slot| {
      if let Some(view) = *slot.borrow() {
        let _: () = msg_send![
            view,
            setFrame: NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(view_width, DEVICE_HEADER_HEIGHT)
            )
        ];
      }
    });

    DEVICE_ROW_VIEW_REFS.with(|rows| {
      for view in rows.borrow().iter().copied() {
        if view == nil {
          continue;
        }
        let _: () = msg_send![
            view,
            setFrame: NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(view_width, DEVICE_MENU_ROW_HEIGHT)
            )
        ];
      }
    });
  }
}

fn compute_device_menu_target_width(model: &DeviceMenuModel) -> f64 {
  unsafe {
    let font = menu_row_font();
    let mut content_width: f64 = 0.0;

    content_width = content_width.max(always_listening_row_width(font));
    content_width = content_width.max(menu_row_width_for_text("Quit Azad", font, 0, false));
    content_width = content_width.max(menu_row_width_for_text("Settings...", font, 0, false));

    if model.expanded {
      if model.rows.is_empty() {
        content_width =
          content_width.max(device_menu_row_width_for_label("No input devices", font));
      } else {
        for row in &model.rows {
          content_width = content_width.max(device_menu_row_width_for_label(&row.label, font));
        }
      }
    }

    content_width =
      content_width.max(device_header_width_for_label(&device_header_label(model), font));

    let screen_cap = menu_screen_width_cap();
    device_menu_width_with_padding(content_width)
      .max(DEVICE_HEADER_MIN_WIDTH)
      .min(screen_cap)
  }
}

fn device_menu_width_with_padding(content_width: f64) -> f64 {
  if !content_width.is_finite() || content_width <= 0.0 {
    return DEVICE_HEADER_MIN_WIDTH;
  }
  content_width + (content_width * DEVICE_MENU_WIDTH_RIGHT_PADDING_RATIO)
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
    + device_header_title_x()
    + DEVICE_HEADER_LABEL_TO_CHEVRON_GAP
    + DEVICE_HEADER_CHEVRON_SIZE
    + DEVICE_HEADER_TRAILING
    + DEVICE_MENU_TEXT_SAFETY_PADDING;
  width += 6.0 + (DEVICE_HEADER_EXTRA_SIDE_MARGIN * 2.0);
  width
}

unsafe fn device_menu_row_width_for_label(label: &str, font: id) -> f64 {
  device_header_title_x()
    + measure_text_width(label, font)
    + DEVICE_HEADER_TRAILING
    + DEVICE_MENU_TEXT_SAFETY_PADDING
}

fn device_header_title_x() -> f64 {
  DEVICE_HEADER_TEXT_LEADING + DEVICE_HEADER_ICON_SIZE + DEVICE_HEADER_ICON_TO_LABEL_GAP
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
  DEVICE_ROW_IDS.with(|rows| rows.borrow_mut().clear());
  DEVICE_ROW_VIEW_REFS.with(|rows| rows.borrow_mut().clear());

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
        NSString::alloc(nil).init_str(""),
      )
      .autorelease();
    settings_item.setTarget_(delegate);
    menu.addItem_(settings_item);

    let quit_item = NSMenuItem::alloc(nil)
      .initWithTitle_action_keyEquivalent_(
        NSString::alloc(nil).init_str("Quit Azad"),
        sel!(quit:),
        NSString::alloc(nil).init_str(""),
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
  let base_color = if enabled {
    design_color(0.02, 0.52, 0.55, 1.0)
  } else {
    design_color(0.48, 0.50, 0.52, 0.55)
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

fn centered_menu_axis_offset(container: f64, item: f64) -> f64 {
  ((container - item) * 0.5).max(0.0)
}

fn menu_label_axis_offset(container: f64, item: f64) -> f64 {
  (centered_menu_axis_offset(container, item) - DEVICE_MENU_LABEL_DOWN_OFFSET).max(0.0)
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
    "No Device".to_string()
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

unsafe fn system_symbol_image(name: &str, fallback_name: &str) -> id {
  let image_class = class!(NSImage);
  let symbol_name = NSString::alloc(nil).init_str(name);
  let supports_symbols: i8 = msg_send![image_class, respondsToSelector: sel!(imageWithSystemSymbolName:accessibilityDescription:)];
  if supports_symbols != 0 {
    let image: id =
      msg_send![image_class, imageWithSystemSymbolName: symbol_name accessibilityDescription: nil];
    if image != nil {
      return image;
    }
  }
  let fallback = NSString::alloc(nil).init_str(fallback_name);
  msg_send![image_class, imageNamed: fallback]
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

    DEVICE_HEADER_ICON_REF.with(|slot| {
      if let Some(image_view) = *slot.borrow() {
        let tint: id = if is_highlighted {
          msg_send![class!(NSColor), selectedMenuItemTextColor]
        } else {
          msg_send![class!(NSColor), secondaryLabelColor]
        };
        set_image_view_tint_if_supported(image_view, tint);
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

  ALWAYS_LISTENING_VIEW_REF.with(|slot| {
    slot.borrow_mut().replace(view);
  });
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

  let icon_x = DEVICE_HEADER_TEXT_LEADING;
  let icon_y = centered_menu_axis_offset(DEVICE_HEADER_HEIGHT, DEVICE_HEADER_ICON_SIZE);
  let icon_frame = NSRect::new(
    NSPoint::new(icon_x, icon_y),
    NSSize::new(DEVICE_HEADER_ICON_SIZE, DEVICE_HEADER_ICON_SIZE),
  );
  let icon_view: id = msg_send![class!(NSImageView), alloc];
  let icon_view: id = msg_send![icon_view, initWithFrame: icon_frame];
  let _: () =
    msg_send![icon_view, setImage: system_symbol_image("mic", "NSTouchBarAudioInputTemplate")];
  let icon_tint: id = msg_send![class!(NSColor), secondaryLabelColor];
  set_image_view_tint_if_supported(icon_view, icon_tint);

  let title_x = device_header_title_x();
  let title_width = DEVICE_HEADER_WIDTH
    - DEVICE_HEADER_TEXT_LEADING
    - DEVICE_HEADER_ICON_SIZE
    - DEVICE_HEADER_ICON_TO_LABEL_GAP
    - DEVICE_HEADER_TRAILING
    - DEVICE_HEADER_CHEVRON_SIZE
    - DEVICE_HEADER_LABEL_TO_CHEVRON_GAP;
  let title_label_frame = NSRect::new(
    NSPoint::new(title_x, menu_label_axis_offset(DEVICE_HEADER_HEIGHT, DEVICE_HEADER_LABEL_HEIGHT)),
    NSSize::new(title_width, DEVICE_HEADER_LABEL_HEIGHT),
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
  let chevron_y = centered_menu_axis_offset(DEVICE_HEADER_HEIGHT, DEVICE_HEADER_CHEVRON_SIZE);
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
  let _: () = msg_send![view, addSubview: icon_view];
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
  DEVICE_HEADER_ICON_REF.with(|slot| {
    slot.borrow_mut().replace(icon_view);
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
  DEVICE_ROW_VIEW_REFS.with(|rows| rows.borrow_mut().clear());
}

unsafe fn insert_device_rows(menu: id, _delegate: id, model: &DeviceMenuModel, mut insert_at: i64) {
  if !model.expanded {
    return;
  }

  if model.rows.is_empty() {
    let placeholder_item = make_device_row_item("No input devices", false, -1, false);
    let _: () = msg_send![menu, insertItem: placeholder_item atIndex: insert_at];
    return;
  }

  for row in &model.rows {
    let tag = DEVICE_ROW_IDS.with(|rows| {
      let mut rows = rows.borrow_mut();
      rows.push(row.id.clone());
      (rows.len() - 1) as i64
    });

    let row_item = make_device_row_item(&row.label, row.checked, tag, true);
    let _: () = msg_send![menu, insertItem: row_item atIndex: insert_at];
    insert_at += 1;
  }
}

unsafe fn make_device_row_item(label: &str, checked: bool, tag: i64, enabled: bool) -> id {
  let item = NSMenuItem::alloc(nil)
    .initWithTitle_action_keyEquivalent_(
      NSString::alloc(nil).init_str(""),
      sel!(noop:),
      NSString::alloc(nil).init_str(""),
    )
    .autorelease();
  let _: () = msg_send![item, setEnabled: if enabled { YES } else { NO }];

  let row_frame =
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(DEVICE_HEADER_WIDTH, DEVICE_MENU_ROW_HEIGHT));
  let row_view_class = register_device_row_view_class();
  let view: id = msg_send![row_view_class, alloc];
  let view: id = msg_send![view, initWithFrame: row_frame];
  let _: () = msg_send![view, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];

  let highlight_frame = NSRect::new(
    NSPoint::new(3.0 + DEVICE_HEADER_EXTRA_SIDE_MARGIN, 1.0),
    NSSize::new(
      DEVICE_HEADER_WIDTH - (6.0 + DEVICE_HEADER_EXTRA_SIDE_MARGIN * 2.0),
      DEVICE_MENU_ROW_HEIGHT - 2.0,
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

  let font: id = msg_send![class!(NSFont), menuFontOfSize: 0.0f64];
  let text_color: id = if enabled {
    msg_send![class!(NSColor), labelColor]
  } else {
    msg_send![class!(NSColor), disabledControlTextColor]
  };

  let text_y = menu_label_axis_offset(DEVICE_MENU_ROW_HEIGHT, DEVICE_HEADER_LABEL_HEIGHT);
  let check_frame = NSRect::new(
    NSPoint::new(DEVICE_HEADER_TEXT_LEADING, text_y),
    NSSize::new(DEVICE_HEADER_ICON_SIZE, DEVICE_HEADER_LABEL_HEIGHT),
  );
  let check_view: id = msg_send![class!(NSImageView), alloc];
  let check_view: id = msg_send![check_view, initWithFrame: check_frame];
  if checked {
    let image = system_symbol_image("checkmark", "NSMenuOnStateTemplate");
    if image != nil {
      let _: () = msg_send![check_view, setImage: image];
    }
  }
  set_image_view_tint_if_supported(check_view, text_color);

  let title_x = device_header_title_x();
  let title_width = DEVICE_HEADER_WIDTH - title_x - DEVICE_HEADER_TRAILING;
  let title_frame = NSRect::new(
    NSPoint::new(title_x, text_y),
    NSSize::new(title_width, DEVICE_HEADER_LABEL_HEIGHT),
  );
  let title_label: id = msg_send![class!(NSTextField), alloc];
  let title_label: id = msg_send![title_label, initWithFrame: title_frame];
  let _: () = msg_send![title_label, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];
  let _: () = msg_send![title_label, setStringValue: NSString::alloc(nil).init_str(label)];
  let _: () = msg_send![title_label, setBezeled: NO];
  let _: () = msg_send![title_label, setDrawsBackground: NO];
  let _: () = msg_send![title_label, setEditable: NO];
  let _: () = msg_send![title_label, setSelectable: NO];
  let _: () = msg_send![title_label, setAlignment: 0isize];
  let _: () = msg_send![title_label, setFont: font];
  let _: () = msg_send![title_label, setTextColor: text_color];

  let _: () = msg_send![view, addSubview: highlight_view];
  let _: () = msg_send![view, addSubview: check_view];
  let _: () = msg_send![view, addSubview: title_label];

  set_objc_ivar(view, "highlightView", highlight_view);
  set_objc_ivar(view, "checkView", check_view);
  set_objc_ivar(view, "titleLabel", title_label);
  set_objc_ivar(view, "trackingArea", nil as id);
  set_objc_ivar(view, "deviceTag", tag);
  set_objc_ivar(view, "rowEnabled", if enabled { 1_u8 } else { 0_u8 });
  let _: () = msg_send![view, updateTrackingAreas];

  DEVICE_ROW_VIEW_REFS.with(|rows| rows.borrow_mut().push(view));

  let _: () = msg_send![item, setView: view];
  item
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
    let mid = (lo + hi).div_ceil(2);
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

#[allow(clippy::too_many_arguments)]
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
    let card_color = design_color(0.035, 0.040, 0.046, 0.94);
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

#[allow(clippy::too_many_arguments)]
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
  let display_text = overlay_display_text(body_text, busy_phase.is_some());

  // Space reserved at the top of the card for the connector chip, folded into the
  // body height budget so the transcription drops below it.
  let show_chip = !connector_tag.is_empty();
  let chip_reserve =
    if show_chip { OVERLAY_CONNECTOR_CHIP_HEIGHT + OVERLAY_CONNECTOR_CHIP_GAP } else { 0.0 };

  let max_body_height =
    (OVERLAY_HEIGHT_MAX - OVERLAY_PAD_TOP - OVERLAY_PAD_BOTTOM - chip_reserve).max(1.0);

  let (rendered_body, mut measured_body_height) =
    fit_rendered_body_for_height(refs.label, display_text, content_width, max_body_height);
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
    let card_color = design_color(0.035, 0.040, 0.046, 0.94);
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

fn overlay_display_text(body_text: &str, finalizing: bool) -> &str {
  if body_text.trim().is_empty() && finalizing { "Finalizing" } else { body_text }
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
// The inner highlight is inset 8 pt from the card edge, so this follows the outer card's
// rounded curve concentrically (the bg is inset 8 pt from the card edge:
// `OVERLAY_PAD_X (12) - HISTORY_BG_X_INSET (4) = 8`). Smaller values leave a
// visible gap between the highlight corner and the card corner; larger values
// clip past the curve.
const HISTORY_BG_RADIUS: f64 = OVERLAY_CARD_RADIUS - 8.0;
// Symmetric padding inside the highlight bg between its left/right edges and
// the entry text. Keeps the text from butting against the rounded bg corner.
const HISTORY_TEXT_INNER_PAD_X: f64 = 12.0;
// Teal selection fill so the active history row reads as part of the same
// accent system as the listening switch and overlay border.
const HISTORY_SELECTED_BG_R: f64 = 0.02;
const HISTORY_SELECTED_BG_G: f64 = 0.42;
const HISTORY_SELECTED_BG_B: f64 = 0.48;
const HISTORY_SELECTED_BG_ALPHA: f64 = 0.78;
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
  for (vis_idx, (rendered, body_h)) in measured.iter().enumerate() {
    let entry_idx = start + vis_idx;
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
    let mid = lo + (hi - lo).div_ceil(2);
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
    let card_color = design_color(0.035, 0.040, 0.046, 0.94);
    let card_cg: id = msg_send![card_color, CGColor];
    let _: () = msg_send![card_layer, setBackgroundColor: card_cg];
    // Carry the subtle outer border forward from speech mode so the history
    // overlay doesn't visually drop a pixel of definition when the user
    // pivots into it.
    let border = design_color(0.26, 0.70, 0.74, 0.28);
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
  let (mut x, mut y) = if force_default_anchor {
    (
      target_screen.origin.x + (target_screen.size.width - width) * 0.5,
      target_screen.origin.y + target_screen.size.height * 0.08,
    )
  } else if let Some(source) = current_screen {
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
  } else {
    (
      target_screen.origin.x + (target_screen.size.width - width) * 0.5,
      target_screen.origin.y + target_screen.size.height * 0.08,
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
  let side_inset = wave_edge_inset_for_width(width);
  let drawable_width = (width - side_inset * 2.0).max(1.0);
  let spacing = (drawable_width / count as f64).max(1.0);
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
    let x = side_inset + i as f64 * spacing + (spacing - bar_width) * 0.5;
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

fn wave_edge_inset_for_width(width: f64) -> f64 {
  OVERLAY_WAVE_EDGE_INSET.min((width * 0.18).max(0.0))
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

  let subtle = design_color(0.26, 0.70, 0.74, 0.28);
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
  let card_color = design_color(0.035, 0.040, 0.046, 0.94);
  let cg_color: id = msg_send![card_color, CGColor];
  let _: () = msg_send![card_layer, setBackgroundColor: cg_color];
  let _: () = msg_send![card_layer, setCornerRadius: OVERLAY_CARD_RADIUS];
  let _: () = msg_send![card_layer, setMasksToBounds: YES];
  let subtle_border = design_color(0.26, 0.70, 0.74, 0.28);
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
      (0.84, 0.99, 0.96, 1.0),
      (0.30, 0.82, 0.82, 0.98),
      (0.08, 0.58, 0.62, 0.62),
      (0.76, 0.42, 0.28, 0.18),
      (0.84, 0.99, 0.96, 1.0),
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
  let _: () = msg_send![
      search_field,
      setPlaceholderString: NSString::alloc(nil).init_str("Search history")
  ];
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
  // Listen hotkey: a non-autorepeat matching Space keydown dispatches a hold
  // press. Once that physical Space hold is claimed, every later Space event in
  // the hold is swallowed until keyup, even if the modifier is released first.
  // Bare unclaimed Space continues to pass through to the focused app.
  if keycode == KEYCODE_SPACE {
    // Listen hotkey: Space plus the user-configured modifier set. Superset-match
    // (all wanted modifiers held; extras OK). `wanted != 0` guards against a
    // corrupt empty mask turning bare Space into a global trigger.
    let wanted = LISTEN_MODIFIERS.load(Ordering::Acquire);
    let live = current_mod_mask(is_option, is_shift, is_command, is_control);
    let decision = space_hotkey_decision(
      wanted,
      SPACE_HOLD_CLAIMED.load(Ordering::Acquire),
      live,
      is_keydown,
      is_autorepeat,
    );
    SPACE_HOLD_CLAIMED.store(decision.claimed_after, Ordering::Release);
    match decision.action {
      SpaceHotkeyAction::PassThrough => return false,
      SpaceHotkeyAction::ClaimOnly => return true,
      SpaceHotkeyAction::Press => {
        crate::app::send_event(AppEvent::HotkeyPressed);
        return true;
      }
      SpaceHotkeyAction::Release { raw_requested } => {
        crate::app::send_event(AppEvent::HotkeyReleased { raw_requested });
        return true;
      }
    }
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
      HotKeyState::Released => {
        crate::app::send_event(AppEvent::HotkeyReleased { raw_requested: is_raw_mode_pressed() })
      }
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

// AXValueType tags from <ApplicationServices/.../AXValue.h>.
const KAX_VALUE_CG_POINT_TYPE: u32 = 1;
const KAX_VALUE_CG_SIZE_TYPE: u32 = 2;

unsafe extern "C" {
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

#[allow(clippy::duplicated_attributes)]
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

#[cfg(test)]
mod tests {
  use super::{OVERLAY_WAVE_EDGE_INSET, overlay_display_text, wave_edge_inset_for_width};

  #[test]
  fn empty_overlay_body_stays_empty_while_holding_to_listen() {
    assert_eq!(overlay_display_text("", false), "");
    assert_eq!(overlay_display_text("   ", false), "   ");
  }

  #[test]
  fn empty_overlay_body_shows_finalizing_when_busy() {
    assert_eq!(overlay_display_text("", true), "Finalizing");
  }

  #[test]
  fn wave_edge_inset_keeps_bars_inside_overlay_edges() {
    assert_eq!(wave_edge_inset_for_width(300.0), OVERLAY_WAVE_EDGE_INSET);
    assert!(wave_edge_inset_for_width(80.0) < OVERLAY_WAVE_EDGE_INSET);
  }
}
