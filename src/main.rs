#![allow(deprecated)]
#![allow(unexpected_cfgs)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

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

const OVERLAY_TEXT: &str = "Azad is listening... (placeholder text)";
const KEYCODE_V: u16 = 0x09;

static APP_DELEGATE_PTR: AtomicPtr<Object> = AtomicPtr::new(ptr::null_mut());
static HOTKEY_HELD: AtomicBool = AtomicBool::new(false);
static OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);
static OVERLAY_CANCELLED: AtomicBool = AtomicBool::new(false);
static HOTKEY_ID: OnceLock<u32> = OnceLock::new();

fn main() {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let app = NSApp();
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );

        let delegate_class = register_delegate_class();
        let delegate: id = msg_send![delegate_class, new];
        APP_DELEGATE_PTR.store(delegate as *mut Object, Ordering::SeqCst);

        setup_status_bar(delegate);
        install_global_option_space_hotkey();
        let _: () = msg_send![app, setDelegate: delegate];
        app.run();
    }
}

fn register_delegate_class() -> &'static Class {
    static DELEGATE_CLASS: OnceLock<&'static Class> = OnceLock::new();
    DELEGATE_CLASS.get_or_init(|| unsafe {
        let superclass = class!(NSObject);
        let mut decl =
            ClassDecl::new("AzadStatusDelegate", superclass).expect("failed to declare delegate");

        decl.add_ivar::<id>("statusItem");
        decl.add_ivar::<id>("overlayWindow");
        decl.add_method(sel!(listen:), listen as extern "C" fn(&mut Object, Sel, id));
        decl.add_method(sel!(quit:), quit as extern "C" fn(&Object, Sel, id));

        decl.register()
    })
}

fn register_overlay_window_class() -> &'static Class {
    static OVERLAY_WINDOW_CLASS: OnceLock<&'static Class> = OnceLock::new();
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

extern "C" fn overlay_can_become_key_window(_: &Object, _: Sel) -> bool {
    true
}

extern "C" fn overlay_can_become_main_window(_: &Object, _: Sel) -> bool {
    true
}

extern "C" fn overlay_key_down(this: &Object, _: Sel, event: id) {
    unsafe {
        let key_code: u16 = msg_send![event, keyCode];
        if key_code == 53 {
            OVERLAY_CANCELLED.store(true, Ordering::SeqCst);
            HOTKEY_HELD.store(false, Ordering::SeqCst);
            OVERLAY_VISIBLE.store(false, Ordering::SeqCst);
            let window_id = this as *const Object as id;
            let _: () = msg_send![window_id, orderOut: nil];
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

    let listen_item = NSMenuItem::alloc(nil)
        .initWithTitle_action_keyEquivalent_(
            NSString::alloc(nil).init_str("Listen"),
            sel!(listen:),
            NSString::alloc(nil).init_str(""),
        )
        .autorelease();
    listen_item.setTarget_(delegate);
    menu.addItem_(listen_item);

    let quit_item = NSMenuItem::alloc(nil)
        .initWithTitle_action_keyEquivalent_(
            NSString::alloc(nil).init_str("Quit"),
            sel!(quit:),
            NSString::alloc(nil).init_str("q"),
        )
        .autorelease();
    quit_item.setTarget_(delegate);
    menu.addItem_(quit_item);

    status_item.setMenu_(menu);
    assign_status_icon(status_item);

    let delegate_obj = &mut *(delegate as *mut Object);
    delegate_obj.set_ivar("statusItem", status_item);
    delegate_obj.set_ivar("overlayWindow", nil);
}

extern "C" fn listen(this: &mut Object, _: Sel, _: id) {
    unsafe {
        show_overlay(this);
    }
}

extern "C" fn quit(_: &Object, _: Sel, _: id) {
    unsafe {
        let app = NSApp();
        let _: () = msg_send![app, terminate: nil];
    }
}

unsafe fn create_overlay_window() -> id {
    let screen = NSScreen::mainScreen(nil);
    let frame = if screen == nil {
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0))
    } else {
        NSScreen::frame(screen)
    };

    let overlay_width = (frame.size.width * 0.58).clamp(420.0, 900.0);
    let overlay_height = (frame.size.height * 0.22).clamp(160.0, 280.0);
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
        NSPoint::new(24.0, (overlay_height * 0.5) - 24.0),
        NSSize::new(overlay_width - 48.0, 48.0),
    );

    let label: id = msg_send![class!(NSTextField), alloc];
    let label: id = msg_send![label, initWithFrame: label_frame];
    let _: () = msg_send![label, setStringValue: NSString::alloc(nil).init_str(OVERLAY_TEXT)];
    let _: () = msg_send![label, setBezeled: NO];
    let _: () = msg_send![label, setDrawsBackground: NO];
    let _: () = msg_send![label, setEditable: NO];
    let _: () = msg_send![label, setSelectable: NO];
    let _: () = msg_send![label, setAlignment: 1isize];
    let font: id = msg_send![class!(NSFont), systemFontOfSize: 30.0f64];
    let _: () = msg_send![label, setFont: font];
    let text_color = NSColor::colorWithCalibratedRed_green_blue_alpha_(nil, 1.0, 1.0, 1.0, 0.95);
    let _: () = msg_send![label, setTextColor: text_color];
    let _: () = msg_send![card_view, addSubview: label];

    window
}

