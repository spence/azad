use std::time::Duration;

use crate::connectors;
use crate::metrics_log;
use crate::model_download;
use crate::models::{self, PackStatus};
use crate::platform::{self, ConnectorRowVM, SettingsTab, SettingsViewModel};
use crate::preferred_store;
use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod, StartupListenMode};
use crate::ui_model::{UiModelPack, UiModelStatus, UiPermissionStatus};

use super::AppController;

fn updated_listen_modifier_mask(current: u8, bit: u8, enabled: bool) -> u8 {
  let next = if enabled { current | bit } else { current & !bit };
  if next == 0 { current } else { next }
}

fn ui_permission_status(status: platform::PermissionStatus) -> UiPermissionStatus {
  match status {
    platform::PermissionStatus::Granted => UiPermissionStatus::Granted,
    platform::PermissionStatus::Denied => UiPermissionStatus::Denied,
    platform::PermissionStatus::NotDetermined => UiPermissionStatus::NotDetermined,
  }
}

fn ui_onboarding_accessibility_status(
  status: platform::PermissionStatus,
  requested: bool,
) -> UiPermissionStatus {
  match status {
    platform::PermissionStatus::Granted => UiPermissionStatus::Granted,
    _ if requested => UiPermissionStatus::Denied,
    _ => UiPermissionStatus::NotDetermined,
  }
}

fn onboarding_get_started_enabled(
  model_ready_or_downloading: bool,
  accessibility_status: platform::PermissionStatus,
  microphone_status: platform::PermissionStatus,
) -> bool {
  model_ready_or_downloading
    && accessibility_status == platform::PermissionStatus::Granted
    && microphone_status == platform::PermissionStatus::Granted
}

fn should_accept_download_event(
  active_pack_id: &str,
  event_pack_id: &str,
  download_active: bool,
) -> bool {
  download_active && active_pack_id == event_pack_id
}

fn ui_model_pack(
  pack: &models::ModelPackDef,
  pack_status: PackStatus,
  bytes_done: u64,
  bytes_total: u64,
  download_paused: bool,
) -> UiModelPack {
  let status = match pack_status {
    PackStatus::NotDownloaded => UiModelStatus::NotDownloaded,
    PackStatus::Downloading { .. } => UiModelStatus::Downloading,
    PackStatus::Resumable { .. } => UiModelStatus::Resumable,
    PackStatus::Ready => UiModelStatus::Ready,
    PackStatus::Incomplete => UiModelStatus::Failed,
  };
  let progress_pct = match pack_status {
    PackStatus::Downloading { progress_pct } => progress_pct,
    PackStatus::Resumable { progress_pct, .. } => progress_pct,
    PackStatus::Ready => 100,
    _ => 0,
  };
  let bytes_done = match pack_status {
    PackStatus::Resumable { bytes_done, .. } => bytes_done,
    _ => bytes_done,
  };
  let total = if bytes_total > 0 { bytes_total } else { pack.total_size_bytes };
  UiModelPack {
    id: pack.id.to_string(),
    welcome_name: "Nemotron-3.5 ASR Streaming".to_string(),
    settings_name: "NVIDIA Nemotron-3.5 ASR Streaming 0.6B".to_string(),
    page_url: pack.page_url.to_string(),
    description: "On-device streaming speech-to-text · English".to_string(),
    size_label: models::format_size(pack.total_size_bytes),
    status,
    download_paused,
    progress_pct,
    bytes_done_label: models::format_size(bytes_done),
    bytes_total_label: models::format_size(total),
    error_message: "Couldn't verify model files".to_string(),
  }
}

