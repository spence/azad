use std::time::Duration;

use crate::metrics_log;
use crate::model_download;
use crate::models::{self, PackStatus};
use crate::platform::{self, ConnectorRowVM, SettingsTab, SettingsViewModel};
use crate::preferred_store;
use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod};

use super::AppController;

impl AppController {
  pub(super) fn handle_menu_open_settings(&mut self) {
    platform::show_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_onboarding_get_started(&mut self) {
    eprintln!("AZAD_ONBOARDING get-started: completing onboarding");
    self.onboarding_complete = true;
    self.onboarding_active = false;
    preferred_store::save_onboarding_complete(true);
    platform::close_onboarding_window();
    self.ensure_session();
  }

  pub(super) fn handle_onboarding_set_trigger(&mut self, automatic: bool) {
    self.always_listening_enabled = automatic;
    preferred_store::save_always_listening_enabled(automatic);
  }

  pub(super) fn handle_onboarding_toggle_history(&mut self, enabled: bool) {
    self.history_enabled = enabled;
    preferred_store::save_history_enabled(enabled);
  }

  pub(super) fn handle_onboarding_toggle_append_trailing_space(&mut self, enabled: bool) {
    self.append_trailing_space_on_paste = enabled;
    preferred_store::save_append_trailing_space_on_paste(enabled);
  }

  pub(super) fn handle_onboarding_set_overlay_position(&mut self, pos: OverlayPosition) {
    self.overlay_position = pos;
    preferred_store::save_overlay_position(pos);
    platform::set_overlay_position(pos);
  }

  pub(super) fn handle_onboarding_set_listen_modifier(&mut self, bit: u8, enabled: bool) {
    let current = platform::listen_modifiers();
    let next = if enabled { current | bit } else { current & !bit };
    if next != 0 {
      platform::set_listen_modifiers(next);
      preferred_store::save_listen_modifiers(next);
    }
    platform::sync_onboarding_listen_modifiers(platform::listen_modifiers());
  }

  pub(super) fn handle_onboarding_select_device(&mut self, index: usize) {
    let device_id = self
      .device_snapshot
      .as_ref()
      .and_then(|s| s.devices.get(index))
      .map(|d| d.id.clone());
    if let Some(device_id) = device_id {
      self.handle_menu_select_device(device_id);
    }
  }

  pub(super) fn handle_onboarding_toggle_login(&mut self, enabled: bool) {
    self.run_on_startup_enabled = enabled;
    preferred_store::save_run_on_startup_enabled(enabled);
    self.apply_run_on_startup_preference();
  }

  pub(super) fn onboarding_view_model(&self) -> platform::OnboardingViewModel {
    let downloading = self.download_handle.is_some();
    let pack = models::pack_by_id(&self.active_pack_id).unwrap_or_else(models::default_pack);
    let header = format!("{} · {}", pack.display_name, models::format_size(pack.total_size_bytes));
    let path = models::pack_dir(&self.active_pack_id)
      .map(|p| {
        let s = p.display().to_string();
        match std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned()) {
          Some(home) => s.strip_prefix(&home).map(|rest| format!("~{rest}")).unwrap_or(s),
          None => s,
        }
      })
      .unwrap_or_default();
    let model_status_text = if downloading {
      let pct = if self.download_progress.1 > 0 {
        ((self.download_progress.0 as f64 / self.download_progress.1 as f64) * 100.0) as u8
      } else {
        0
      };
      format!("{header}\nDownloading… {pct}%")
    } else if self.models_ready {
      format!("{header}\n✓ Installed at {path}")
    } else {
      format!("{header}\nNot downloaded yet")
    };
    let download_enabled = !self.models_ready && !downloading;
    let accessibility_status = platform::accessibility_authorization();
    let microphone_status = platform::microphone_authorization();
    let get_started_enabled = (self.models_ready || downloading)
      && accessibility_status == platform::PermissionStatus::Granted
      && microphone_status == platform::PermissionStatus::Granted;
    let (devices, selected_device_index) = match &self.device_snapshot {
      Some(snapshot) => {
        let devices: Vec<(String, String)> =
          snapshot.devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
        let selected = snapshot
          .current_id
          .as_deref()
          .and_then(|cur| devices.iter().position(|(id, _)| id == cur));
        (devices, selected)
      }
      None => (Vec::new(), None),
    };
    platform::OnboardingViewModel {
      always_listening_enabled: self.always_listening_enabled,
      history_enabled: self.history_enabled,
      paste_method: self.paste_method,
      append_trailing_space_on_paste: self.append_trailing_space_on_paste,
      overlay_position: self.overlay_position,
      run_on_startup_enabled: self.run_on_startup_enabled,
      accessibility_status,
      microphone_status,
      model_status_text,
      download_enabled,
      get_started_enabled,
      devices,
      selected_device_index,
      listen_modifiers: platform::listen_modifiers(),
    }
  }

  pub(super) fn apply_run_on_startup_preference(&mut self) {
    if self.run_on_startup_enabled {
      platform::create_launch_agent_plist_if_missing();
    }
    if platform::launch_agent_plist_exists()
      && !platform::set_launch_agent_startup_enabled(self.run_on_startup_enabled)
    {
      eprintln!(
        "Azad: failed to apply run-on-startup preference (enabled={})",
        self.run_on_startup_enabled
      );
    }
  }