unsafe fn show_overlay(delegate: &mut Object) {
    let mut overlay_window: id = *delegate.get_ivar("overlayWindow");
    if overlay_window == nil {
        overlay_window = create_overlay_window();
        delegate.set_ivar("overlayWindow", overlay_window);
    }

    let _: () = msg_send![overlay_window, orderFrontRegardless];
    OVERLAY_VISIBLE.store(true, Ordering::SeqCst);
}

unsafe fn hide_overlay(delegate: &Object) {
    let overlay_window: id = *delegate.get_ivar("overlayWindow");
    if overlay_window != nil {
        let _: () = msg_send![overlay_window, orderOut: nil];
    }
    OVERLAY_VISIBLE.store(false, Ordering::SeqCst);
}

fn install_global_option_space_hotkey() {
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

    let _ = HOTKEY_ID.set(hotkey_id);
    GlobalHotKeyEvent::set_event_handler(Some(|event| {
        handle_global_hotkey_event(event);
    }));

    // Keep manager alive for the lifetime of the app.
    let _ = Box::leak(Box::new(manager));
}

fn handle_global_hotkey_event(event: GlobalHotKeyEvent) {
    let Some(target_id) = HOTKEY_ID.get().copied() else {
        return;
    };
    if event.id != target_id {
        return;
    }

    match event.state {
        HotKeyState::Pressed => {
            if HOTKEY_HELD.swap(true, Ordering::SeqCst) {
                return;
            }

            OVERLAY_CANCELLED.store(false, Ordering::SeqCst);
            with_delegate_mut(|delegate| unsafe {
                show_overlay(delegate);
            });
        }
        HotKeyState::Released => {
            if !HOTKEY_HELD.swap(false, Ordering::SeqCst) {
                return;
            }

            with_delegate_ref(|delegate| unsafe {
                hide_overlay(delegate);
            });

            let cancelled = OVERLAY_CANCELLED.swap(false, Ordering::SeqCst);
            if !cancelled {
                unsafe {
                    paste_placeholder_to_active_app();
                }
            }
        }
    }
}

fn with_delegate_mut<F: FnOnce(&mut Object)>(f: F) {
    let ptr = APP_DELEGATE_PTR.load(Ordering::SeqCst);
    if ptr.is_null() {
        return;
    }
    unsafe {
        f(&mut *ptr);
    }
}

fn with_delegate_ref<F: FnOnce(&Object)>(f: F) {
    let ptr = APP_DELEGATE_PTR.load(Ordering::SeqCst);
    if ptr.is_null() {
        return;
    }
    unsafe {
        f(&*ptr);
    }
}

unsafe fn paste_placeholder_to_active_app() {
    let pasteboard = NSPasteboard::generalPasteboard(nil);
    let _ = pasteboard.clearContents();
    let text = NSString::alloc(nil).init_str(OVERLAY_TEXT);
    let _ = pasteboard.setString_forType(text, NSPasteboardTypeString);
    send_command_v();
}

unsafe fn send_command_v() {
    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Ok(source) => source,
        Err(_) => return,
    };

    if let Ok(key_down) = CGEvent::new_keyboard_event(source.clone(), KEYCODE_V, true) {
        key_down.set_flags(CGEventFlags::CGEventFlagCommand);
        key_down.post(CGEventTapLocation::HID);
    }

    if let Ok(key_up) = CGEvent::new_keyboard_event(source, KEYCODE_V, false) {
        key_up.set_flags(CGEventFlags::CGEventFlagCommand);
        key_up.post(CGEventTapLocation::HID);
    }
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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);

    if !path.exists() {
        return nil;
    }

    let ns_path = NSString::alloc(nil).init_str(&path.to_string_lossy());
    let image = NSImage::alloc(nil).initByReferencingFile_(ns_path);
    if image == nil {
        return nil;
    }

    let _: () = msg_send![image, setSize: NSSize::new(18.0, 18.0)];
    image
}
