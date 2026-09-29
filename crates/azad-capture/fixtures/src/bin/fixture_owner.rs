//! A competing exclusive owner of the fixture source keyboard.
//!
//! `--mode remap` models Karabiner-Elements/Kanata: it seizes the source and re-emits every key
//! through its own Karabiner-driver virtual keyboard presented as an Apple ANSI keyboard.
//! `--mode discard` seizes the source and drops its input, an owner Azad cannot cooperate with.
//!
//! Usage: fixture-owner --mode remap|discard --seconds N [--vendor N --product N]

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;

use serde_json::json;

use azad_capture::engine::{ForwardState, PAGE_KEYBOARD, Usage};
use azad_capture::sys::*;
use azad_capture::vhid::{self, Client};
use azad_capture_fixtures::*;

const APPLE_VENDOR_ID: u32 = 0x05ac;
const APPLE_ANSI_PRODUCT_ID: u32 = 0x024f;

struct Owner {
  queue: IOHIDQueueRef,
  held: BTreeSet<u16>,
  output: Option<Arc<Client>>,
  last: ForwardState,
  reports: u64,
}

fn main() {
  require_virtual_mac();
  let args: Vec<String> = std::env::args().collect();
  let mode = arg(&args, "--mode").expect("--mode remap|discard");
  let seconds: f64 = arg(&args, "--seconds").map_or(30.0, |v| v.parse().expect("seconds"));
  let vendor: i64 = arg(&args, "--vendor").map_or(0xfeed, |v| v.parse().expect("vendor"));
  let product: i64 = arg(&args, "--product").map_or(0x1790, |v| v.parse().expect("product"));

  let output = (mode == "remap").then(|| {
    let _ = std::process::Command::new("/usr/bin/defaults")
      .args([
        "write",
        "/Library/Preferences/com.apple.keyboardtype",
        "keyboardtype",
        "-dict-add",
        &format!("{APPLE_ANSI_PRODUCT_ID}-{APPLE_VENDOR_ID}-0"),
        "-int",
        "40",
      ])
      .status();
    let (tx, rx) = std::sync::mpsc::channel();
    let client = Client::connect_keyboard(
      vhid::SERVER_SOCKET_PATH,
      APPLE_VENDOR_ID,
      APPLE_ANSI_PRODUCT_ID,
      0,
      Box::new(move |state| {
        let _ = tx.send(state.can_forward());
      }),
    )
    .expect("remapper output keyboard");
    while !rx.recv_timeout(std::time::Duration::from_secs(10)).expect("output ready") {}
    client
  });

  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
  let device = loop {
    if let Some(device) = find_device(vendor, product) {
      break device;
    }
    if std::time::Instant::now() > deadline {
      println!("{}", json!({ "event": "owner_no_device" }));
      std::process::exit(5);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
  };

  let owner = Box::leak(Box::new(Owner {
    queue: ptr::null_mut(),
    held: BTreeSet::new(),
    output,
    last: ForwardState::default(),
    reports: 0,
  }));
  // SAFETY: Creates and starts a queue on the device before seizing it, as the helper does; all
  // objects live until process exit.
  let status = unsafe {
    let queue = IOHIDQueueCreate(kCFAllocatorDefault, device, 1024, 0);
    let elements = IOHIDDeviceCopyMatchingElements(device, ptr::null(), 0);
    for index in 0..CFArrayGetCount(elements) {
      let element = CFArrayGetValueAtIndex(elements, index) as IOHIDElementRef;
      if IOHIDElementGetUsagePage(element) == PAGE_KEYBOARD {
        IOHIDQueueAddElement(queue, element);
      }
    }
    IOHIDQueueRegisterValueAvailableCallback(queue, values, (owner as *mut Owner).cast());
    IOHIDQueueScheduleWithRunLoop(queue, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
    IOHIDQueueStart(queue);
    owner.queue = queue;
    IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice)
  };
  println!(
    "{}",
    json!({ "event": "owner_seized", "mode": mode, "status": format!("{status:#x}") })
  );
  // SAFETY: Runs this thread's run loop for the fixture duration.
  unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 0) };
  if let Some(output) = &owner.output {
    output.release_all().ok();
    output.shutdown();
  }
  println!("{}", json!({ "event": "owner_done", "reports": owner.reports }));
}

fn find_device(vendor: i64, product: i64) -> Option<IOHIDDeviceRef> {
  // SAFETY: Standard registry iteration; unmatched devices are released.
  unsafe {
    let mut iterator: io_iterator_t = 0;
    if IOServiceGetMatchingServices(0, IOServiceMatching(c"IOHIDDevice".as_ptr()), &mut iterator)
      != 0
    {
      return None;
    }
    let mut found = None;
    loop {
      let service = IOIteratorNext(iterator);
      if service == 0 {
        break;
      }
      let device = IOHIDDeviceCreate(kCFAllocatorDefault, service);
      IOObjectRelease(service);
      if device.is_null() {
        continue;
      }
      let key = |name: &str| {
        let key = CfString::new(name);
        cf_i64(IOHIDDeviceGetProperty(device, key.0))
      };
      if found.is_none() && key("VendorID") == Some(vendor) && key("ProductID") == Some(product) {
        found = Some(device);
      } else {
        CFRelease(device.cast_const());
      }
    }
    IOObjectRelease(iterator);
    found
  }
}

extern "C" fn values(context: *mut c_void, _result: IOReturn, _sender: *mut c_void) {
  // SAFETY: `context` is the leaked owner registered with the queue.
  let owner = unsafe { &mut *(context as *mut Owner) };
  loop {
    // SAFETY: The queue is live for the process lifetime.
    let value = unsafe { IOHIDQueueCopyNextValueWithTimeout(owner.queue, 0.0) };
    if value.is_null() {
      break;
    }
    // SAFETY: `value` is retained and released here.
    let (usage, pressed) = unsafe {
      let element = IOHIDValueGetElement(value);
      let usage = Usage::new(IOHIDElementGetUsagePage(element), IOHIDElementGetUsage(element));
      let pressed = IOHIDValueGetIntegerValue(value) != 0;
      CFRelease(value.cast_const());
      (usage, pressed)
    };
    if !usage.is_forwardable() {
      continue;
    }
    owner.reports += 1;
    let key = usage.usage as u16;
    if pressed {
      owner.held.insert(key);
    } else {
      owner.held.remove(&key);
    }
  }
  let Some(output) = &owner.output else { return };
  let mut next = ForwardState::default();
  for key in &owner.held {
    if (0xE0..=0xE7).contains(key) {
      next.modifiers |= 1 << (key - 0xE0);
    } else {
      next.keyboard.push(*key);
    }
  }
  if next != owner.last {
    output.post_state(&owner.last, &next).ok();
    owner.last = next;
  }
}
