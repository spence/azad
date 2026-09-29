//! Keyboard discovery, observation and seizure through IOKit.
//!
//! A device is seized only while no key is held on it: seizing mid-press would strand the
//! pre-seize key-down in the OS with no release. Until then it is opened non-exclusively and
//! only its held-key set is tracked. All methods run on the helper's main run loop.

use std::collections::{BTreeSet, HashMap};
use std::ffi::c_void;
use std::ptr;

use serde::Serialize;

use crate::engine::{DeviceTranslation, Usage};
use crate::sys::*;
use crate::vhid::{
  AZAD_OUTPUT_PRODUCT_ID, AZAD_OUTPUT_VENDOR_ID, VIRTUAL_KEYBOARD_MANUFACTURER,
  VIRTUAL_KEYBOARD_PRODUCT_PREFIX,
};

const QUEUE_DEPTH: CFIndex = 1024;
const PAGE_GENERIC_DESKTOP: u32 = 0x01;
const USAGE_KEYBOARD: u32 = 0x06;
const USAGE_KEYPAD: u32 = 0x07;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
  Keyboard,
  /// Output keyboard of another Karabiner-driver client (Karabiner-Elements, Kanata). Its
  /// presence means that client owns the physical keyboards, so Azad captures its output.
  RemapperOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "code", rename_all = "snake_case")]
pub enum DeviceState {
  Closed,
  /// Yielded to a remapper that owns physical keyboards.
  Yielded,
  Observing,
  Seized,
  OwnedByOther,
  Failed(i32),
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
  pub id: u64,
  pub kind: DeviceKind,
  pub vendor_id: u32,
  pub product_id: u32,
  pub product: String,
  pub transport: String,
  pub state: DeviceState,
  pub seized_reports: u64,
  /// The keyboard publishes OS key translations (F-row, fn combinations) the helper applies.
  pub key_translation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
  Off,
  Capture,
}

/// Events queued for the helper core. They are delivered through a run-loop source rather than
/// inline so the core may reconfigure devices without re-entering an IOKit callback.
#[derive(Debug)]
pub enum DeviceEvent {
  Batch {
    device: u64,
    values: Vec<(Usage, bool)>,
    timestamp: u64,
    /// Present for keyboards with OS key translations, with the F-key mode read at this batch.
    translation: Option<DeviceTranslation>,
  },
  /// A seized device stopped delivering input (removed or released).
  Released {
    device: u64,
  },
  Changed,
}

struct Device {
  manager: *mut Manager,
  service_id: u64,
  device: IOHIDDeviceRef,
  queue: IOHIDQueueRef,
  info: DeviceInfo,
  open_options: Option<IOOptionBits>,
  held: BTreeSet<Usage>,
  translation: Option<DeviceTranslation>,
}

pub struct Manager {
  run_loop: CFRunLoopRef,
  devices: HashMap<u64, Box<Device>>,
  mode: Mode,
  notify: CFRunLoopSourceRef,
  events: Vec<DeviceEvent>,
}

