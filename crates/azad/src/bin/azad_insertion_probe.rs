//! Recipient-level insertion probe restricted to an isolated macOS virtual machine.
//!
//! This binary compiles the production insertion code without app state, audio, or preferences.
//! Native input requires both the VM hardware guard and an explicit command-line flag.

#![allow(dead_code)]
#![allow(deprecated)]
#![allow(unexpected_cfgs)]

#[path = "../settings.rs"]
mod settings;
#[path = "../typing.rs"]
mod typing;

use std::process::Command;
use std::time::Instant;

use anyhow::{Result, ensure};
use cocoa::appkit::NSPasteboard;
use cocoa::base::{id, nil};
use cocoa::foundation::NSAutoreleasePool;
use objc::{class, msg_send, sel, sel_impl};

use settings::PasteMethod;

const KEYCODE_RETURN: u16 = 0x24;
const AZAD_SYNTHETIC_MARKER: i64 = 0x1A2A_D1A2;
const KCG_EVENT_SOURCE_USER_DATA_FIELD: u32 = 42;

#[path = "../platform/paste.rs"]
#[allow(unsafe_op_in_unsafe_fn)]
mod paste;

fn main() -> Result<()> {
  let args: Vec<String> = std::env::args().collect();
  ensure!(
    args.len() == 3 && args[1] == "--allow-vm-input",
    "usage: azad-insertion-probe --allow-vm-input TEXT"
  );
  let model = Command::new("/usr/sbin/sysctl").args(["-n", "hw.model"]).output()?;
  ensure!(model.status.success(), "cannot establish the VM hardware boundary");
  ensure!(
    String::from_utf8(model.stdout)?.trim().starts_with("VirtualMac"),
    "refusing native input outside a macOS VM"
  );

  // SAFETY: this thread owns the pool and drains it after all Objective-C calls.
  let pool = unsafe { NSAutoreleasePool::new(nil) };
  // SAFETY: NSWorkspace supplies the current guest application; no activation occurs here.
  let frontmost = unsafe {
    let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
    let application: id = msg_send![workspace, frontmostApplication];
    let bundle: id = msg_send![application, bundleIdentifier];
    nsstring_to_string(bundle)
  };
  // SAFETY: this query does not request or change event-posting permission.
  let post_access = unsafe { CGPreflightPostEventAccess() };
  ensure!(
    frontmost.as_deref() == Some("com.brave.Browser"),
    "Brave is not the foreground guest application: {frontmost:?}"
  );
  ensure!(post_access, "the guest probe lacks event-posting permission");
  let started = Instant::now();
  let result = paste::insert_text(&args[2], PasteMethod::ClipboardPaste, 20);
  // SAFETY: the VM guard precedes access to this guest's pasteboard.
  let clipboard_matches = unsafe {
    let board = NSPasteboard::generalPasteboard(nil);
    let text_type: id =
      msg_send![class!(NSString), stringWithUTF8String: c"public.utf8-plain-text".as_ptr()];
    let value: id = msg_send![board, stringForType: text_type];
    nsstring_to_string(value).as_deref() == Some(args[2].as_str())
  };
  println!(
    "{}",
    serde_json::json!({"result": format!("{result:?}"), "clipboard_matches": clipboard_matches, "frontmost": frontmost, "post_access": post_access, "elapsed_ms": started.elapsed().as_millis()})
  );
  // SAFETY: no Objective-C object from the pool is used after this call.
  unsafe { pool.drain() };
  ensure!(result == paste::PasteResult::Pasted, "production insertion did not report success");
  ensure!(clipboard_matches, "guest pasteboard does not contain the requested text");
  Ok(())
}

fn ensure_accessibility_for_auto_paste() -> bool {
  // SAFETY: this is the read-only Accessibility trust query for this process.
  unsafe { AXIsProcessTrusted() }
}

fn overlay_debug_logs_enabled() -> bool {
  true
}

unsafe fn nsstring_to_string(value: id) -> Option<String> {
  if value == nil {
    return None;
  }
  // SAFETY: callers supply an NSString or nil; its UTF-8 storage outlives this copy.
  let bytes: *const std::ffi::c_char = unsafe { msg_send![value, UTF8String] };
  if bytes.is_null() {
    return None;
  }
  // SAFETY: NSString returns a valid null-terminated UTF-8 buffer.
  Some(unsafe { std::ffi::CStr::from_ptr(bytes) }.to_string_lossy().into_owned())
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
  fn AXIsProcessTrusted() -> bool;
  fn CGPreflightPostEventAccess() -> bool;
}
