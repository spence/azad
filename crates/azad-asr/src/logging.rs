use whisper_cpp_plus::whisper_cpp_plus_sys as ffi;

/// Disable whisper.cpp / ggml logging (prevents stdout/stderr spam that breaks the TUI).
pub fn init_quiet() {
  set_native_logging_enabled(false);
}

/// Restore whisper.cpp / ggml default logging callback behavior.
pub fn init_default() {
  set_native_logging_enabled(true);
}

/// Configure native whisper.cpp / ggml logging.
///
/// `enabled = false` suppresses native logs.
/// `enabled = true` restores upstream default log callback.
pub fn set_native_logging_enabled(enabled: bool) {
  unsafe {
    if enabled {
      ffi::whisper_log_set(None, std::ptr::null_mut());
    } else {
      ffi::whisper_log_set(Some(drop_log), std::ptr::null_mut());
    }
  }
}

unsafe extern "C" fn drop_log(
  _level: ffi::ggml_log_level,
  _text: *const ::core::ffi::c_char,
  _user_data: *mut ::core::ffi::c_void,
) {
  // Intentionally discard.
}
