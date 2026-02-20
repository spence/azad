use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cocoa::appkit::{
    NSApp, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSImage,
    NSMainMenuWindowLevel, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSScreen,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
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

const KEYCODE_V: u16 = 0x09;
const KEYCODE_LEFT_COMMAND: u16 = 0x37;
const KEYCODE_RIGHT_COMMAND: u16 = 0x36;
const KEYCODE_LEFT_SHIFT: u16 = 0x38;
const KEYCODE_RIGHT_SHIFT: u16 = 0x3C;
const KEYCODE_LEFT_OPTION: u16 = 0x3A;
const KEYCODE_RIGHT_OPTION: u16 = 0x3D;
const KEYCODE_LEFT_CONTROL: u16 = 0x3B;
const KEYCODE_RIGHT_CONTROL: u16 = 0x3E;
const PASTE_CHORD_HOLD_MS: u64 = 100;
const POST_PASTE_SETTLE_MS: u64 = 50;
const OVERLAY_WIDTH_MIN: f64 = 300.0;
const OVERLAY_WIDTH_MAX: f64 = 620.0;
const OVERLAY_HEIGHT_MIN: f64 = 60.0;
const OVERLAY_HEIGHT_MAX: f64 = 380.0;
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
const SETTINGS_WINDOW_WIDTH: f64 = 720.0;
const SETTINGS_WINDOW_HEIGHT: f64 = 460.0;
const SETTINGS_INSET_X: f64 = 20.0;
const SETTINGS_TOP_MARGIN: f64 = 18.0;
const SETTINGS_CONTROL_HEIGHT: f64 = 24.0;
const SETTINGS_REFRESH_WIDTH: f64 = 90.0;
const SETTINGS_METRICS_TOP_GAP: f64 = 14.0;

// NSAutoresizingMaskOptions (see AppKit NSView.h)
const NS_VIEW_MIN_X_MARGIN: u64 = 1 << 0;
const NS_VIEW_WIDTH_SIZABLE: u64 = 1 << 1;
const NS_VIEW_HEIGHT_SIZABLE: u64 = 1 << 4;
const NSEVENT_MODIFIER_FLAG_OPTION: u64 = 1 << 19;
const HOLD_HOTKEY_MODIFIERS: Modifiers = Modifiers::ALT;
const HOLD_HOTKEY_KEY: Code = Code::Space;

static DELEGATE_CLASS: OnceLock<&'static Class> = OnceLock::new();
static OVERLAY_WINDOW_CLASS: OnceLock<&'static Class> = OnceLock::new();
static DEVICE_HEADER_VIEW_CLASS: OnceLock<&'static Class> = OnceLock::new();
static HOTKEY_OPTION_SPACE_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ESCAPE_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ENTER_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ENTER_OPTION_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_NUMPAD_ENTER_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_NUMPAD_ENTER_OPTION_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ESCAPE_REGISTERED: AtomicBool = AtomicBool::new(false);
static HOTKEY_ENTER_REGISTERED: AtomicBool = AtomicBool::new(false);
static OPENED_ACCESSIBILITY_SETTINGS: AtomicBool = AtomicBool::new(false);

thread_local! {
    static OVERLAY_REFS: RefCell<Option<OverlayRefs>> = const { RefCell::new(None) };
    static OVERLAY_TOP_REFS: RefCell<Option<OverlayRefs>> = const { RefCell::new(None) };
    static STATUS_MENU_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static STATUS_DELEGATE_REF: RefCell<Option<id>> = const { RefCell::new(None) };
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
}

#[derive(Clone, Copy)]
struct OverlayRefs {
    window: id,
    card_view: id,
    label: id,
    hold_badge: id,
    raw_badge: id,
    meter_view: id,
    wave_bars: [id; OVERLAY_WAVE_BAR_COUNT],
    busy_gradient_layer: id,
    busy_mask_layer: id,
}

#[derive(Clone, Copy)]
struct SettingsWindowRefs {
    window: id,
    debug_checkbox: id,
    metrics_text_view: id,
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
    pub debug_stats_enabled: bool,
    pub metrics_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteResult {
    Pasted,
    EmptyText,
    ClipboardWriteFailed,
    AccessibilityRequired,
}

pub fn run_app() {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let app = NSApp();
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );

        let delegate_class = register_delegate_class();
        let delegate: id = msg_send![delegate_class, new];

        STATUS_DELEGATE_REF.with(|r| {
            r.borrow_mut().replace(delegate);
        });

        setup_status_bar(delegate);
        install_global_hotkeys();

        let _: () = msg_send![app, setDelegate: delegate];
        app.run();
    }
}