impl AppController {
  pub(super) fn handle_menu_open_settings(&mut self) {
    platform::show_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_onboarding_get_started(&mut self) {
    if !onboarding_get_started_enabled(
      self.models_ready || self.download_handle.is_some(),
      platform::accessibility_authorization(),
      platform::microphone_authorization(),
    ) {
      self.refresh_setup_surfaces();
      return;
    }

    eprintln!("AZAD_ONBOARDING get-started: completing onboarding");
    self.onboarding_complete = true;
    self.onboarding_active = false;
    self.last_onboarding_view_model = None;
    preferred_store::save_onboarding_complete(true);
    platform::close_onboarding_window();
    platform::set_status_item_visible(true);
    self.ensure_session_if_capture_should_be_live();
  }

  pub(super) fn handle_request_permission(&mut self, permission: &str) {
    match permission {
      "accessibility" => {
        preferred_store::save_accessibility_permission_requested(true);
        platform::request_accessibility_permission();
      }
      "microphone" => {}
      _ => return,
    }
    self.start_device_controller();
    self.refresh_setup_surfaces();
  }

  fn refresh_setup_surfaces(&mut self) {
    if self.onboarding_active {
      let model = self.onboarding_view_model();
      platform::update_onboarding_window(model.clone());
      self.last_onboarding_view_model = Some(model);
    }
    if platform::settings_window_is_open() {
      platform::update_settings_window(self.settings_view_model());
    }
  }

  pub(super) fn handle_onboarding_set_listen_modifier(&mut self, bit: u8, enabled: bool) {
    self.handle_listen_modifier_change(bit, enabled);
  }

  pub(super) fn handle_settings_set_listen_modifier(&mut self, bit: u8, enabled: bool) {
    self.handle_listen_modifier_change(bit, enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  fn handle_listen_modifier_change(&mut self, bit: u8, enabled: bool) {
    let current = platform::listen_modifiers();
    let next = updated_listen_modifier_mask(current, bit, enabled);
    if next != current {
      platform::set_listen_modifiers(next);
      preferred_store::save_listen_modifiers(next);
    }
    platform::sync_onboarding_listen_modifiers(platform::listen_modifiers());
    platform::sync_settings_listen_modifiers(platform::listen_modifiers());
  }

  pub(super) fn onboarding_view_model(&self) -> platform::OnboardingViewModel {
    let downloading = self.download_handle.is_some();
    let pack = models::pack_by_id(&self.active_pack_id).unwrap_or_else(models::default_pack);
    let pack_status = if downloading {
      let pct = if self.download_progress.1 > 0 {
        ((self.download_progress.0 as f64 / self.download_progress.1 as f64) * 100.0) as u8
      } else {
        0
      };
      PackStatus::Downloading { progress_pct: pct }
    } else if self.models_ready {
      PackStatus::Ready
    } else {
      models::check_pack_status(pack)
    };
    let accessibility_status = platform::accessibility_authorization();
    let microphone_status = platform::microphone_authorization();
    let get_started_enabled = onboarding_get_started_enabled(
      self.models_ready || downloading,
      accessibility_status,
      microphone_status,
    );
    platform::OnboardingViewModel {
      accessibility_status: ui_onboarding_accessibility_status(
        accessibility_status,
        preferred_store::load_accessibility_permission_requested(),
      ),
      microphone_status: ui_permission_status(microphone_status),
      model: ui_model_pack(
        pack,
        pack_status,
        self.download_progress.0,
        self.download_progress.1,
        self.download_handle.as_ref().is_some_and(|handle| handle.is_paused()),
      ),
      get_started_enabled,
      listen_modifiers: platform::listen_modifiers(),
    }
  }

  pub(super) fn apply_run_on_startup_preference(&self) -> bool {
    if self.run_on_startup_enabled {
      platform::create_launch_agent_plist_if_missing() && platform::enable_launch_agent_startup()
    } else {
      platform::remove_launch_agent_plist_if_present()
    }
  }

  pub(super) fn handle_settings_toggle_run_on_startup(&mut self, enabled: bool) {
    self.run_on_startup_enabled = enabled;
    if self.apply_run_on_startup_preference() {
      preferred_store::save_run_on_startup_enabled(enabled);
    } else {
      self.run_on_startup_enabled = !enabled;
      eprintln!("Azad: failed to set run-on-startup to {enabled}");
    }
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_select_startup_listen_mode(&mut self, mode: StartupListenMode) {
    self.startup_listen_mode = mode;
    preferred_store::save_startup_listen_mode(mode);
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

  pub(super) fn handle_settings_set_activation_level(&mut self, value: i64) {
    let value = value.clamp(0, 100);
    self.activation_level = value;
    let min_rms_db = super::start_min_rms_db_for_activation_level(value);
    preferred_store::save_activation_level(value);
    if let Some(session) = &self.session {
      session.set_start_min_rms_db(min_rms_db);
    }
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

  pub(super) fn handle_settings_toggle_history(&mut self, enabled: bool) {
    self.history_enabled = enabled;
    preferred_store::save_history_enabled(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_append_trailing_space(&mut self, enabled: bool) {
    self.append_trailing_space_on_paste = enabled;
    preferred_store::save_append_trailing_space_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_deduplicate_words(&mut self, enabled: bool) {
    self.deduplicate_words_on_paste = enabled;
    preferred_store::save_deduplicate_words_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_convert_number_words(&mut self, enabled: bool) {
    self.convert_number_words_on_paste = enabled;
    preferred_store::save_convert_number_words_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_convert_spoken_emoji(&mut self, enabled: bool) {
    self.convert_spoken_emoji_on_paste = enabled;
    preferred_store::save_convert_spoken_emoji_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_lowercase_except_uppercase_words(&mut self, enabled: bool) {
    self.lowercase_except_uppercase_words_on_paste = enabled;
    preferred_store::save_lowercase_except_uppercase_words_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_remove_hesitations(&mut self, enabled: bool) {
    self.remove_hesitations_on_paste = enabled;
    preferred_store::save_remove_hesitations_on_paste(enabled);
    platform::update_settings_window(self.settings_view_model());
  }

  pub(super) fn handle_settings_toggle_connector(&mut self, index: usize, enabled: bool) {
    let Some(id) = self.connectors.get(index).map(|c| c.id) else {
      return;
    };
    // Azad: refuse enable when the device is ineligible for Apple Intelligence.
    if enabled && id == connectors::AZAD_CONNECTOR_ID {
      self.refresh_apple_lm_availability(true);
      if !self.apple_lm_availability.state.can_enable_connector() {
        platform::update_settings_window(self.settings_view_model());
        return;
      }
    }
    // Spotify: hard gate on desktop app install.
    if enabled && id == connectors::SPOTIFY_CONNECTOR_ID {
      self.spotify_app_installed = crate::spotify_client::spotify_app_installed();
      if !self.spotify_app_installed {
        platform::update_settings_window(self.settings_view_model());
        return;
      }
    }
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
    self.refresh_apple_lm_availability(true);
    self.spotify_app_installed = crate::spotify_client::spotify_app_installed();
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
    let progress = models::pack_download_progress(pack);
    self.download_progress = (progress.bytes_done, pack.total_size_bytes);
    self.download_handle = Some(model_download::start_pack_download(pack));
    self.refresh_setup_surfaces();
  }

  pub(super) fn handle_settings_set_download_paused(&mut self, paused: bool) {
    let Some(handle) = self.download_handle.as_ref() else {
      return;
    };
    if paused {
      handle.pause();
      eprintln!("AZAD_MODEL_DOWNLOAD event=paused pack_id={}", self.active_pack_id);
    } else {
      handle.resume();
      eprintln!("AZAD_MODEL_DOWNLOAD event=resumed pack_id={}", self.active_pack_id);
    }
    self.refresh_setup_surfaces();
  }

  pub(super) fn handle_model_download_progress(
    &mut self,
    pack_id: &str,
    bytes_done: u64,
    bytes_total: u64,
  ) {
    if !should_accept_download_event(&self.active_pack_id, pack_id, self.download_handle.is_some())
    {
      return;
    }
    self.download_progress = (bytes_done, bytes_total);
    self.download_progress_dirty = true;
  }

  pub(super) fn handle_model_download_completed(&mut self, pack_id: &str) {
    if !should_accept_download_event(&self.active_pack_id, pack_id, self.download_handle.is_some())
    {
      return;
    }
    self.download_handle = None;
    self.download_progress = (0, 0);
    self.active_pack_id = pack_id.to_string();
    preferred_store::save_active_model_pack(pack_id);
    self.refresh_models_ready();
    self.refresh_setup_surfaces();
    if self.models_ready {
      if !self.onboarding_active {
        self.show_overlay_notice("Model ready", "Azad is ready to dictate", Duration::from_secs(4));
      }
      self.ensure_session_if_capture_should_be_live();
    }
  }

  pub(super) fn handle_model_download_error(&mut self, pack_id: &str, message: &str) {
    if !should_accept_download_event(&self.active_pack_id, pack_id, self.download_handle.is_some())
    {
      return;
    }
    eprintln!("Azad: model download error: {message}");
    self.download_handle = None;
    self.download_progress = (0, 0);
    self.refresh_setup_surfaces();
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
      accessibility_status: ui_permission_status(platform::accessibility_authorization()),
      microphone_status: ui_permission_status(platform::microphone_authorization()),
      run_on_startup_enabled: self.run_on_startup_enabled,
      startup_listen_mode_index: self.startup_listen_mode.ui_index(),
      activation_level: self.activation_level,
      history_enabled: self.history_enabled,
      paste_method_index: self.paste_method.ui_index(),
      auto_submit_index: self.auto_submit_mode.ui_index(),
      overlay_position_index: self.overlay_position.ui_index(),
      append_trailing_space_on_paste: self.append_trailing_space_on_paste,
      deduplicate_words_on_paste: self.deduplicate_words_on_paste,
      convert_number_words_on_paste: self.convert_number_words_on_paste,
      convert_spoken_emoji_on_paste: self.convert_spoken_emoji_on_paste,
      lowercase_except_uppercase_words_on_paste: self.lowercase_except_uppercase_words_on_paste,
      remove_hesitations_on_paste: self.remove_hesitations_on_paste,
      listen_modifiers: platform::listen_modifiers(),
      debug_stats_enabled: self.debug_stats_enabled,
      metrics_text,
      model: ui_model_pack(
        pack,
        pack_status,
        self.download_progress.0,
        self.download_progress.1,
        self.download_handle.as_ref().is_some_and(|handle| handle.is_paused()),
      ),
      removed_words: self.removed_words.clone(),
      connectors: self.connector_rows_vm(),
      build_info: format!("build {} · {}", env!("AZAD_BUILD_GIT_SHA"), env!("AZAD_BUILD_TIME")),
    }
  }

  fn connector_rows_vm(&self) -> Vec<ConnectorRowVM> {
    self
      .connectors
      .iter()
      .map(|c| {
        if c.id == connectors::AZAD_CONNECTOR_ID {
          let state = self.apple_lm_availability.state;
          ConnectorRowVM {
            id: c.id.to_string(),
            display_name: c.display_name.to_string(),
            trigger: c.trigger.to_string(),
            enabled: c.enabled,
            can_enable: state.can_enable_connector(),
            availability_state: state.as_str().to_string(),
            availability_message: state.message().to_string(),
            show_open_settings: state.show_open_settings(),
          }
        } else if c.id == connectors::SPOTIFY_CONNECTOR_ID {
          let installed = self.spotify_app_installed;
          ConnectorRowVM {
            id: c.id.to_string(),
            display_name: c.display_name.to_string(),
            trigger: c.trigger.to_string(),
            enabled: c.enabled && installed,
            can_enable: installed,
            availability_state: if installed {
              "available".to_string()
            } else {
              "notInstalled".to_string()
            },
            availability_message: if installed {
              "Say “hey spotify …” to control playback. Shazam identify coming next.".to_string()
            } else {
              "Install the Spotify app to enable this connector.".to_string()
            },
            show_open_settings: !installed,
          }
        } else {
          ConnectorRowVM {
            id: c.id.to_string(),
            display_name: c.display_name.to_string(),
            trigger: c.trigger.to_string(),
            enabled: c.enabled,
            can_enable: true,
            availability_state: "available".to_string(),
            availability_message: String::new(),
            show_open_settings: false,
          }
        }
      })
      .collect()
  }
}

#[cfg(test)]
mod tests {
  use crate::platform::{MOD_COMMAND, MOD_CONTROL, MOD_OPTION, MOD_SHIFT};

  use super::{
    onboarding_get_started_enabled, should_accept_download_event,
    ui_onboarding_accessibility_status, ui_permission_status, updated_listen_modifier_mask,
  };
  use crate::ui_model::UiPermissionStatus;

  #[test]
  fn listen_modifier_update_adds_and_removes_bits() {
    let mask = updated_listen_modifier_mask(MOD_OPTION, MOD_COMMAND, true);
    assert_eq!(mask, MOD_OPTION | MOD_COMMAND);

    let mask = updated_listen_modifier_mask(mask, MOD_OPTION, false);
    assert_eq!(mask, MOD_COMMAND);

    let mask = updated_listen_modifier_mask(mask, MOD_CONTROL, true);
    assert_eq!(mask, MOD_COMMAND | MOD_CONTROL);
  }

  #[test]
  fn listen_modifier_update_keeps_last_modifier() {
    assert_eq!(updated_listen_modifier_mask(MOD_SHIFT, MOD_SHIFT, false), MOD_SHIFT);
    assert_eq!(updated_listen_modifier_mask(MOD_OPTION, MOD_OPTION, false), MOD_OPTION);
  }

  #[test]
  fn ui_permission_status_preserves_native_state() {
    assert_eq!(
      ui_permission_status(crate::platform::PermissionStatus::Granted),
      UiPermissionStatus::Granted
    );
    assert_eq!(
      ui_permission_status(crate::platform::PermissionStatus::Denied),
      UiPermissionStatus::Denied
    );
    assert_eq!(
      ui_permission_status(crate::platform::PermissionStatus::NotDetermined),
      UiPermissionStatus::NotDetermined
    );
  }

  #[test]
  fn onboarding_accessibility_shows_request_until_user_requests_it() {
    assert_eq!(
      ui_onboarding_accessibility_status(crate::platform::PermissionStatus::Denied, false),
      UiPermissionStatus::NotDetermined
    );
    assert_eq!(
      ui_onboarding_accessibility_status(crate::platform::PermissionStatus::Denied, true),
      UiPermissionStatus::Denied
    );
  }

  #[test]
  fn onboarding_completion_requires_permissions() {
    assert!(onboarding_get_started_enabled(
      true,
      crate::platform::PermissionStatus::Granted,
      crate::platform::PermissionStatus::Granted,
    ));
    assert!(!onboarding_get_started_enabled(
      true,
      crate::platform::PermissionStatus::Denied,
      crate::platform::PermissionStatus::Granted,
    ));
    assert!(!onboarding_get_started_enabled(
      true,
      crate::platform::PermissionStatus::Granted,
      crate::platform::PermissionStatus::NotDetermined,
    ));
    assert!(!onboarding_get_started_enabled(
      false,
      crate::platform::PermissionStatus::Granted,
      crate::platform::PermissionStatus::Granted,
    ));
  }

  #[test]
  fn download_events_are_ignored_after_cancel_or_pack_switch() {
    assert!(should_accept_download_event("pack-a", "pack-a", true));
    assert!(!should_accept_download_event("pack-a", "pack-a", false));
    assert!(!should_accept_download_event("pack-a", "pack-b", true));
  }
}
