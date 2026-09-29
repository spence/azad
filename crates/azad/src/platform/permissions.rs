use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use cocoa::base::{YES, id, nil};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};

static OPENED_ACCESSIBILITY_SETTINGS: AtomicBool = AtomicBool::new(false);

pub fn ensure_accessibility_for_auto_paste() -> bool {
  if is_accessibility_trusted() {
    return true;
  }
  eprintln!(
    "Azad: Accessibility permission missing. Enable Azad in System Settings -> Privacy & Security -> Accessibility."
  );
  false
}

pub fn request_accessibility_permission() {
  maybe_request_accessibility_permission_once();
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

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
  static AVMediaTypeAudio: id;
}