pub fn check_required_permissions_on_startup() {
    if !is_accessibility_trusted() {
        maybe_request_accessibility_permission_once();
        eprintln!(
            "Azad: Accessibility permission missing at startup. Enable Azad in System Settings -> Privacy & Security -> Accessibility."
        );
    }
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
        apply_settings_view_model(refs, &model);
        let app = NSApp();
        let _: () = msg_send![app, activateIgnoringOtherApps: YES];
        let _: () = msg_send![refs.window, makeKeyAndOrderFront: nil];
    }
}

pub fn update_settings_window(model: SettingsViewModel) {
    if let Some(refs) = current_settings_window() {
        unsafe {
            apply_settings_view_model(refs, &model);
        }
    }
}

pub fn show_overlay() {
    unsafe {
        let refs = ensure_overlay();
        move_overlay_to_cursor_screen(refs, true);
        let _: () = msg_send![refs.window, orderFrontRegardless];
    }
    set_escape_hotkey_enabled(true);
    set_enter_hotkey_enabled(true);
}

pub fn show_overlay_top() {
    unsafe {
        let refs = ensure_overlay_top();
        if let Some(bottom) = current_overlay() {
            position_overlay_top_relative_to_bottom(refs, bottom);
        } else {
            move_overlay_to_cursor_screen(refs, true);
        }
        let _: () = msg_send![refs.window, orderFrontRegardless];
    }
}

pub fn hide_overlay() {
    hide_overlay_top();
    if let Some(refs) = current_overlay() {
        unsafe {
            let _: () = msg_send![refs.window, orderOut: nil];
            let app = NSApp();
            let _: () = msg_send![app, updateWindows];
        }
    }
    set_escape_hotkey_enabled(false);
    set_enter_hotkey_enabled(false);
}

pub fn hide_overlay_top() {
    if let Some(refs) = current_overlay_top() {
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
) {
    let Some(refs) = current_overlay() else {
        return;
    };
    unsafe {
        move_overlay_to_cursor_screen(refs, false);
        render_overlay_text(
            refs,
            draft,
            activity,
            busy_phase,
            show_raw_badge,
            show_hold_badge,
        );
    }
}

pub fn set_overlay_top_stream_content(
    draft: &str,
    activity: &[f32],
    busy_phase: Option<f32>,
) {
    let Some(refs) = current_overlay_top() else {
        return;
    };
    unsafe {
        if let Some(bottom) = current_overlay() {
            position_overlay_top_relative_to_bottom(refs, bottom);
        }
        render_overlay_text(refs, draft, activity, busy_phase, false, false);
        if let Some(bottom) = current_overlay() {
            position_overlay_top_relative_to_bottom(refs, bottom);
        }
    }
}

pub fn set_overlay_notice_content(title: &str, body: &str) {
    let Some(refs) = current_overlay() else {
        return;
    };
    let title = title.trim();
    let body = body.trim();
    let rendered = if body.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n{body}")
    };

    unsafe {
        move_overlay_to_cursor_screen(refs, false);
        render_overlay_text(refs, &rendered, &[], None, false, false);
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
    HOLD_HOTKEY_MODIFIERS.contains(Modifiers::ALT)
}

pub fn paste_text(text: &str, paste_delay_ms: u64) -> PasteResult {
    if text.trim().is_empty() {
        return PasteResult::EmptyText;
    }

    unsafe {
        if !write_pasteboard_string(text) {
            eprintln!("Azad: failed to write transcript to pasteboard");
            return PasteResult::ClipboardWriteFailed;
        }
    }

    if !is_accessibility_trusted() {
        maybe_request_accessibility_permission_once();
        eprintln!(
            "Azad: paste skipped; grant Accessibility to Azad in System Settings -> Privacy & Security -> Accessibility"
        );
        return PasteResult::AccessibilityRequired;
    }

    // Clipboard propagation delay so the focused target app sees the new clipboard value.
    std::thread::sleep(Duration::from_millis(paste_delay_ms));

    unsafe {
        send_command_v_robust();
    }
    // Give the target app a short settle window after synthetic paste.
    std::thread::sleep(Duration::from_millis(POST_PASTE_SETTLE_MS));

    PasteResult::Pasted
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
        decl.add_method(
            sel!(toggleDevices:),
            toggle_devices as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(openSettings:),
            open_settings as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(settingsToggleDebug:),
            settings_toggle_debug as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(settingsRefresh:),
            settings_refresh as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(selectDevice:),
            select_device as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(menuWillOpen:),
            menu_will_open as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(menuDidClose:),
            menu_did_close as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(menu:willHighlightItem:),
            menu_will_highlight_item as extern "C" fn(&Object, Sel, id, id),
        );
        decl.add_method(
            sel!(syncMenuLayout:),
            sync_menu_layout as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(sel!(noop:), noop as extern "C" fn(&Object, Sel, id));

        decl.register()
    })
}

