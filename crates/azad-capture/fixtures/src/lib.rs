//! Fixtures that drive `azad-capture` inside a disposable macOS VM.
//!
//! Every fixture refuses to run unless the machine identifies as a `VirtualMac`: they create
//! virtual keyboards, seize devices, and toggle Secure Input, none of which belong on a real
//! desktop.

use std::time::{SystemTime, UNIX_EPOCH};

/// Exits with status 70 unless running inside a macOS virtual machine.
pub fn require_virtual_mac() {
  let mut model = [0u8; 128];
  let mut size = model.len();
  // SAFETY: `model` is writable for `size` bytes.
  let status = unsafe {
    libc::sysctlbyname(
      c"hw.model".as_ptr(),
      model.as_mut_ptr().cast(),
      &mut size,
      std::ptr::null_mut(),
      0,
    )
  };
  if status != 0 || !model.starts_with(b"VirtualMac") {
    eprintln!("REFUSED: azad-capture fixtures run only in a disposable VirtualMac guest");
    std::process::exit(70);
  }
}

pub fn wall_ms() -> u128 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Value of `--name` in `args`, if present.
pub fn arg(args: &[String], name: &str) -> Option<String> {
  args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
  pub fn EnableSecureEventInput() -> i32;
  pub fn DisableSecureEventInput() -> i32;
  pub fn IsSecureEventInputEnabled() -> u8;
}
