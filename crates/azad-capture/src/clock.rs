//! The host monotonic clock both processes stamp key events with.

#[repr(C)]
struct TimebaseInfo {
  numer: u32,
  denom: u32,
}

unsafe extern "C" {
  fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
  fn mach_absolute_time() -> u64;
}

/// Nanoseconds for a `mach_absolute_time` value.
pub fn mach_to_nanos(ticks: u64) -> u64 {
  static TIMEBASE: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
  let (numer, denom) = *TIMEBASE.get_or_init(|| {
    let mut info = TimebaseInfo { numer: 0, denom: 0 };
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