impl Manager {
  /// Creates the manager and arms IOKit notifications on the current run loop. `notify` is
  /// signalled whenever events are queued. The manager is leaked because IOKit callbacks keep
  /// raw pointers to it for the process lifetime.
  pub fn start(notify: CFRunLoopSourceRef) -> &'static mut Manager {
    let manager = Box::leak(Box::new(Manager {
      // SAFETY: Called on the thread whose run loop drives the helper.
      run_loop: unsafe { CFRunLoopGetCurrent() },
      devices: HashMap::new(),
      mode: Mode::Off,
      notify,
      events: Vec::new(),
    }));
    // SAFETY: The notification port and matching dictionary are created and handed to IOKit
    // according to its ownership rules; the dictionary reference is consumed by the call.
    unsafe {
      let port = IONotificationPortCreate(0);
      CFRunLoopAddSource(
        manager.run_loop,
        IONotificationPortGetRunLoopSource(port),
        kCFRunLoopDefaultMode,
      );
      let matching = IOServiceMatching(c"IOHIDDevice".as_ptr());
      let mut iterator: io_iterator_t = 0;
      IOServiceAddMatchingNotification(
        port,
        c"IOServiceFirstMatch".as_ptr(),
        matching,
        service_matched,
        (manager as *mut Manager).cast(),
        &mut iterator,
      );
      service_matched((manager as *mut Manager).cast(), iterator);
    }
    manager
  }

  /// Re-enumerates present devices. Device creation fails while Input Monitoring is denied, so
  /// devices that matched before a grant are picked up here.
  pub fn rescan(&mut self) {
    // SAFETY: The matching dictionary is consumed by the call; the iterator is released below.
    unsafe {
      let matching = IOServiceMatching(c"IOHIDDevice".as_ptr());
      let mut iterator: io_iterator_t = 0;
      if IOServiceGetMatchingServices(0, matching, &mut iterator) == 0 {
        service_matched((self as *mut Manager).cast(), iterator);
        IOObjectRelease(iterator);
      }
    }
  }

  pub fn devices(&self) -> Vec<DeviceInfo> {
    let mut devices: Vec<_> = self.devices.values().map(|device| device.info.clone()).collect();
    devices.sort_by_key(|device| device.id);
    devices
  }

  pub fn take_events(&mut self) -> Vec<DeviceEvent> {
    std::mem::take(&mut self.events)
  }

  fn push(&mut self, event: DeviceEvent) {
    self.events.push(event);
    // SAFETY: `notify` is a live run-loop source owned by the core for the process lifetime.
    unsafe { CFRunLoopSourceSignal(self.notify) };
  }

  pub fn seized_count(&self) -> usize {
    self
      .devices
      .values()
      .filter(|device| device.info.state == DeviceState::Seized)
      .count()
  }

  pub fn set_mode(&mut self, mode: Mode) {
    self.mode = mode;
    self.reconcile();
  }

  /// Moves every device toward its desired state; also retries failed opens.
  pub fn reconcile(&mut self) {
    let remapper_present = self
      .devices
      .values()
      .any(|device| device.info.kind == DeviceKind::RemapperOutput);
    let ids: Vec<u64> = self.devices.keys().copied().collect();
    let mut released = Vec::new();
    for id in ids {
      let mode = self.mode;
      let device = self.devices.get_mut(&id).expect("device");
      let yield_device = remapper_present && device.info.kind == DeviceKind::Keyboard;
      let was_seized = device.info.state == DeviceState::Seized;
      if mode == Mode::Off || yield_device {
        device.close();
        device.info.state = if yield_device && mode == Mode::Capture {
          DeviceState::Yielded
        } else {
          DeviceState::Closed
        };
      } else {
        device.advance();
      }
      if was_seized && device.info.state != DeviceState::Seized {
        released.push(id);
      }
    }
    for id in released {
      self.push(DeviceEvent::Released { device: id });
    }
    self.push(DeviceEvent::Changed);
  }

  fn add_service(&mut self, service: io_service_t) {
    if std::env::var_os("AZAD_CAPTURE_TRACE").is_some() {
      eprintln!("azad-capture: matched service {service}");
    }
    let mut service_id = 0u64;
    // SAFETY: `service` is a live registry entry from the notification iterator.
    unsafe { IORegistryEntryGetRegistryEntryID(service, &mut service_id) };
    if self.devices.contains_key(&service_id) {
      return;
    }
    // SAFETY: `service` is live; the returned device is owned by this function until stored.
    let device = unsafe { IOHIDDeviceCreate(kCFAllocatorDefault, service) };
    if device.is_null() {
      return;
    }
    // SAFETY: `device` is a live IOHIDDevice.
    let is_keyboard = unsafe {
      IOHIDDeviceConformsTo(device, PAGE_GENERIC_DESKTOP, USAGE_KEYBOARD) != 0
        || IOHIDDeviceConformsTo(device, PAGE_GENERIC_DESKTOP, USAGE_KEYPAD) != 0
    };
    let vendor_id = device_i64(device, "VendorID").unwrap_or(0) as u32;
    let product_id = device_i64(device, "ProductID").unwrap_or(0) as u32;
    let product = device_string(device, "Product").unwrap_or_default();
    let manufacturer = device_string(device, "Manufacturer").unwrap_or_default();
    let transport = device_string(device, "Transport").unwrap_or_default();
    let is_virtual_keyboard = manufacturer == VIRTUAL_KEYBOARD_MANUFACTURER
      && product.starts_with(VIRTUAL_KEYBOARD_PRODUCT_PREFIX);
    let is_own_output = is_virtual_keyboard
      && vendor_id == AZAD_OUTPUT_VENDOR_ID
      && product_id == AZAD_OUTPUT_PRODUCT_ID;
    if !is_keyboard || is_own_output {
      // SAFETY: Balances IOHIDDeviceCreate.
      unsafe { CFRelease(device.cast_const()) };
      return;
    }
    let kind = if is_virtual_keyboard { DeviceKind::RemapperOutput } else { DeviceKind::Keyboard };
    let mut boxed = Box::new(Device {
      manager: self,
      service_id,
      device,
      queue: ptr::null_mut(),
      info: DeviceInfo {
        id: service_id,
        kind,
        vendor_id,
        product_id,
        product,
        transport,
        state: DeviceState::Closed,
        seized_reports: 0,
        key_translation: false,
      },
      open_options: None,
      held: BTreeSet::new(),
      translation: read_translation(device),
    });
    boxed.info.key_translation = boxed.translation.is_some();
    let context: *mut Device = &mut *boxed;
    // SAFETY: `context` stays valid until the device is removed and unscheduled.
    unsafe {
      IOHIDDeviceRegisterRemovalCallback(device, device_removed, context.cast());
      IOHIDDeviceScheduleWithRunLoop(device, self.run_loop, kCFRunLoopDefaultMode);
    }
    self.devices.insert(service_id, boxed);
    self.reconcile();
  }

  fn remove_device(&mut self, service_id: u64) {
    let Some(mut device) = self.devices.remove(&service_id) else { return };
    let was_seized = device.info.state == DeviceState::Seized;
    device.close();
    // SAFETY: The device was scheduled on this run loop in `add_service`.
    unsafe {
      IOHIDDeviceUnscheduleFromRunLoop(device.device, self.run_loop, kCFRunLoopDefaultMode);
      CFRelease(device.device.cast_const());
    }
    if was_seized {
      self.push(DeviceEvent::Released { device: service_id });
    }
    self.reconcile();
  }
}