fn register_overlay_window_class() -> &'static Class {
    OVERLAY_WINDOW_CLASS.get_or_init(|| unsafe {
        let superclass = class!(NSWindow);
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
        decl.add_method(
            sel!(keyDown:),
            overlay_key_down as extern "C" fn(&Object, Sel, id),
        );

        decl.register()
    })
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

extern "C" fn settings_refresh(_: &Object, _: Sel, _: id) {
    crate::app::send_event(AppEvent::SettingsRefresh);
    crate::app::drain_events();
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
            DEVICE_HEADER_VIEW_REF.with(|slot| {
                slot.borrow()
                    .is_some_and(|header_view| header_view == item_view)
            })
        }
    };
    set_device_header_highlighted(is_header);
}

extern "C" fn sync_menu_layout(_: &Object, _: Sel, _: id) {
    sync_device_header_width_to_live_menu();
}

extern "C" fn noop(_: &Object, _: Sel, _: id) {}

extern "C" fn overlay_can_become_key_window(_: &Object, _: Sel) -> bool {
    false
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
    let menu_width = if menu_is_open(menu) {
        sticky_menu_open_width(width)
    } else {
        width
    };

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
    let final_width = if menu_is_open(menu) {
        sticky_header_open_width(row_width)
    } else {
        row_width
    };
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
    if adjusted.is_finite() && adjusted > 0.0 {
        adjusted
    } else {
        base_width
    }
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
                max_width =
                    max_width.max(menu_row_width_for_text("No input devices", font, 1, false));
            } else {
                for row in &model.rows {
                    max_width = max_width.max(menu_row_width_for_text(&row.label, font, 1, true));
                }
            }
        }

        max_width = max_width.max(device_header_width_for_label(
            &device_header_label(model),
            font,
        ));

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
        if cap.is_finite() && cap > DEVICE_HEADER_MIN_WIDTH {
            cap
        } else {
            DEVICE_HEADER_WIDTH * 2.5
        }
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
        NSSize::new(
            ALWAYS_LISTENING_SWITCH_THUMB_SIZE,
            ALWAYS_LISTENING_SWITCH_THUMB_SIZE,
        ),
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
    if model.expanded {
        "NSTouchBarGoDownTemplate"
    } else {
        "NSGoRightTemplate"
    }
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
    let label_frame = NSRect::new(
        NSPoint::new(ALWAYS_LISTENING_LABEL_LEADING, 2.0),
        NSSize::new(label_width, 18.0),
    );
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
        NSSize::new(
            ALWAYS_LISTENING_SWITCH_WIDTH,
            ALWAYS_LISTENING_SWITCH_HEIGHT,
        ),
    );
    let switch_container: id = msg_send![class!(NSView), alloc];
    let switch_container: id = msg_send![switch_container, initWithFrame: switch_frame];
    let _: () = msg_send![switch_container, setAutoresizingMask: NS_VIEW_MIN_X_MARGIN];
    let _: () = msg_send![switch_container, setWantsLayer: YES];

    let track_frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(
            ALWAYS_LISTENING_SWITCH_WIDTH,
            ALWAYS_LISTENING_SWITCH_HEIGHT,
        ),
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
    let thumb_view: id =
        msg_send![thumb_view, initWithFrame: always_listening_thumb_frame(enabled)];
    let _: () = msg_send![thumb_view, setWantsLayer: YES];
    let thumb_layer: id = msg_send![thumb_view, layer];
    if thumb_layer != nil {
        let _: () =
            msg_send![thumb_layer, setCornerRadius: (ALWAYS_LISTENING_SWITCH_THUMB_SIZE * 0.5)];
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

    let view_frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(DEVICE_HEADER_WIDTH, DEVICE_HEADER_HEIGHT),
    );
    let header_view_class = register_device_header_view_class();
    let view: id = msg_send![header_view_class, alloc];
    let view: id = msg_send![view, initWithFrame: view_frame];
    let _: () = msg_send![view, setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE];

    let highlight_frame = NSRect::new(
        NSPoint::new(
            3.0 + DEVICE_HEADER_EXTRA_SIDE_MARGIN,
            1.0 + DEVICE_HEADER_EXTRA_TOP_PADDING,
        ),
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

    let button_frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(DEVICE_HEADER_WIDTH, DEVICE_HEADER_HEIGHT),
    );
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
        NSPoint::new(
            DEVICE_HEADER_TEXT_LEADING,
            2.0 + DEVICE_HEADER_EXTRA_TOP_PADDING,
        ),
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
        top_frame
            .size
            .height
            .clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX)
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

