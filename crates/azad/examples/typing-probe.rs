#![allow(deprecated, unexpected_cfgs, unsafe_op_in_unsafe_fn)]

#[allow(dead_code)]
#[path = "../src/platform/paste.rs"]
mod paste;
#[allow(dead_code)]
#[path = "../src/preferred_store.rs"]
mod preferred_store;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;
#[path = "../src/typing.rs"]
mod typing;

use std::cell::RefCell;
use std::ffi::CStr;
use std::process::Command;
use std::time::{Duration, Instant};

use cocoa::appkit::{
  NSApp, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSPasteboard, NSWindow,
  NSWindowStyleMask,
};
use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use objc::declare::ClassDecl;
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use paste::{InsertStart, PacedTyping, PasteResult};
use settings::{AutoSubmitMode, PasteMethod};
use typing::TypingProgress;

const KEYCODE_RETURN: u16 = 0x24;
const AZAD_SYNTHETIC_MARKER: i64 = 0x1A2A_D1A2;
const KCG_EVENT_SOURCE_USER_DATA_FIELD: u32 = 42;

thread_local! {
  static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

struct Probe {
  text_view: id,
  search_field: id,
  panel: id,
  text: String,
  rate: u32,
  copy: bool,
  method: PasteMethod,
  active: Option<PacedTyping>,
  started: Option<Instant>,
  finished: Option<Instant>,
  received: Vec<Instant>,
  heartbeat: usize,
}

fn main() {
  let model = Command::new("/usr/sbin/sysctl").args(["-n", "hw.model"]).output().unwrap();
  assert!(
    model.status.success() && String::from_utf8_lossy(&model.stdout).starts_with("VirtualMac"),
    "typing-probe refuses to post input outside a macOS VM"
  );
  let rate: u32 = std::env::args().nth(1).unwrap_or_else(|| "80".into()).parse().unwrap();
  assert!(settings::TYPING_KEY_RATES.contains(&rate));
  let method = match std::env::args().nth(2).as_deref() {
    Some("type") => PasteMethod::DirectTyping,
    Some("paste") => PasteMethod::ClipboardPaste,
    _ => PasteMethod::DirectTypingAndCopyClipboard,
  };
  let copy = method != PasteMethod::DirectTyping;
  let fixture: serde_json::Value =
    serde_json::from_str(include_str!("../docs/typing-key-rate.fixture.json")).unwrap();
  assert!(ensure_accessibility_for_auto_paste(), "grant only this VM probe Accessibility first");
  preferred_store::save_typing_key_rate(80);
  assert_eq!(preferred_store::load_typing_key_rate(), 80);
  preferred_store::save_typing_key_rate(0);
  assert_eq!(preferred_store::load_typing_key_rate(), 0);

  // SAFETY: this guarded executable owns its AppKit receiver and VM desktop.
  unsafe {
    let _pool = NSAutoreleasePool::new(nil);
    let app = NSApp();
    app.setActivationPolicy_(NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular);
    let menu: id = msg_send![class!(NSMenu), new];
    let edit: id = msg_send![class!(NSMenu), new];
    let edit_item: id = msg_send![class!(NSMenuItem), new];
    let _: () = msg_send![menu, addItem: edit_item];
    let _: () = msg_send![edit_item, setSubmenu: edit];
    let paste_title = NSString::alloc(nil).init_str("Paste");
    let paste_key = NSString::alloc(nil).init_str("v");
    let paste_item: id = msg_send![class!(NSMenuItem), alloc];
    let paste_item: id = msg_send![paste_item, initWithTitle: paste_title action: sel!(paste:) keyEquivalent: paste_key];
    let _: () = msg_send![edit, addItem: paste_item];
    let _: () = msg_send![app, setMainMenu: menu];
    let mut receiver = ClassDecl::new("AzadTypingProofReceiver", class!(NSTextView)).unwrap();
    receiver.add_method(sel!(keyDown:), receive_key as extern "C" fn(&Object, Sel, id));
    let receiver = receiver.register();
    let frame = NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(620.0, 240.0));
    let view: id = msg_send![receiver, alloc];
    let view: id = msg_send![view, initWithFrame: frame];
    let _: () = msg_send![view, setAutomaticQuoteSubstitutionEnabled: NO];
    let _: () = msg_send![view, setAutomaticSpellingCorrectionEnabled: NO];
    let window = NSWindow::alloc(nil).initWithContentRect_styleMask_backing_defer_(
      frame,
      NSWindowStyleMask::NSTitledWindowMask,
      NSBackingStoreType::NSBackingStoreBuffered,
      NO,
    );
    window.setContentView_(view);
    window.makeKeyAndOrderFront_(nil);
    let _: () = msg_send![window, makeFirstResponder: view];
    app.activateIgnoringOtherApps_(YES);
    let mut panel_class = ClassDecl::new("AzadTypingProofPanel", class!(NSPanel)).unwrap();
    panel_class
      .add_method(sel!(canBecomeKeyWindow), never_key as extern "C" fn(&Object, Sel) -> bool);
    let panel_class = panel_class.register();
    let panel: id = msg_send![panel_class, alloc];
    let panel_frame = NSRect::new(NSPoint::new(100.0, 400.0), NSSize::new(400.0, 80.0));
    let panel: id = msg_send![panel, initWithContentRect: panel_frame styleMask: (1_u64 << 7)
      backing: NSBackingStoreType::NSBackingStoreBuffered defer: NO];
    let search: id = msg_send![class!(NSTextField), alloc];
    let search: id = msg_send![search, initWithFrame: frame];
    let _: () = msg_send![search, setEditable: NO];
    let _: () = msg_send![search, setSelectable: NO];
    panel.setContentView_(search);
    let _: () = msg_send![panel, orderFront: nil];
    let _: () = msg_send![panel, makeKeyWindow];
    let is_key: bool = msg_send![panel, isKeyWindow];
    assert!(!is_key);
    let board = NSPasteboard::generalPasteboard(nil);
    let _: usize = msg_send![board, clearContents];
    let sentinel = NSString::alloc(nil).init_str("before typing");
    let kind = NSString::alloc(nil).init_str("public.utf8-plain-text");
    let _: bool = msg_send![board, setString: sentinel forType: kind];
    PROBE.with(|slot| {
      slot.replace(Some(Probe {
        text_view: view,
        search_field: search,
        panel,
        text: fixture["unicodeSample"].as_str().unwrap().into(),
        rate,
        copy,
        method,
        active: None,
        started: None,
        finished: None,
        received: Vec::new(),
        heartbeat: 0,
      }))
    });
    let mut delegate = ClassDecl::new("AzadTypingProofTimer", class!(NSObject)).unwrap();
    delegate.add_method(sel!(step:), step as extern "C" fn(&Object, Sel, id));
    delegate.add_method(sel!(heartbeat:), heartbeat as extern "C" fn(&Object, Sel, id));
    let delegate = delegate.register();
    let target: id = msg_send![delegate, new];
    let _: id = msg_send![class!(NSTimer), scheduledTimerWithTimeInterval: 0.2_f64
      target: target selector: sel!(step:) userInfo: nil repeats: NO];
    let _: id = msg_send![class!(NSTimer), scheduledTimerWithTimeInterval: 0.01_f64
      target: target selector: sel!(heartbeat:) userInfo: nil repeats: YES];
    app.run();
  }
  panic!("receiver run loop returned before verification");
}