impl Device {
  /// Opens for observation, then seizes once no key is held.
  fn advance(&mut self) {
    match self.info.state {
      DeviceState::Seized => {}
      DeviceState::Observing if self.held.is_empty() => {
        self.close();
        self.open(kIOHIDOptionsTypeSeizeDevice);
      }
      DeviceState::Observing => {}
      _ => self.open(kIOHIDOptionsTypeNone),
    }
  }

  fn open(&mut self, options: IOOptionBits) {
    self.held.clear();
    self.start_queue();
    // SAFETY: `self.device` is a live IOHIDDevice owned by this record.
    let result = unsafe { IOHIDDeviceOpen(self.device, options) };
    if result != kIOReturnSuccess {
      self.stop_queue();
      self.info.state = if result == kIOReturnExclusiveAccess {
        DeviceState::OwnedByOther
      } else {
        DeviceState::Failed(result)
      };
      return;
    }
    self.open_options = Some(options);
    self.info.state = if options == kIOHIDOptionsTypeSeizeDevice {
      DeviceState::Seized
    } else {
      DeviceState::Observing
    };
  }

  fn close(&mut self) {
    self.stop_queue();
    if let Some(options) = self.open_options.take() {
      // SAFETY: Balances the successful IOHIDDeviceOpen with the same options.
      unsafe { IOHIDDeviceClose(self.device, options) };
    }
    self.held.clear();
  }