unsafe fn apply_settings_view_model(refs: SettingsWindowRefs, model: &SettingsViewModel) {
    let checkbox_state: i64 = if model.debug_stats_enabled { 1 } else { 0 };
    let _: () = msg_send![refs.debug_checkbox, setState: checkbox_state];

    let metrics = NSString::alloc(nil).init_str(&model.metrics_text);
    let _: () = msg_send![refs.metrics_text_view, setString: metrics];
}

unsafe fn render_overlay_text(
    refs: OverlayRefs,
    body_text: &str,
    activity: &[f32],
    busy_phase: Option<f32>,
    show_raw_badge: bool,
    show_hold_badge: bool,
) {
    let current_frame: NSRect = msg_send![refs.window, frame];
    let screen = overlay_screen_frame_for_window(current_frame);
    let width = overlay_width_for_screen(screen);
    let max_body_height = (OVERLAY_HEIGHT_MAX - OVERLAY_PAD_TOP - OVERLAY_PAD_BOTTOM).max(1.0);
    let content_width = (width - OVERLAY_PAD_X * 2.0).max(1.0);

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
    let content_height = OVERLAY_PAD_TOP + body_height + OVERLAY_PAD_BOTTOM;
    let height = content_height.clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX);

    let default_x = screen.origin.x + (screen.size.width - width) * 0.5;
    let default_y = screen.origin.y + screen.size.height * 0.08;
    let x = if current_frame.size.width <= 0.0 {
        default_x
    } else {
        current_frame.origin.x
    };
    let y = if current_frame.size.height <= 0.0 {
        default_y
    } else {
        current_frame.origin.y
    };
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

    apply_busy_border_style(refs, busy_phase, width, height);

    let available_height = (height - OVERLAY_PAD_TOP - OVERLAY_PAD_BOTTOM).max(1.0);
    let body_text_height = body_height.min(available_height).max(1.0);
    let body_y = if is_single_line {
        OVERLAY_PAD_BOTTOM + ((available_height - body_text_height) * 0.5).max(0.0)
    } else {
        OVERLAY_PAD_BOTTOM
    };
    let meter_height = body_text_height
        .max(OVERLAY_WAVE_BG_HEIGHT)
        .min(available_height)
        .max(1.0);
    let meter_y = OVERLAY_PAD_BOTTOM;
    let body_frame = NSRect::new(
        NSPoint::new(OVERLAY_PAD_X, body_y),
        NSSize::new(content_width, body_text_height),
    );
    let meter_frame = NSRect::new(
        NSPoint::new(OVERLAY_PAD_X, meter_y),
        NSSize::new(content_width, meter_height),
    );
    let _: () = msg_send![refs.label, setFrame: body_frame];
    let _: () = msg_send![refs.meter_view, setFrame: meter_frame];
    let mut badge_right =
        (width - OVERLAY_RAW_BADGE_RIGHT_INSET).max(OVERLAY_RAW_BADGE_RIGHT_INSET);
    if show_raw_badge {
        let raw_badge_x =
            (badge_right - OVERLAY_RAW_BADGE_WIDTH).max(OVERLAY_RAW_BADGE_RIGHT_INSET);
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
        let hold_badge_x =
            (badge_right - OVERLAY_HOLD_BADGE_WIDTH).max(OVERLAY_RAW_BADGE_RIGHT_INSET);
        let hold_badge_frame = NSRect::new(
            NSPoint::new(hold_badge_x, OVERLAY_RAW_BADGE_BOTTOM_INSET),
            NSSize::new(OVERLAY_HOLD_BADGE_WIDTH, OVERLAY_HOLD_BADGE_HEIGHT),
        );
        let _: () = msg_send![refs.hold_badge, setFrame: hold_badge_frame];
        let _: () = msg_send![refs.hold_badge, setHidden: NO];
    } else {
        let _: () = msg_send![refs.hold_badge, setHidden: YES];
    }

    let _: () = msg_send![refs.label, setAlignment: 1isize];
    let _: () =
        msg_send![refs.label, setStringValue: NSString::alloc(nil).init_str(&rendered_body)];
    render_activity_wave(
        refs,
        activity,
        meter_frame.size.width,
        meter_frame.size.height,
    );
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