extern "C" fn step(this: &Object, _: Sel, _: id) {
  let after = PROBE.with(|slot| {
    let mut slot = slot.borrow_mut();
    let probe = slot.as_mut().unwrap();
    let now = Instant::now();
    if let Some(finished) = probe.finished {
      if finished.elapsed() < Duration::from_millis(250) {
        return Some(Duration::from_millis(20));
      }
      // SAFETY: receiver and pasteboard are owned VM-only AppKit objects.
      unsafe {
        let value: id = msg_send![probe.text_view, string];
        let actual = nsstring_to_string(value).unwrap();
        let expected = format!("{}\n", probe.text);
        let clipboard = read_clipboard();
        let search_value: id = msg_send![probe.search_field, stringValue];
        let search = nsstring_to_string(search_value).unwrap();
        let panel_key: bool = msg_send![probe.panel, isKeyWindow];
        let elapsed = probe.received.last().unwrap().duration_since(probe.received[0]);
        let minimum = if probe.rate == 0 || probe.method == PasteMethod::ClipboardPaste { 0.0 } else { (probe.text.chars().count() - 1) as f64 / f64::from(probe.rate) };
        let passed = actual == expected && elapsed.as_secs_f64() >= minimum * 0.9
          && clipboard == if probe.copy { &probe.text } else { "before typing" }
          && (probe.rate == 0 || probe.heartbeat > 20) && !panel_key && search.is_empty();
        println!("{}", serde_json::json!({"passed":passed,"rate":probe.rate,"copy":probe.copy,"method":probe.method.prefs_value(),
          "expected":expected,"received":actual,"clipboard":clipboard,"receive_ms":elapsed.as_millis(),
          "minimum_ms":minimum * 1000.0,"heartbeat_ticks":probe.heartbeat,"keydowns":probe.received.len(),
          "history_panel_key":panel_key,"history_search":search}));
        std::process::exit(if passed { 0 } else { 1 });
      }
    }
    if probe.started.is_none() {
      probe.started = Some(now);
      match paste::start_insert(&probe.text, probe.method, 30, probe.rate) {
        InsertStart::Paced(active) => probe.active = Some(active),
        InsertStart::Finished(PasteResult::Pasted) => {},
        _ => panic!("native insertion did not start"),
      }
    }
    if let Some(active) = probe.active.as_mut() {
      match active.advance(Instant::now()) {
        TypingProgress::Waiting(after) => {
          // SAFETY: this probe has already verified its owned VM boundary.
          assert_eq!(unsafe { read_clipboard() }, "before typing");
          return Some(after);
        }
        TypingProgress::Complete => { probe.active = None; }
        TypingProgress::Failed => panic!("native event recipe failed"),
      }
    }
    assert!(paste::send_auto_submit(AutoSubmitMode::Enter));
    probe.finished = Some(Instant::now());
    Some(Duration::from_millis(20))
  });
  if let Some(after) = after {
    // SAFETY: this delegate and one-shot timer are confined to the VM main run loop.
    unsafe {
      let _: id = msg_send![class!(NSTimer), scheduledTimerWithTimeInterval: after.as_secs_f64().max(0.001)
        target: this selector: sel!(step:) userInfo: nil repeats: NO];
    }
  }
}