  fn start_queue(&mut self) {
    if !self.queue.is_null() {
      return;
    }
    // SAFETY: Creates a queue on the live device; elements are borrowed from the copied array
    // while it is alive, and the queue retains those it adds.
    unsafe {
      let queue = IOHIDQueueCreate(kCFAllocatorDefault, self.device, QUEUE_DEPTH, 0);
      if queue.is_null() {
        return;
      }
      let elements = IOHIDDeviceCopyMatchingElements(self.device, ptr::null(), 0);
      if !elements.is_null() {
        for index in 0..CFArrayGetCount(elements) {
          let element = CFArrayGetValueAtIndex(elements, index) as IOHIDElementRef;
          let kind = IOHIDElementGetType(element);
          let is_input = kind == kIOHIDElementTypeInput_Misc
            || kind == kIOHIDElementTypeInput_Button
            || kind == kIOHIDElementTypeInput_ScanCodes;
          let usage = Usage::new(IOHIDElementGetUsagePage(element), IOHIDElementGetUsage(element));
          if is_input && usage.is_forwardable() {
            IOHIDQueueAddElement(queue, element);
          }
        }
        CFRelease(elements);
      }
      IOHIDQueueRegisterValueAvailableCallback(
        queue,
        values_available,
        (self as *mut Device).cast(),
      );
      IOHIDQueueScheduleWithRunLoop(queue, (*self.manager).run_loop, kCFRunLoopDefaultMode);
      IOHIDQueueStart(queue);
      self.queue = queue;
    }
  }

  fn stop_queue(&mut self) {
    if self.queue.is_null() {
      return;
    }
    // SAFETY: The queue was created, scheduled, and started by `start_queue` on this run loop.
    unsafe {
      IOHIDQueueStop(self.queue);
      IOHIDQueueUnscheduleFromRunLoop(self.queue, (*self.manager).run_loop, kCFRunLoopDefaultMode);
      CFRelease(self.queue.cast_const());
    }
    self.queue = ptr::null_mut();
  }

  fn drain(&mut self) {
    let seized = self.info.state == DeviceState::Seized;
    let mut batch = Vec::new();
    let mut batch_time = None;
    loop {
      // SAFETY: The queue is live while this record holds it; nothing below closes it.
      let value = unsafe { IOHIDQueueCopyNextValueWithTimeout(self.queue, 0.0) };
      if value.is_null() {
        break;
      }
      // SAFETY: `value` is a retained IOHIDValue released below.
      let (usage, pressed, timestamp) = unsafe {
        let element = IOHIDValueGetElement(value);
        let usage = Usage::new(IOHIDElementGetUsagePage(element), IOHIDElementGetUsage(element));
        let pressed = IOHIDValueGetIntegerValue(value) != 0;
        let timestamp = mach_to_nanos(IOHIDValueGetTimeStamp(value));
        CFRelease(value.cast_const());
        (usage, pressed, timestamp)
      };
      if !usage.is_forwardable() {
        continue;
      }
      if let Some(time) = batch_time.filter(|time| *time != timestamp) {
        self.flush(seized, std::mem::take(&mut batch), time);
      }
      batch_time = Some(timestamp);
      batch.push((usage, pressed));
    }
    if let Some(time) = batch_time {
      self.flush(seized, batch, time);
    }
    // SAFETY: Invoked from an IOKit callback on the main run loop, where the manager lives.
    let manager = unsafe { &mut *self.manager };
    if !seized && self.held.is_empty() && manager.mode == Mode::Capture {
      self.advance();
      manager.push(DeviceEvent::Changed);
    }
  }

  fn flush(&mut self, seized: bool, values: Vec<(Usage, bool)>, timestamp: u64) {
    for &(usage, pressed) in &values {
      if pressed {
        self.held.insert(usage);
      } else {
        self.held.remove(&usage);
      }
    }
    if seized {
      self.info.seized_reports += 1;
      // SAFETY: Invoked from an IOKit callback on the main run loop, where the manager lives.
      let manager = unsafe { &mut *self.manager };
      let translation = self.translation.clone().map(|mut translation| {
        translation.standard_function_keys = standard_function_keys(self.device);
        translation
      });
      manager.push(DeviceEvent::Batch { device: self.service_id, values, timestamp, translation });
    }
  }
}