unsafe fn move_overlay_to_cursor_screen(refs: OverlayRefs, force_default_anchor: bool) {
    if !force_default_anchor && is_left_mouse_button_down() {
        return;
    }

    let target_screen = cursor_screen_frame().unwrap_or_else(main_screen_frame);
    let current_frame: NSRect = msg_send![refs.window, frame];
    let current_screen = current_overlay_screen_frame(current_frame);
    if !force_default_anchor
        && current_screen.is_some_and(|screen| same_screen_frame(screen, target_screen))
    {
        return;
    }

    let width = overlay_width_for_screen(target_screen);
    let height = if current_frame.size.height > 0.0 {
        current_frame
            .size
            .height
            .clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX)
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

unsafe fn measure_label_height(label: id, text: &str, width: f64) -> f64 {
    let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str(text)];
    let cell: id = msg_send![label, cell];
    if cell == nil {
        return OVERLAY_TEXT_LINE_HEIGHT;
    }
    let probe_bounds = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(width.max(1.0), OVERLAY_HEIGHT_MAX * 4.0),
    );
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
        let bar_h = (OVERLAY_WAVE_BAR_MIN_HEIGHT
            + dramatic * (max_h - OVERLAY_WAVE_BAR_MIN_HEIGHT))
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

    let frame = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(width.max(1.0), height.max(1.0)),
    );
    let _: () = msg_send![refs.busy_gradient_layer, setFrame: frame];
    let _: () = msg_send![refs.busy_mask_layer, setFrame: frame];
    let _: () = msg_send![refs.busy_mask_layer, setCornerRadius: OVERLAY_CARD_RADIUS];
    let _: () = msg_send![refs.busy_mask_layer, setBorderWidth: OVERLAY_BUSY_RING_THICKNESS];

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
    let window_frame = NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(SETTINGS_WINDOW_WIDTH, SETTINGS_WINDOW_HEIGHT),
    );

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

    let top_y = SETTINGS_WINDOW_HEIGHT - SETTINGS_TOP_MARGIN - SETTINGS_CONTROL_HEIGHT;
    let checkbox_frame = NSRect::new(
        NSPoint::new(SETTINGS_INSET_X, top_y),
        NSSize::new(320.0, SETTINGS_CONTROL_HEIGHT),
    );
    let debug_checkbox: id = msg_send![class!(NSButton), alloc];
    let debug_checkbox: id = msg_send![debug_checkbox, initWithFrame: checkbox_frame];
    let _: () = msg_send![debug_checkbox, setButtonType: 3usize];
    let _: () = msg_send![debug_checkbox, setTitle: NSString::alloc(nil).init_str("Enable debug statistics")];
    let _: () = msg_send![debug_checkbox, setAction: sel!(settingsToggleDebug:)];

    let refresh_x = SETTINGS_WINDOW_WIDTH - SETTINGS_INSET_X - SETTINGS_REFRESH_WIDTH;
    let refresh_frame = NSRect::new(
        NSPoint::new(refresh_x, top_y),
        NSSize::new(SETTINGS_REFRESH_WIDTH, SETTINGS_CONTROL_HEIGHT),
    );
    let refresh_button: id = msg_send![class!(NSButton), alloc];
    let refresh_button: id = msg_send![refresh_button, initWithFrame: refresh_frame];
    let _: () = msg_send![refresh_button, setBezelStyle: 1usize];
    let _: () = msg_send![refresh_button, setTitle: NSString::alloc(nil).init_str("Refresh")];
    let _: () = msg_send![refresh_button, setAction: sel!(settingsRefresh:)];

    let metrics_height =
        (top_y - SETTINGS_METRICS_TOP_GAP - SETTINGS_INSET_X).max(SETTINGS_CONTROL_HEIGHT * 2.0);
    let scroll_frame = NSRect::new(
        NSPoint::new(SETTINGS_INSET_X, SETTINGS_INSET_X),
        NSSize::new(SETTINGS_WINDOW_WIDTH - SETTINGS_INSET_X * 2.0, metrics_height),
    );
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
    let _: () = msg_send![metrics_text_view, setString: NSString::alloc(nil).init_str("")];
    let _: () = msg_send![scroll_view, setDocumentView: metrics_text_view];

    if let Some(delegate) = STATUS_DELEGATE_REF.with(|slot| *slot.borrow()) {
        let _: () = msg_send![debug_checkbox, setTarget: delegate];
        let _: () = msg_send![refresh_button, setTarget: delegate];
    }

    let _: () = msg_send![content_view, addSubview: debug_checkbox];
    let _: () = msg_send![content_view, addSubview: refresh_button];
    let _: () = msg_send![content_view, addSubview: scroll_view];

    SettingsWindowRefs {
        window,
        debug_checkbox,
        metrics_text_view,
    }
}

