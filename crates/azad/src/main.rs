#![allow(deprecated)]
#![allow(unexpected_cfgs)]
#![allow(unsafe_op_in_unsafe_fn)]

mod app;
mod config;
mod connectors;
mod device;
mod hotkey_sm;
mod input_log;
mod interaction_sm;
mod metrics_log;
mod model_download;
mod models;
mod platform;
mod preferred_store;
mod settings;
mod single_instance;
mod speech;
mod transcript_history;

fn main() {
  let _single_instance_guard = match single_instance::acquire_primary_instance_lock() {
    Ok(guard) => guard,
    Err(single_instance::SingleInstanceError::AlreadyRunning) => {
      let focused = platform::focus_existing_instance("ai.azad");
      eprintln!("Azad: secondary launch detected; existing instance focus attempted: {focused}");
      return;
    }
    Err(err) => {
      eprintln!("Azad: failed to establish single-instance lock: {err}");
      return;
    }
  };

  app::run();
}
