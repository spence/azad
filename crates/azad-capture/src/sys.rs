//! Minimal CoreFoundation, IOKit and Security declarations used by the helper.

#![allow(non_upper_case_globals, non_camel_case_types)]

use std::ffi::{CStr, CString, c_char, c_void};

pub type CFTypeRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type CFArrayRef = *const c_void;
pub type CFDataRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFRunLoopRef = *mut c_void;
pub type CFRunLoopSourceRef = *mut c_void;
pub type CFRunLoopTimerRef = *mut c_void;
pub type CFIndex = isize;
pub type CFTypeID = usize;
pub type CFOptionFlags = usize;
pub type CFTimeInterval = f64;
pub type CFAbsoluteTime = f64;
pub type Boolean = u8;

pub type IOReturn = i32;
pub type IOOptionBits = u32;
pub type kern_return_t = i32;
pub type mach_port_t = u32;
pub type io_object_t = mach_port_t;
pub type io_iterator_t = io_object_t;
pub type io_service_t = io_object_t;
pub type IONotificationPortRef = *mut c_void;
pub type IOHIDDeviceRef = *mut c_void;
pub type IOHIDQueueRef = *mut c_void;
pub type IOHIDValueRef = *mut c_void;
pub type IOHIDElementRef = *mut c_void;
pub type SecCodeRef = *mut c_void;
pub type SecStaticCodeRef = *mut c_void;
pub type SecRequirementRef = *mut c_void;
pub type OSStatus = i32;

pub type IOServiceMatchingCallback = extern "C" fn(refcon: *mut c_void, iterator: io_iterator_t);
pub type IOHIDCallback = extern "C" fn(context: *mut c_void, result: IOReturn, sender: *mut c_void);
pub type CFRunLoopTimerCallBack = extern "C" fn(timer: CFRunLoopTimerRef, info: *mut c_void);

pub const kIOReturnSuccess: IOReturn = 0;
pub const kIOReturnExclusiveAccess: IOReturn = 0xe00002c5_u32 as i32;
pub const kIOReturnNotPermitted: IOReturn = 0xe00002e2_u32 as i32;
pub const kIOHIDOptionsTypeNone: IOOptionBits = 0;
pub const kIOHIDOptionsTypeSeizeDevice: IOOptionBits = 1;
pub const kIOHIDElementTypeInput_Misc: u32 = 1;
pub const kIOHIDElementTypeInput_Button: u32 = 2;
pub const kIOHIDElementTypeInput_ScanCodes: u32 = 4;
pub const kIOHIDRequestTypeListenEvent: u32 = 1;
pub const kIOHIDAccessTypeGranted: u32 = 0;
pub const kIOHIDAccessTypeDenied: u32 = 1;
pub const kCFNumberSInt64Type: CFIndex = 4;
pub const kSecCSDefaultFlags: u32 = 0;
pub const kSecCSSigningInformation: u32 = 2;
pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;

#[repr(C)]
pub struct CFRunLoopSourceContext {
  pub version: CFIndex,
  pub info: *mut c_void,
  pub retain: *const c_void,
  pub release: *const c_void,
  pub copy_description: *const c_void,
  pub equal: *const c_void,
  pub hash: *const c_void,
  pub schedule: *const c_void,
  pub cancel: *const c_void,
  pub perform: extern "C" fn(info: *mut c_void),
}

#[repr(C)]
pub struct CFRunLoopTimerContext {
  pub version: CFIndex,
  pub info: *mut c_void,
  pub retain: *const c_void,
  pub release: *const c_void,
  pub copy_description: *const c_void,
}