unsafe fn create_overlay_window(read_only: bool) -> OverlayRefs {
    let frame = cursor_screen_frame().unwrap_or_else(main_screen_frame);

    let overlay_width = overlay_width_for_screen(frame);
    let overlay_height = OVERLAY_HEIGHT_MIN;
    let x = frame.origin.x + (frame.size.width - overlay_width) * 0.5;
    let y = frame.origin.y + frame.size.height * 0.08;

    let overlay_frame = NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(overlay_width, overlay_height),
    );

    let overlay_class = register_overlay_window_class();
    let window: id = msg_send![overlay_class, alloc];
    let window: id = msg_send![window, initWithContentRect: overlay_frame
                                                styleMask: NSWindowStyleMask::NSBorderlessWindowMask
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

    let busy_gradient_layer: id = msg_send![class!(CAGradientLayer), layer];
    let busy_mask_layer: id = msg_send![class!(CALayer), layer];
    if busy_gradient_layer != nil && busy_mask_layer != nil {
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(overlay_width, overlay_height),
        );
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

    let refs = OverlayRefs {
        window,
        card_view,
        label,
        hold_badge,
        raw_badge,
        meter_view,
        wave_bars,
        busy_gradient_layer,
        busy_mask_layer,
    };
    render_overlay_text(refs, "", &[], None, false, false);
    refs
}

fn install_global_hotkeys() {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(err) => {
            eprintln!("Azad: failed to initialize global hotkey manager: {}", err);
            return;
        }
    };

    let hotkey = HotKey::new(Some(HOLD_HOTKEY_MODIFIERS), HOLD_HOTKEY_KEY);
    let hotkey_id = hotkey.id();

    if let Err(err) = manager.register(hotkey) {
        eprintln!(
            "Azad: failed to register Option+Space hotkey (might be in use): {}",
            err
        );
        return;
    }

    let _ = HOTKEY_OPTION_SPACE_ID.set(hotkey_id);

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

    GlobalHotKeyEvent::set_event_handler(Some(|event| {
        handle_global_hotkey_event(event);
    }));

    HOTKEY_MANAGER_REF.with(|slot| {
        slot.borrow_mut().replace(manager);
    });
}