/// Searches the device's registry subtree (its HID event service) for `key`; returns a retained
/// value or null.
fn service_property(device: IOHIDDeviceRef, key: &str) -> CFTypeRef {
  let key = CfString::new(key);
  // SAFETY: `device` is live; the search returns a retained object or null.
  unsafe {
    IORegistryEntrySearchCFProperty(
      IOHIDDeviceGetService(device),
      c"IOService".as_ptr(),
      key.0,
      kCFAllocatorDefault,
      kIORegistryIterateRecursively,
    )
  }
}

fn service_string(device: IOHIDDeviceRef, key: &str) -> Option<String> {
  let value = service_property(device, key);
  // SAFETY: `value` is null or a retained CF object released here.
  unsafe {
    let text = cf_string(value);
    if !value.is_null() {
      CFRelease(value);
    }
    text
  }
}

fn read_translation(device: IOHIDDeviceRef) -> Option<DeviceTranslation> {
  let fn_function =
    service_string(device, "FnFunctionUsageMap").map(|t| DeviceTranslation::parse_map(&t));
  let fn_keyboard =
    service_string(device, "FnKeyboardUsageMap").map(|t| DeviceTranslation::parse_map(&t));
  if fn_function.is_none() && fn_keyboard.is_none() {
    return None;
  }
  Some(DeviceTranslation {
    fn_function: fn_function.unwrap_or_default(),
    fn_keyboard: fn_keyboard.unwrap_or_default(),
    standard_function_keys: standard_function_keys(device),
  })
}

/// Reads the live "standard function keys" setting the HID system applies to this keyboard.
fn standard_function_keys(device: IOHIDDeviceRef) -> bool {
  let properties = service_property(device, "HIDEventServiceProperties");
  if properties.is_null() {
    return false;
  }
  // SAFETY: `properties` is a retained CF object; the mode value is borrowed from it.
  unsafe {
    let mode = if CFGetTypeID(properties) == CFDictionaryGetTypeID() {
      let key = CfString::new("HIDFKeyMode");
      cf_i64(CFDictionaryGetValue(properties, key.0))
    } else {
      None
    };
    CFRelease(properties);
    mode == Some(1)
  }
}

fn device_property(device: IOHIDDeviceRef, key: &str) -> CFTypeRef {
  let key = CfString::new(key);
  // SAFETY: `device` is live; the returned property is borrowed (get rule).
  unsafe { IOHIDDeviceGetProperty(device, key.0) }
}

fn device_i64(device: IOHIDDeviceRef, key: &str) -> Option<i64> {
  // SAFETY: Device properties are live CF objects owned by the device.
  unsafe { cf_i64(device_property(device, key)) }
}

fn device_string(device: IOHIDDeviceRef, key: &str) -> Option<String> {
  // SAFETY: Device properties are live CF objects owned by the device.
  unsafe { cf_string(device_property(device, key)) }
}

extern "C" fn service_matched(refcon: *mut c_void, iterator: io_iterator_t) {
  // SAFETY: `refcon` is the leaked manager registered with this notification.
  let manager = unsafe { &mut *(refcon as *mut Manager) };
  loop {
    // SAFETY: `iterator` is the live notification iterator.
    let service = unsafe { IOIteratorNext(iterator) };
    if service == 0 {
      break;
    }
    manager.add_service(service);
    // SAFETY: Balances the reference returned by IOIteratorNext.
    unsafe { IOObjectRelease(service) };
  }
}

extern "C" fn device_removed(context: *mut c_void, _result: IOReturn, _sender: *mut c_void) {
  // SAFETY: `context` is the device record registered in `add_service`, still owned by the map.
  let device = unsafe { &mut *(context as *mut Device) };
  let service_id = device.service_id;
  // SAFETY: The manager outlives every device.
  unsafe { (*device.manager).remove_device(service_id) };
}

extern "C" fn values_available(context: *mut c_void, result: IOReturn, _sender: *mut c_void) {
  if result != kIOReturnSuccess {
    return;
  }
  // SAFETY: `context` is the device record that owns the queue.
  let device = unsafe { &mut *(context as *mut Device) };
  if device.open_options.is_none() || device.queue.is_null() {
    return;
  }
  device.drain();
}