extern "C" fn heartbeat(_: &Object, _: Sel, _: id) {
  PROBE.with(|slot| {
    if let Some(probe) = slot.borrow_mut().as_mut() {
      probe.heartbeat += 1;
      if probe.started.is_some_and(|started| started.elapsed() > Duration::from_secs(10)) {
        eprintln!("native receiver timed out");
        std::process::exit(1);
      }
    }
  });
}

extern "C" fn never_key(_: &Object, _: Sel) -> bool {
  false
}

extern "C" fn receive_key(this: &Object, _: Sel, event: id) {
  unsafe extern "C" {
    fn CGEventGetIntegerValueField(event: *mut std::ffi::c_void, field: u32) -> i64;
  }
  // SAFETY: this is the receiver's actual event, not a posted or fabricated event.
  unsafe {
    let cg_event: *mut std::ffi::c_void = msg_send![event, CGEvent];
    assert_eq!(
      CGEventGetIntegerValueField(cg_event, KCG_EVENT_SOURCE_USER_DATA_FIELD),
      AZAD_SYNTHETIC_MARKER
    );
  }
  PROBE.with(|slot| slot.borrow_mut().as_mut().unwrap().received.push(Instant::now()));
  // SAFETY: the superclass processes the actual window-server event in the owned receiver.
  unsafe {
    let _: () = msg_send![super(this, class!(NSTextView)), keyDown: event];
  }
}

fn ensure_accessibility_for_auto_paste() -> bool {
  #[link(name = "ApplicationServices", kind = "framework")]
  unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
  }
  // SAFETY: this read-only query cannot request or alter host permissions.
  unsafe { AXIsProcessTrusted() }
}

unsafe fn nsstring_to_string(value: id) -> Option<String> {
  if value == nil {
    return None;
  }
  let pointer: *const std::os::raw::c_char = unsafe { msg_send![value, UTF8String] };
  if pointer.is_null() {
    None
  } else {
    Some(unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into())
  }
}

unsafe fn read_clipboard() -> String {
  let kind = unsafe { NSString::alloc(nil).init_str("public.utf8-plain-text") };
  let board = unsafe { NSPasteboard::generalPasteboard(nil) };
  let value: id = unsafe { msg_send![board, stringForType: kind] };
  unsafe { nsstring_to_string(value) }.unwrap_or_default()
}
