/// Native helpers log through their own stderr streams. This hook remains so callers can
/// keep one logging setup path independent of the active ASR/VAD runtime.
pub fn init_quiet() {
  set_native_logging_enabled(false);
}

/// Restore native helper logging behavior.
pub fn init_default() {
  set_native_logging_enabled(true);
}

pub fn set_native_logging_enabled(_enabled: bool) {}
