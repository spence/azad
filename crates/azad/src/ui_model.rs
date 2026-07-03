use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsTab {
  #[default]
  General,
  Models,
  Permissions,
  Debug,
  Connectors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UiPermissionStatus {
  Granted,
  NotGranted,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UiModelStatus {
  NotDownloaded,
  Downloading,
  Ready,
  Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiModelPack {
  pub id: String,
  pub welcome_name: String,
  pub settings_name: String,
  pub description: String,
  pub size_label: String,
  pub status: UiModelStatus,
  pub progress_pct: u8,
  pub bytes_done_label: String,
  pub bytes_total_label: String,
  pub error_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiDeviceOption {
  pub id: String,
  pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingViewModel {
  pub always_listening_enabled: bool,
  pub history_enabled: bool,
  pub paste_method_index: i64,
  pub append_trailing_space_on_paste: bool,
  pub overlay_position_index: i64,
  pub run_on_startup_enabled: bool,
  pub accessibility_status: UiPermissionStatus,
  pub microphone_status: UiPermissionStatus,
  pub model: UiModelPack,
  pub get_started_enabled: bool,
  pub devices: Vec<UiDeviceOption>,
  pub selected_device_index: Option<usize>,
  pub listen_modifiers: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorRowVM {
  pub display_name: String,
  pub trigger: String,
  pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsViewModel {
  pub selected_tab: SettingsTab,
  pub accessibility_status: UiPermissionStatus,
  pub microphone_status: UiPermissionStatus,
  pub run_on_startup_enabled: bool,
  pub paste_method_index: i64,
  pub auto_submit_index: i64,
  pub overlay_position_index: i64,
  pub append_trailing_space_on_paste: bool,
  pub deduplicate_words_on_paste: bool,
  pub listen_modifiers: u8,
  pub debug_stats_enabled: bool,
  pub metrics_text: String,
  pub model: UiModelPack,
  pub removed_words: Vec<String>,
  pub connectors: Vec<ConnectorRowVM>,
  pub build_info: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPermissionUpdate {
  pub accessibility_status: UiPermissionStatus,
  pub microphone_status: UiPermissionStatus,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiEvent {
  pub surface: String,
  pub action: String,
  #[serde(default)]
  pub bool_value: Option<bool>,
  #[serde(default)]
  pub index: Option<usize>,
  #[serde(default)]
  pub bit: Option<u8>,
  #[serde(default)]
  pub value: Option<String>,
  #[serde(default)]
  pub pack_id: Option<String>,
  #[serde(default)]
  pub permission: Option<String>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn onboarding_payload_uses_camel_case_state() {
    let json = serde_json::to_string(&OnboardingViewModel {
      always_listening_enabled: true,
      history_enabled: true,
      paste_method_index: 0,
      append_trailing_space_on_paste: false,
      overlay_position_index: 0,
      run_on_startup_enabled: false,
      accessibility_status: UiPermissionStatus::NotGranted,
      microphone_status: UiPermissionStatus::Granted,
      model: UiModelPack {
        id: "nemotron".to_string(),
        welcome_name: "Nemotron-3.5 ASR Streaming".to_string(),
        settings_name: "NVIDIA Nemotron-3.5 ASR Streaming 0.6B".to_string(),
        description: "On-device streaming speech-to-text · English".to_string(),
        size_label: "1.2 GB".to_string(),
        status: UiModelStatus::Downloading,
        progress_pct: 51,
        bytes_done_label: "612 MB".to_string(),
        bytes_total_label: "1.2 GB".to_string(),
        error_message: String::new(),
      },
      get_started_enabled: true,
      devices: vec![UiDeviceOption {
        id: "default".to_string(),
        label: "MacBook Pro Microphone".to_string(),
      }],
      selected_device_index: Some(0),
      listen_modifiers: 4,
    })
    .unwrap();

    assert!(json.contains("\"alwaysListeningEnabled\":true"));
    assert!(json.contains("\"accessibilityStatus\":\"notGranted\""));
    assert!(json.contains("\"status\":\"downloading\""));
  }

  #[test]
  fn ui_event_decodes_optional_fields() {
    let event: UiEvent = serde_json::from_str(
      r#"{"surface":"settings","action":"setListenModifier","bit":4,"boolValue":false}"#,
    )
    .unwrap();
    assert_eq!(event.surface, "settings");
    assert_eq!(event.action, "setListenModifier");
    assert_eq!(event.bit, Some(4));
    assert_eq!(event.bool_value, Some(false));
  }
}