#[repr(C)]
#[derive(Default)]
pub struct mach_timebase_info_data_t {
  pub numer: u32,
  pub denom: u32,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
  pub static kCFAllocatorDefault: CFAllocatorRef;
  pub static kCFRunLoopDefaultMode: CFStringRef;
  pub static kCFRunLoopCommonModes: CFStringRef;
  pub fn CFRelease(value: CFTypeRef);
  pub fn CFRetain(value: CFTypeRef) -> CFTypeRef;
  pub fn CFGetTypeID(value: CFTypeRef) -> CFTypeID;
  pub fn CFNumberGetTypeID() -> CFTypeID;
  pub fn CFStringGetTypeID() -> CFTypeID;
  pub fn CFNumberGetValue(number: CFNumberRef, kind: CFIndex, out: *mut c_void) -> Boolean;
  pub fn CFStringCreateWithCString(
    allocator: CFAllocatorRef,
    value: *const c_char,
    encoding: u32,
  ) -> CFStringRef;
  pub fn CFStringGetCString(
    value: CFStringRef,
    buffer: *mut c_char,
    size: CFIndex,
    encoding: u32,
  ) -> Boolean;
  pub fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
  pub fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
  pub fn CFDataCreate(allocator: CFAllocatorRef, bytes: *const u8, length: CFIndex) -> CFDataRef;
  pub fn CFDictionaryCreate(
    allocator: CFAllocatorRef,
    keys: *const *const c_void,
    values: *const *const c_void,
    count: CFIndex,
    key_callbacks: *const c_void,
    value_callbacks: *const c_void,
  ) -> CFDictionaryRef;
  pub fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
  pub static kCFTypeDictionaryKeyCallBacks: c_void;
  pub static kCFTypeDictionaryValueCallBacks: c_void;
  pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
  pub fn CFRunLoopRun();
  pub fn CFRunLoopRunInMode(
    mode: CFStringRef,
    seconds: CFTimeInterval,
    return_after: Boolean,
  ) -> i32;
  pub fn CFRunLoopWakeUp(run_loop: CFRunLoopRef);
  pub fn CFRunLoopAddSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
  pub fn CFRunLoopSourceCreate(
    allocator: CFAllocatorRef,
    order: CFIndex,
    context: *mut CFRunLoopSourceContext,
  ) -> CFRunLoopSourceRef;
  pub fn CFRunLoopSourceSignal(source: CFRunLoopSourceRef);
  pub fn CFAbsoluteTimeGetCurrent() -> CFAbsoluteTime;
  pub fn CFRunLoopTimerCreate(
    allocator: CFAllocatorRef,
    fire_date: CFAbsoluteTime,
    interval: CFTimeInterval,
    flags: CFOptionFlags,
    order: CFIndex,
    callout: CFRunLoopTimerCallBack,
    context: *mut CFRunLoopTimerContext,
  ) -> CFRunLoopTimerRef;
  pub fn CFRunLoopAddTimer(run_loop: CFRunLoopRef, timer: CFRunLoopTimerRef, mode: CFStringRef);
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
  pub fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
  pub fn IONotificationPortCreate(main_port: mach_port_t) -> IONotificationPortRef;
  pub fn IONotificationPortGetRunLoopSource(port: IONotificationPortRef) -> CFRunLoopSourceRef;
  pub fn IOServiceAddMatchingNotification(
    port: IONotificationPortRef,
    notification_type: *const c_char,
    matching: CFDictionaryRef,
    callback: IOServiceMatchingCallback,
    refcon: *mut c_void,
    notification: *mut io_iterator_t,
  ) -> kern_return_t;
  pub fn IOIteratorNext(iterator: io_iterator_t) -> io_object_t;
  pub fn IOServiceGetMatchingServices(
    main_port: mach_port_t,
    matching: CFDictionaryRef,
    iterator: *mut io_iterator_t,
  ) -> kern_return_t;
  pub fn IOObjectRelease(object: io_object_t) -> kern_return_t;
  pub fn IORegistryEntryGetRegistryEntryID(entry: io_object_t, id: *mut u64) -> kern_return_t;
  pub fn IOHIDDeviceCreate(allocator: CFAllocatorRef, service: io_service_t) -> IOHIDDeviceRef;
  pub fn IOHIDDeviceOpen(device: IOHIDDeviceRef, options: IOOptionBits) -> IOReturn;
  pub fn IOHIDDeviceClose(device: IOHIDDeviceRef, options: IOOptionBits) -> IOReturn;
  pub fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> CFTypeRef;
  pub fn IOHIDDeviceConformsTo(device: IOHIDDeviceRef, page: u32, usage: u32) -> Boolean;
  pub fn IOHIDDeviceCopyMatchingElements(
    device: IOHIDDeviceRef,
    matching: CFDictionaryRef,
    options: IOOptionBits,
  ) -> CFArrayRef;
  pub fn IOHIDDeviceRegisterRemovalCallback(
    device: IOHIDDeviceRef,
    callback: IOHIDCallback,
    context: *mut c_void,
  );
  pub fn IOHIDDeviceScheduleWithRunLoop(
    device: IOHIDDeviceRef,
    run_loop: CFRunLoopRef,
    mode: CFStringRef,
  );
  pub fn IOHIDDeviceUnscheduleFromRunLoop(
    device: IOHIDDeviceRef,
    run_loop: CFRunLoopRef,
    mode: CFStringRef,
  );
  pub fn IOHIDQueueCreate(
    allocator: CFAllocatorRef,
    device: IOHIDDeviceRef,
    depth: CFIndex,
    options: IOOptionBits,
  ) -> IOHIDQueueRef;
  pub fn IOHIDQueueAddElement(queue: IOHIDQueueRef, element: IOHIDElementRef);
  pub fn IOHIDQueueRegisterValueAvailableCallback(
    queue: IOHIDQueueRef,
    callback: IOHIDCallback,
    context: *mut c_void,
  );
  pub fn IOHIDQueueScheduleWithRunLoop(
    queue: IOHIDQueueRef,
    run_loop: CFRunLoopRef,
    mode: CFStringRef,
  );
  pub fn IOHIDQueueUnscheduleFromRunLoop(
    queue: IOHIDQueueRef,
    run_loop: CFRunLoopRef,
    mode: CFStringRef,
  );
  pub fn IOHIDQueueStart(queue: IOHIDQueueRef);
  pub fn IOHIDQueueStop(queue: IOHIDQueueRef);
  pub fn IOHIDQueueCopyNextValueWithTimeout(queue: IOHIDQueueRef, timeout: f64) -> IOHIDValueRef;
  pub fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
  pub fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> CFIndex;
  pub fn IOHIDValueGetTimeStamp(value: IOHIDValueRef) -> u64;
  pub fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
  pub fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;
  pub fn IOHIDElementGetType(element: IOHIDElementRef) -> u32;
  pub fn IOHIDCheckAccess(request: u32) -> u32;
  pub fn IOHIDRequestAccess(request: u32) -> Boolean;
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
  pub static kSecGuestAttributeAudit: CFStringRef;
  pub static kSecCodeInfoTeamIdentifier: CFStringRef;
  pub static kSecCodeInfoIdentifier: CFStringRef;
  pub fn SecCodeCopySelf(flags: u32, code: *mut SecCodeRef) -> OSStatus;
  pub fn SecCodeCopyStaticCode(
    code: SecCodeRef,
    flags: u32,
    out: *mut SecStaticCodeRef,
  ) -> OSStatus;
  pub fn SecCodeCopySigningInformation(
    code: SecStaticCodeRef,
    flags: u32,
    information: *mut CFDictionaryRef,
  ) -> OSStatus;
  pub fn SecCodeCopyGuestWithAttributes(
    host: SecCodeRef,
    attributes: CFDictionaryRef,
    flags: u32,
    guest: *mut SecCodeRef,
  ) -> OSStatus;
  pub fn SecRequirementCreateWithString(
    text: CFStringRef,
    flags: u32,
    requirement: *mut SecRequirementRef,
  ) -> OSStatus;
  pub fn SecCodeCheckValidity(
    code: SecCodeRef,
    flags: u32,
    requirement: SecRequirementRef,
  ) -> OSStatus;
}

