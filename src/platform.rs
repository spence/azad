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
const PASTE_FOCUS_DELAY_MS: u64 = 60;
const PASTE_CLIPBOARD_DELAY_MS: u64 = 60;
const OVERLAY_WIDTH_MIN: f64 = 420.0;
const OVERLAY_WIDTH_MAX: f64 = 900.0;
const OVERLAY_HEIGHT_MIN: f64 = 160.0;
const OVERLAY_HEIGHT_MAX: f64 = 280.0;

static DELEGATE_CLASS: OnceLock<&'static Class> = OnceLock::new();
static OVERLAY_WINDOW_CLASS: OnceLock<&'static Class> = OnceLock::new();
static HOTKEY_OPTION_SPACE_ID: OnceLock<u32> = OnceLock::new();
static HOTKEY_ESCAPE_ID: OnceLock<u32> = OnceLock::new();
static OPENED_ACCESSIBILITY_SETTINGS: AtomicBool = AtomicBool::new(false);

thread_local! {
    static OVERLAY_REFS: RefCell<Option<OverlayRefs>> = const { RefCell::new(None) };
    static STATUS_MENU_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static STATUS_DELEGATE_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_LABEL_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_HEADER_CHEVRON_REF: RefCell<Option<id>> = const { RefCell::new(None) };
    static DEVICE_ROW_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static DEVICE_MENU_MODEL: RefCell<DeviceMenuModel> = RefCell::new(DeviceMenuModel::default());
}

