//! Build script: capture the git short SHA + build timestamp at compile time
//! and expose them as `env!()` constants to the binary. Surfaced in the
//! Settings window's bottom-right footer so the user can identify which
//! build is running without reaching for the terminal.

use std::process::Command;
use std::time::SystemTime;

fn main() {
  let git_sha = Command::new("git")
    .args(["rev-parse", "--short", "HEAD"])
    .output()
    .ok()
    .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "unknown".to_string());

  let dirty = Command::new("git")
    .args(["status", "--porcelain"])
    .output()
    .ok()
    .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
    .map(|s| !s.trim().is_empty())
    .unwrap_or(false);

  let git_label = if dirty { format!("{git_sha}-dirty") } else { git_sha };

  // "YYYY-MM-DD HH:MM" UTC timestamp. Avoids pulling chrono just for this.
  let now = SystemTime::now()
    .duration_since(SystemTime::UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0);
  let build_iso = format_utc_yyyy_mm_dd_hh_mm(now);

  println!("cargo:rustc-env=AZAD_BUILD_GIT_SHA={git_label}");
  println!("cargo:rustc-env=AZAD_BUILD_TIME={build_iso}");
  // Re-run when HEAD moves or the index changes so the SHA stays fresh
  // without forcing a full clean build.
  println!("cargo:rerun-if-changed=../../.git/HEAD");
  println!("cargo:rerun-if-changed=../../.git/index");
}

/// Convert a unix timestamp (seconds) to "YYYY-MM-DD HH:MM" in UTC. Hand-rolled
/// so the build script doesn't need a date crate. Handles civil-date conversion
/// via Howard Hinnant's days-from-civil algorithm
/// (https://howardhinnant.github.io/date_algorithms.html#civil_from_days).
fn format_utc_yyyy_mm_dd_hh_mm(unix_secs: i64) -> String {
  if unix_secs <= 0 {
    return "unknown".to_string();
  }
  let secs_per_day: i64 = 86_400;
  let days = unix_secs.div_euclid(secs_per_day);
  let secs_in_day = unix_secs.rem_euclid(secs_per_day);
  let hour = secs_in_day / 3600;
  let minute = (secs_in_day % 3600) / 60;

  let z = days + 719_468;
  let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
  let doe = z - era * 146_097;
  let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
  let y = yoe + era * 400;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let d = doy - (153 * mp + 2) / 5 + 1;
  let m = if mp < 10 { mp + 3 } else { mp - 9 };
  let y = if m <= 2 { y + 1 } else { y };

  format!("{y:04}-{m:02}-{d:02} {hour:02}:{minute:02}")
}