unsafe extern "C" {
  pub fn mach_timebase_info(info: *mut mach_timebase_info_data_t) -> kern_return_t;
  pub fn mach_absolute_time() -> u64;
}

/// Owned CoreFoundation string.
pub struct CfString(pub CFStringRef);

impl CfString {
  pub fn new(value: &str) -> Self {
    let value = CString::new(value).expect("CF string without NUL");
    // SAFETY: `value` is a valid NUL-terminated UTF-8 string for the duration of the call.
    Self(unsafe {
      CFStringCreateWithCString(kCFAllocatorDefault, value.as_ptr(), kCFStringEncodingUTF8)
    })
  }
}

impl Drop for CfString {
  fn drop(&mut self) {
    if !self.0.is_null() {
      // SAFETY: The string was created by this wrapper and is released once.
      unsafe { CFRelease(self.0) };
    }
  }
}

/// Reads a borrowed CF string value, returning `None` for other CF types.
///
/// # Safety
/// `value` must be null or a live CoreFoundation object.
pub unsafe fn cf_string(value: CFTypeRef) -> Option<String> {
  if value.is_null() {
    return None;
  }
  // SAFETY: `value` is a live CF object owned by the caller.
  if unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
    return None;
  }
  let mut buffer = [0 as c_char; 256];
  // SAFETY: The buffer is writable for its full length.
  let ok = unsafe {
    CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as CFIndex, kCFStringEncodingUTF8)
  };
  if ok == 0 {
    return None;
  }
  // SAFETY: CFStringGetCString NUL-terminates on success.
  Some(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().into_owned())
}

/// Reads a borrowed CF number as `i64`.
///
/// # Safety
/// `value` must be null or a live CoreFoundation object.
pub unsafe fn cf_i64(value: CFTypeRef) -> Option<i64> {
  if value.is_null() {
    return None;
  }
  // SAFETY: `value` is a live CF object owned by the caller.
  if unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
    return None;
  }
  let mut out = 0i64;
  // SAFETY: `out` is a valid destination for a 64-bit integer.
  let ok = unsafe { CFNumberGetValue(value, kCFNumberSInt64Type, (&mut out as *mut i64).cast()) };
  (ok != 0).then_some(out)
}

/// Nanoseconds for a `mach_absolute_time` value.
pub fn mach_to_nanos(ticks: u64) -> u64 {
  static TIMEBASE: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
  let (numer, denom) = *TIMEBASE.get_or_init(|| {
    let mut info = mach_timebase_info_data_t::default();
    // SAFETY: `info` is a valid out-parameter.
    unsafe { mach_timebase_info(&mut info) };
    (info.numer.max(1) as u64, info.denom.max(1) as u64)
  });
  ((ticks as u128 * numer as u128) / denom as u128) as u64
}

pub fn now_nanos() -> u64 {
  // SAFETY: No preconditions.
  mach_to_nanos(unsafe { mach_absolute_time() })
}