#[derive(Clone, Copy)]
struct OverlayRefs {
    window: id,
    label: id,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceMenuModel {
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

pub fn show_overlay() {
    unsafe {
        let refs = ensure_overlay();
        let _: () = msg_send![refs.window, orderFrontRegardless];
    }
}

pub fn hide_overlay() {
    if let Some(refs) = current_overlay() {
        unsafe {
            let _: () = msg_send![refs.window, orderOut: nil];
        }
    }
}

pub fn set_overlay_content(status: &str, draft: &str, spinner: Option<char>) {
    let Some(refs) = current_overlay() else {
        return;
    };

    let prefix = match spinner {
        Some(ch) => format!("{ch} {status}"),
        None => status.to_string(),
    };
    let body = draft.trim();
    let rendered = if body.is_empty() {
        prefix
    } else {
        format!("{prefix}\n{body}")
    };

    unsafe {
        let _: () = msg_send![refs.label, setStringValue: NSString::alloc(nil).init_str(&rendered)];
    }
}

pub fn paste_text(text: &str) -> PasteResult {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return PasteResult::EmptyText;
    }

    unsafe {
        if !write_pasteboard_string(trimmed) {
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

    // Give the previously focused app a moment to regain key status after overlay hide.
    std::thread::sleep(Duration::from_millis(PASTE_FOCUS_DELAY_MS));
    std::thread::sleep(Duration::from_millis(PASTE_CLIPBOARD_DELAY_MS));

    unsafe {
        send_command_v_robust();
    }

    PasteResult::Pasted
}

fn register_delegate_class() -> &'static Class {
    DELEGATE_CLASS.get_or_init(|| unsafe {
        let superclass = class!(NSObject);
        let mut decl =
            ClassDecl::new("AzadStatusDelegate", superclass).expect("failed to declare delegate");

        decl.add_ivar::<id>("statusItem");
        decl.add_method(sel!(listen:), listen as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(quit:), quit as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(tick:), tick as extern "C" fn(&Object, Sel, id));
        decl.add_method(
            sel!(toggleDevices:),
            toggle_devices as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(selectDevice:),
            select_device as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(menuWillOpen:),
            menu_will_open as extern "C" fn(&Object, Sel, id),
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

extern "C" fn listen(_: &Object, _: Sel, _: id) {
    crate::app::send_event(AppEvent::MenuListen);
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

extern "C" fn toggle_devices(_: &Object, _: Sel, _: id) {
    crate::app::send_event(AppEvent::MenuToggleDevices);
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
    let _: id = msg_send![
        class!(NSTimer),
        scheduledTimerWithTimeInterval: 0.05f64
        target: delegate
        selector: sel!(tick:)
        userInfo: nil
        repeats: YES
    ];

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

    if menu_is_open(menu) {
        update_menu_inline(menu, delegate, &model);
    } else {
        build_menu_fresh(menu, delegate, &model);
    }
}

fn menu_is_open(menu: id) -> bool {
    unsafe {
        let attached: i8 = msg_send![menu, isAttached];
        attached != 0
    }
}

fn build_menu_fresh(menu: id, delegate: id, model: &DeviceMenuModel) {
    unsafe {
        let _: () = msg_send![menu, removeAllItems];

        let listen_item = NSMenuItem::alloc(nil)
            .initWithTitle_action_keyEquivalent_(
                NSString::alloc(nil).init_str("Listen"),
                sel!(listen:),
                NSString::alloc(nil).init_str(""),
            )
            .autorelease();
        listen_item.setTarget_(delegate);
        menu.addItem_(listen_item);

        let header_item = make_device_header_item(delegate, model);
        menu.addItem_(header_item);

        insert_device_rows(menu, delegate, model, 2);

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
    set_device_header_title(model);
    clear_device_rows(menu);

    unsafe {
        let count: i64 = msg_send![menu, numberOfItems];
        // Items are [Listen, Header, ..., Quit]
        let insert_at = if count >= 3 { count - 1 } else { 2 };
        insert_device_rows(menu, delegate, model, insert_at);
    }
}

fn device_header_label(model: &DeviceMenuModel) -> String {
    if model.header_label.trim().is_empty() {
        "No Input Device".to_string()
    } else {
        model.header_label.clone()
    }
}

fn device_header_chevron(model: &DeviceMenuModel) -> &'static str {
    if model.expanded { "▾" } else { "▸" }
}

unsafe fn make_device_header_item(delegate: id, model: &DeviceMenuModel) -> id {
    let item = NSMenuItem::alloc(nil)
        .initWithTitle_action_keyEquivalent_(
            NSString::alloc(nil).init_str(""),
            sel!(noop:),
            NSString::alloc(nil).init_str(""),
        )
        .autorelease();

    let view_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 24.0));
    let view: id = msg_send![class!(NSView), alloc];
    let view: id = msg_send![view, initWithFrame: view_frame];

    let button_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 24.0));
    let button: id = msg_send![class!(NSButton), alloc];
    let button: id = msg_send![button, initWithFrame: button_frame];
    let _: () = msg_send![button, setBordered: NO];
    let _: () = msg_send![button, setTarget: delegate];
    let _: () = msg_send![button, setAction: sel!(toggleDevices:)];
    let _: () = msg_send![button, setTitle: NSString::alloc(nil).init_str("")];

    let font: id = msg_send![class!(NSFont), systemFontOfSize: 13.0f64];
    let text_color: id = msg_send![class!(NSColor), labelColor];

    let title_label_frame = NSRect::new(NSPoint::new(10.0, 2.0), NSSize::new(236.0, 20.0));
    let title_label: id = msg_send![class!(NSTextField), alloc];
    let title_label: id = msg_send![title_label, initWithFrame: title_label_frame];
    let _: () = msg_send![title_label, setStringValue: NSString::alloc(nil).init_str(&device_header_label(model))];
    let _: () = msg_send![title_label, setBezeled: NO];
    let _: () = msg_send![title_label, setDrawsBackground: NO];
    let _: () = msg_send![title_label, setEditable: NO];
    let _: () = msg_send![title_label, setSelectable: NO];
    let _: () = msg_send![title_label, setAlignment: 0isize];
    let _: () = msg_send![title_label, setFont: font];
    let _: () = msg_send![title_label, setTextColor: text_color];

    let chevron_label_frame = NSRect::new(NSPoint::new(250.0, 2.0), NSSize::new(20.0, 20.0));
    let chevron_label: id = msg_send![class!(NSTextField), alloc];
    let chevron_label: id = msg_send![chevron_label, initWithFrame: chevron_label_frame];
    let _: () = msg_send![chevron_label, setStringValue: NSString::alloc(nil).init_str(device_header_chevron(model))];
    let _: () = msg_send![chevron_label, setBezeled: NO];
    let _: () = msg_send![chevron_label, setDrawsBackground: NO];
    let _: () = msg_send![chevron_label, setEditable: NO];
    let _: () = msg_send![chevron_label, setSelectable: NO];
    let _: () = msg_send![chevron_label, setAlignment: 2isize];
    let _: () = msg_send![chevron_label, setFont: font];
    let _: () = msg_send![chevron_label, setTextColor: text_color];

    let _: () = msg_send![view, addSubview: button];
    let _: () = msg_send![view, addSubview: title_label];
    let _: () = msg_send![view, addSubview: chevron_label];
    let _: () = msg_send![item, setView: view];

    DEVICE_HEADER_LABEL_REF.with(|slot| {
        slot.borrow_mut().replace(title_label);
    });
    DEVICE_HEADER_CHEVRON_REF.with(|slot| {
        slot.borrow_mut().replace(chevron_label);
    });

    item
}

fn set_device_header_title(model: &DeviceMenuModel) {
    DEVICE_HEADER_LABEL_REF.with(|slot| {
        if let Some(label) = *slot.borrow() {
            unsafe {
                let text = NSString::alloc(nil).init_str(&device_header_label(model));
                let _: () = msg_send![label, setStringValue: text];
            }
        }
    });
    DEVICE_HEADER_CHEVRON_REF.with(|slot| {
        if let Some(label) = *slot.borrow() {
            unsafe {
                let chevron = NSString::alloc(nil).init_str(device_header_chevron(model));
                let _: () = msg_send![label, setStringValue: chevron];
            }
        }
    });
}

fn clear_device_rows(menu: id) {
    unsafe {
        let count: i64 = msg_send![menu, numberOfItems];
        // Keep [Listen, Header, Quit], remove rows in between.
        if count >= 4 {
            for idx in (2..=(count - 2)).rev() {
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

    let refs = create_overlay_window();
    OVERLAY_REFS.with(|store| {
        store.borrow_mut().replace(refs);
    });
    refs
}

fn current_overlay() -> Option<OverlayRefs> {
    OVERLAY_REFS.with(|store| *store.borrow())
}

unsafe fn create_overlay_window() -> OverlayRefs {
    let screen = NSScreen::mainScreen(nil);
    let frame = if screen == nil {
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0))
    } else {
        NSScreen::frame(screen)
    };

    let overlay_width = (frame.size.width * 0.58).clamp(OVERLAY_WIDTH_MIN, OVERLAY_WIDTH_MAX);
    let overlay_height = (frame.size.height * 0.22).clamp(OVERLAY_HEIGHT_MIN, OVERLAY_HEIGHT_MAX);
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
    window.setIgnoresMouseEvents_(NO);
    window.setMovableByWindowBackground_(YES);
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
    let _: () = msg_send![card_layer, setCornerRadius: 22.0f64];
    let _: () = msg_send![card_layer, setMasksToBounds: YES];
    window.setContentView_(card_view);

    let label_frame = NSRect::new(
        NSPoint::new(20.0, 20.0),
        NSSize::new(overlay_width - 40.0, overlay_height - 40.0),
    );

    let label: id = msg_send![class!(NSTextField), alloc];
    let label: id = msg_send![label, initWithFrame: label_frame];
    let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str("Listening...")];
    let _: () = msg_send![label, setBezeled: NO];
    let _: () = msg_send![label, setDrawsBackground: NO];
    let _: () = msg_send![label, setEditable: NO];
    let _: () = msg_send![label, setSelectable: NO];
    let _: () = msg_send![label, setAlignment: 0isize];
    let _: () = msg_send![label, setLineBreakMode: 0isize];
    let _: () = msg_send![label, setUsesSingleLineMode: NO];
    let _: () = msg_send![label, setMaximumNumberOfLines: 0isize];
    let font: id = msg_send![class!(NSFont), systemFontOfSize: 24.0f64];
    let _: () = msg_send![label, setFont: font];
    let text_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
    let _: () = msg_send![label, setTextColor: text_color];
    let _: () = msg_send![card_view, addSubview: label];

    OverlayRefs { window, label }
}

fn install_global_hotkeys() {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(err) => {
            eprintln!("Azad: failed to initialize global hotkey manager: {}", err);
            return;
        }
    };

    let hotkey = HotKey::new(Some(Modifiers::ALT), Code::Space);
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
    let escape_hotkey_id = escape_hotkey.id();
    if let Err(err) = manager.register(escape_hotkey) {
        eprintln!(
            "Azad: failed to register Escape hotkey for overlay cancel: {}",
            err
        );
    } else {
        let _ = HOTKEY_ESCAPE_ID.set(escape_hotkey_id);
    }

    GlobalHotKeyEvent::set_event_handler(Some(|event| {
        handle_global_hotkey_event(event);
    }));

    // Keep manager alive for the lifetime of the app.
    let _ = Box::leak(Box::new(manager));
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
        if event.id == escape_id && matches!(event.state, HotKeyState::Pressed) {
            crate::app::send_event(AppEvent::OverlayCancel);
        }
    }
}

unsafe fn send_command_v_robust() {
    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Ok(source) => source,
        Err(_) => return,
    };

    release_modifiers(&source);

    if let Ok(command_down) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_LEFT_COMMAND, true) {
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
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
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
