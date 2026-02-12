use std::ffi::CStr;
use std::os::raw::c_char;

use cocoa::base::{id, nil};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};

const PREFERRED_DEVICE_KEY: &str = "AzadPreferredInputDeviceId";

pub fn load_preferred_device_id() -> Option<String> {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return None;
        }

        let key = NSString::alloc(nil).init_str(PREFERRED_DEVICE_KEY);
        let value: id = msg_send![defaults, stringForKey: key];
        nsstring_to_string(value)
    }
}

pub fn save_preferred_device_id(device_id: &str) {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return;
        }

        let key = NSString::alloc(nil).init_str(PREFERRED_DEVICE_KEY);
        let value = NSString::alloc(nil).init_str(device_id);
        let _: () = msg_send![defaults, setObject: value forKey: key];
    }
}

unsafe fn nsstring_to_string(value: id) -> Option<String> {
    if value == nil {
        return None;
    }

    let ptr: *const c_char = unsafe { msg_send![value, UTF8String] };
    if ptr.is_null() {
        return None;
    }

    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}
