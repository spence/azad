use std::ffi::CStr;
use std::os::raw::c_char;

use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};

const PREFERRED_DEVICE_KEY: &str = "AzadPreferredInputDeviceId";
const ALWAYS_LISTENING_KEY: &str = "AzadAlwaysListeningEnabled";
const DEBUG_STATS_ENABLED_KEY: &str = "AzadDebugStatsEnabled";
const RUN_ON_STARTUP_KEY: &str = "AzadRunOnStartup";

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

pub fn load_always_listening_enabled() -> bool {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return true;
        }

        let key = NSString::alloc(nil).init_str(ALWAYS_LISTENING_KEY);
        let existing: id = msg_send![defaults, objectForKey: key];
        if existing == nil {
            return true;
        }

        let value: i8 = msg_send![defaults, boolForKey: key];
        value != 0
    }
}

pub fn save_always_listening_enabled(enabled: bool) {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return;
        }

        let key = NSString::alloc(nil).init_str(ALWAYS_LISTENING_KEY);
        let value = if enabled { YES } else { NO };
        let _: () = msg_send![defaults, setBool: value forKey: key];
    }
}

pub fn load_debug_stats_enabled() -> bool {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return false;
        }

        let key = NSString::alloc(nil).init_str(DEBUG_STATS_ENABLED_KEY);
        let existing: id = msg_send![defaults, objectForKey: key];
        if existing == nil {
            return false;
        }

        let value: i8 = msg_send![defaults, boolForKey: key];
        value != 0
    }
}

pub fn save_debug_stats_enabled(enabled: bool) {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return;
        }

        let key = NSString::alloc(nil).init_str(DEBUG_STATS_ENABLED_KEY);
        let value = if enabled { YES } else { NO };
        let _: () = msg_send![defaults, setBool: value forKey: key];
    }
}

pub fn load_run_on_startup_enabled() -> bool {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return true;
        }

        let key = NSString::alloc(nil).init_str(RUN_ON_STARTUP_KEY);
        let existing: id = msg_send![defaults, objectForKey: key];
        if existing == nil {
            return true;
        }

        let value: i8 = msg_send![defaults, boolForKey: key];
        value != 0
    }
}

pub fn save_run_on_startup_enabled(enabled: bool) {
    unsafe {
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        if defaults == nil {
            return;
        }

        let key = NSString::alloc(nil).init_str(RUN_ON_STARTUP_KEY);
        let value = if enabled { YES } else { NO };
        let _: () = msg_send![defaults, setBool: value forKey: key];
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
