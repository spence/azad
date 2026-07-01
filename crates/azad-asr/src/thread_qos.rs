#[cfg(target_os = "macos")]
mod imp {
  unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
  }

  // Values from macOS SDK sys/qos.h.
  const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
  const QOS_CLASS_USER_INITIATED: u32 = 0x19;
  const QOS_CLASS_UTILITY: u32 = 0x11;
  const QOS_CLASS_BACKGROUND: u32 = 0x09;

  fn set(qos_class: u32, relpri: i32) {
    // Best-effort: ignore failures (e.g., thread opted out of QoS system).
    unsafe {
      let _ = pthread_set_qos_class_self_np(qos_class, relpri);
    }
  }

  pub fn user_interactive() {
    set(QOS_CLASS_USER_INTERACTIVE, 0);
  }

  pub fn user_initiated() {
    set(QOS_CLASS_USER_INITIATED, 0);
  }

  pub fn utility() {
    set(QOS_CLASS_UTILITY, 0);
  }

  pub fn background() {
    set(QOS_CLASS_BACKGROUND, 0);
  }
}

#[cfg(not(target_os = "macos"))]
mod imp {
  pub fn user_interactive() {}
  pub fn user_initiated() {}
  pub fn utility() {}
  pub fn background() {}
}

pub fn user_interactive() {
  imp::user_interactive();
}

pub fn user_initiated() {
  imp::user_initiated();
}

pub fn utility() {
  imp::utility();
}

pub fn background() {
  imp::background();
}