  pub(super) fn handle_settings_toggle_run_on_startup(&mut self, enabled: bool) {
    if platform::set_launch_agent_startup_enabled(enabled) {
      self.run_on_startup_enabled = enabled;
      preferred_store::save_run_on_startup_enabled(enabled);
    } else {
      eprintln!("Azad: failed to set run-on-startup to {enabled}");
    }
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_debug_stats(&mut self, enabled: bool) {
    self.debug_stats_enabled = enabled;
    preferred_store::save_debug_stats_enabled(enabled);
    platform::set_overlay_debug_logs_enabled(enabled);
    if let Some(session) = &self.session {
      session.set_debug_stats_enabled(enabled);
    }
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_select_paste_method(&mut self, method: PasteMethod) {
    self.paste_method = method;
    preferred_store::save_paste_method(method);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_select_auto_submit(&mut self, mode: AutoSubmitMode) {
    self.auto_submit_mode = mode;
    preferred_store::save_auto_submit_mode(mode);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_select_overlay_position(&mut self, pos: OverlayPosition) {
    self.overlay_position = pos;
    preferred_store::save_overlay_position(pos);
    platform::set_overlay_position(pos);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_append_trailing_space(&mut self, enabled: bool) {
    self.append_trailing_space_on_paste = enabled;
    preferred_store::save_append_trailing_space_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_connector(&mut self, index: usize, enabled: bool) {
    let Some(connector) = self.connectors.get_mut(index) else {
      return;
    };
    connector.enabled = enabled;
    let enabled_ids: Vec<String> =
      self.connectors.iter().filter(|c| c.enabled).map(|c| c.id.to_string()).collect();
    preferred_store::save_enabled_connector_ids(&enabled_ids);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_add_removed_word(&mut self, word: String) {
    let word = word.trim().to_ascii_lowercase();
    if word.is_empty() || self.removed_words.iter().any(|w| w == &word) {
      return;
    }
    self.removed_words.push(word);
    preferred_store::save_removed_words(&self.removed_words);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_remove_removed_word(&mut self, word: String) {
    self.removed_words.retain(|w| w != &word);
    preferred_store::save_removed_words(&self.removed_words);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_refresh(&mut self) {
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_download_model(&mut self, pack_id: &str) {
    if self.download_handle.is_some() {
      return;
    }
    let Some(pack) = models::pack_by_id(pack_id) else {
      return;
    };
    self.active_pack_id = pack_id.to_string();
    preferred_store::save_active_model_pack(pack_id);
    self.download_progress = (0, pack.total_size_bytes);
    self.download_handle = Some(model_download::start_pack_download(pack));
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_cancel_download(&mut self) {
    if let Some(handle) = self.download_handle.take() {
      handle.cancel();
    }
    self.download_progress = (0, 0);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_model_download_progress(
    &mut self,
    _pack_id: &str,
    bytes_done: u64,
    bytes_total: u64,
  ) {
    self.download_progress = (bytes_done, bytes_total);
    self.download_progress_dirty = true;
  }

  pub(super) fn handle_model_download_completed(&mut self, pack_id: &str) {
    self.download_handle = None;
    self.download_progress = (0, 0);
    self.active_pack_id = pack_id.to_string();
    preferred_store::save_active_model_pack(pack_id);
    self.refresh_models_ready();
    platform::update_settings_window(self.settings_view_model());
    if self.models_ready {
      if !self.onboarding_active {
        self.show_overlay_notice("Model ready", "Azad is ready to dictate", Duration::from_secs(4));
      }
      self.ensure_session();
    }
  }

  pub(super) fn handle_model_download_error(&mut self, _pack_id: &str, message: &str) {
    eprintln!("Azad: model download error: {message}");
    self.download_handle = None;
    self.download_progress = (0, 0);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn settings_view_model(&self) -> SettingsViewModel {
    let metrics_text = match metrics_log::summarize_last_24h() {
      Ok(summary) => metrics_log::render_summary(&summary),
      Err(err) => format!("Failed to load debug metrics: {err}"),
    };

    let pack = models::pack_by_id(&self.active_pack_id).unwrap_or_else(models::default_pack);
    let pack_status = if self.download_handle.is_some() {
      let pct = if self.download_progress.1 > 0 {
        ((self.download_progress.0 as f64 / self.download_progress.1 as f64) * 100.0) as u8
      } else {
        0
      };
      PackStatus::Downloading { progress_pct: pct }
    } else {
      models::check_pack_status(pack)
    };

    SettingsViewModel {
      selected_tab: SettingsTab::General,
      accessibility_status: platform::accessibility_authorization(),
      microphone_status: platform::microphone_authorization(),
      run_on_startup_enabled: self.run_on_startup_enabled,
      paste_method: self.paste_method,
      auto_submit_mode: self.auto_submit_mode,
      overlay_position: self.overlay_position,
      append_trailing_space_on_paste: self.append_trailing_space_on_paste,
      debug_stats_enabled: self.debug_stats_enabled,
      metrics_text,
      model_pack_display_name: pack.display_name.to_string(),
      model_pack_description: pack.description.to_string(),
      model_pack_size_label: models::format_size(pack.total_size_bytes),
      model_pack_status: pack_status,
      model_download_bytes_done: self.download_progress.0,
      model_download_bytes_total: self.download_progress.1,
      removed_words: self.removed_words.clone(),
      connectors: self
        .connectors
        .iter()
        .map(|c| ConnectorRowVM { display_name: c.display_name.to_string(), enabled: c.enabled })
        .collect(),
    }
  }
}