fn handle_global_hotkey_event(event: GlobalHotKeyEvent) {
    if let Some(option_space_id) = HOTKEY_OPTION_SPACE_ID.get().copied() {
        if event.id == option_space_id {
            match event.state {
                HotKeyState::Pressed => crate::app::send_event(AppEvent::HotkeyPressed),
                HotKeyState::Released => crate::app::send_event(AppEvent::HotkeyReleased),
            }
            return;
        }
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
        || HOTKEY_ENTER_OPTION_ID
            .get()
            .is_some_and(|id| event.id == *id)
        || HOTKEY_NUMPAD_ENTER_ID
            .get()
            .is_some_and(|id| event.id == *id)
        || HOTKEY_NUMPAD_ENTER_OPTION_ID
            .get()
            .is_some_and(|id| event.id == *id);
    let is_option_enter_hotkey = HOTKEY_ENTER_OPTION_ID
        .get()
        .is_some_and(|id| event.id == *id)
        || HOTKEY_NUMPAD_ENTER_OPTION_ID
            .get()
            .is_some_and(|id| event.id == *id);
    if is_enter_hotkey
        && HOTKEY_ENTER_REGISTERED.load(Ordering::Relaxed)
        && matches!(event.state, HotKeyState::Pressed)
    {
        crate::app::send_event(AppEvent::FinalizeHotkeyPressed {
            raw_requested: is_option_enter_hotkey,
        });
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
        let result = if enabled {
            manager.register(escape_hotkey)
        } else {
            manager.unregister(escape_hotkey)
        };

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
                eprintln!(
                    "Azad: failed to register Option+NumpadEnter hotkey: {}",
                    err
                );
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
            eprintln!(
                "Azad: failed to unregister Option+NumpadEnter hotkey: {}",
                err
            );
        }
        HOTKEY_ENTER_REGISTERED.store(false, Ordering::Relaxed);
    });
}

unsafe fn send_command_v_robust() {
    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Ok(source) => source,
        Err(_) => return,
    };

    release_modifiers(&source);

    if let Ok(command_down) =
        CGEvent::new_keyboard_event(source.clone(), KEYCODE_LEFT_COMMAND, true)
    {
        command_down.set_flags(CGEventFlags::CGEventFlagCommand);
        command_down.post(CGEventTapLocation::HID);
    }

    if let Ok(key_down) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_V, true) {
        key_down.set_flags(CGEventFlags::CGEventFlagCommand);
        key_down.post(CGEventTapLocation::HID);
    }

    if let Ok(key_up) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_V, false) {
        key_up.set_flags(CGEventFlags::CGEventFlagCommand);
        key_up.post(CGEventTapLocation::HID);
    }
    // Hold the command chord briefly so targets consistently register the paste action.
    std::thread::sleep(Duration::from_millis(PASTE_CHORD_HOLD_MS));

    if let Ok(command_up) = CGEvent::new_keyboard_event(source, KEYCODE_LEFT_COMMAND, false) {
        command_up.post(CGEventTapLocation::HID);
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

unsafe fn write_pasteboard_string(text: &str) -> bool {
    let pasteboard = NSPasteboard::generalPasteboard(nil);
    let _: usize = msg_send![pasteboard, clearContents];
    let ns_text = NSString::alloc(nil).init_str(text);
    let ok: i8 = msg_send![pasteboard, setString: ns_text forType: NSPasteboardTypeString];
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
                .arg(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                )
                .spawn();
        }
    }
}

unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
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

unsafe fn assign_status_icon(status_item: id) {
    let button: id = msg_send![status_item, button];
    if button == nil {
        return;
    }

    // Use a template status item icon so AppKit automatically handles
    // light/dark mode and highlighted menu bar states.
    let template_icon = load_icon("azad-black.png");
    let fallback_icon = load_icon("azad-white.png");
    let icon = if template_icon != nil {
        template_icon
    } else {
        fallback_icon
    };

    if icon != nil {
        let _: () = msg_send![icon, setTemplate: YES];
        let _: () = msg_send![button, setImage: icon];
    } else {
        let _: () = msg_send![button, setTitle: NSString::alloc(nil).init_str("Azad")];
    }
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

    Some(PathBuf::from(
        CStr::from_ptr(ptr).to_string_lossy().into_owned(),
    ))
}
