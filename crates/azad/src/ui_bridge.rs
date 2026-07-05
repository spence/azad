use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::AppEvent;
use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod, StartupListenMode};
use crate::ui_model::{
  OnboardingViewModel, SettingsPermissionUpdate, SettingsViewModel, UiEvent, UiPermissionStatus,
};

type RegisterCallbackFn = unsafe extern "C" fn(Option<unsafe extern "C" fn(*const c_char)>);
type JsonFn = unsafe extern "C" fn(*const c_char) -> i32;
type VoidFn = unsafe extern "C" fn() -> i32;
type SyncModifiersFn = unsafe extern "C" fn(u8) -> i32;

struct AzadUiLibrary {
  _handle: *mut c_void,
  register_callback: RegisterCallbackFn,
  show_onboarding: JsonFn,
  update_onboarding: JsonFn,
  close_onboarding: VoidFn,
  show_settings: JsonFn,
  update_settings: JsonFn,
  refresh_settings_permissions: JsonFn,
  sync_listen_modifiers: SyncModifiersFn,
}

unsafe impl Send for AzadUiLibrary {}
unsafe impl Sync for AzadUiLibrary {}

static UI_LIBRARY: OnceLock<Result<AzadUiLibrary, String>> = OnceLock::new();
static SETTINGS_WINDOW_OPEN: AtomicBool = AtomicBool::new(false);

pub fn show_onboarding_window(model: &OnboardingViewModel) {
  with_library(|lib| call_json(lib.show_onboarding, model, "show_onboarding"));
}

pub fn update_onboarding_window(model: &OnboardingViewModel) {
  with_library(|lib| call_json(lib.update_onboarding, model, "update_onboarding"));
}

pub fn close_onboarding_window() {
  with_library(|lib| unsafe {
    if (lib.close_onboarding)() == 0 {
      eprintln!("Azad UI: close_onboarding failed");
    }
  });
}

pub fn show_settings_window(model: &SettingsViewModel) {
  SETTINGS_WINDOW_OPEN.store(true, Ordering::Release);
  with_library(|lib| call_json(lib.show_settings, model, "show_settings"));
}

pub fn update_settings_window(model: &SettingsViewModel) {
  with_library(|lib| call_json(lib.update_settings, model, "update_settings"));
}

pub fn settings_window_is_open() -> bool {
  SETTINGS_WINDOW_OPEN.load(Ordering::Acquire)
}

pub fn refresh_settings_permissions(
  accessibility_status: UiPermissionStatus,
  microphone_status: UiPermissionStatus,
) {
  if !settings_window_is_open() {
    return;
  }
  let update = SettingsPermissionUpdate { accessibility_status, microphone_status };
  with_library(|lib| call_json(lib.refresh_settings_permissions, &update, "refresh_permissions"));
}

pub fn sync_listen_modifiers(mask: u8) {
  with_library(|lib| unsafe {
    if (lib.sync_listen_modifiers)(mask) == 0 {
      eprintln!("Azad UI: sync_listen_modifiers failed");
    }
  });
}

fn with_library(f: impl FnOnce(&AzadUiLibrary)) {
  match UI_LIBRARY.get_or_init(load_library) {
    Ok(lib) => f(lib),
    Err(err) => eprintln!("Azad UI unavailable: {err}"),
  }
}

fn call_json<T: serde::Serialize>(func: JsonFn, model: &T, name: &str) {
  let json = match serde_json::to_string(model) {
    Ok(json) => json,
    Err(err) => {
      eprintln!("Azad UI: failed to serialize {name} payload: {err}");
      return;
    }
  };
  let c_json = match CString::new(json) {
    Ok(c_json) => c_json,
    Err(err) => {
      eprintln!("Azad UI: {name} payload contained an interior NUL: {err}");
      return;
    }
  };
  unsafe {
    if func(c_json.as_ptr()) == 0 {
      eprintln!("Azad UI: {name} failed");
    }
  }
}

fn load_library() -> Result<AzadUiLibrary, String> {
  let mut errors = Vec::new();
  for path in candidate_library_paths() {
    let c_path = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
      errors.push(format!("{}: {}", path.display(), dlerror()));
      continue;
    }

    unsafe {
      let lib = AzadUiLibrary {
        _handle: handle,
        register_callback: symbol(handle, "azad_ui_register_callback")?,
        show_onboarding: symbol(handle, "azad_ui_show_onboarding")?,
        update_onboarding: symbol(handle, "azad_ui_update_onboarding")?,
        close_onboarding: symbol(handle, "azad_ui_close_onboarding")?,
        show_settings: symbol(handle, "azad_ui_show_settings")?,
        update_settings: symbol(handle, "azad_ui_update_settings")?,
        refresh_settings_permissions: symbol(handle, "azad_ui_refresh_settings_permissions")?,
        sync_listen_modifiers: symbol(handle, "azad_ui_sync_listen_modifiers")?,
      };
      (lib.register_callback)(Some(ui_event_callback));
      return Ok(lib);
    }
  }

  Err(errors.join("; "))
}

unsafe fn symbol<T: Copy>(handle: *mut c_void, name: &str) -> Result<T, String> {
  let c_name = CString::new(name).map_err(|e| e.to_string())?;
  let sym = unsafe { libc::dlsym(handle, c_name.as_ptr()) };
  if sym.is_null() {
    Err(format!("{name}: {}", dlerror()))
  } else {
    Ok(unsafe { std::mem::transmute_copy(&sym) })
  }
}

fn candidate_library_paths() -> Vec<PathBuf> {
  let mut paths = Vec::new();
  if let Some(path) = std::env::var_os("AZAD_UI_LIB_PATH") {
    paths.push(PathBuf::from(path));
  }
  if let Ok(exe) = std::env::current_exe()
    && let Some(dir) = exe.parent()
  {
    paths.push(dir.join("libAzadUI.dylib"));
  }
  paths.push(
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
      .join("../..")
      .join("target/swift/azad-ui/release/libAzadUI.dylib"),
  );
  paths
}

fn dlerror() -> String {
  unsafe {
    let err = libc::dlerror();
    if err.is_null() {
      "unknown dlopen error".to_string()
    } else {
      CStr::from_ptr(err).to_string_lossy().into_owned()
    }
  }
}

unsafe extern "C" fn ui_event_callback(json: *const c_char) {
  if json.is_null() {
    return;
  }
  let Ok(text) = (unsafe { CStr::from_ptr(json) }).to_str() else {
    eprintln!("Azad UI: event callback received invalid UTF-8");
    return;
  };
  let event: UiEvent = match serde_json::from_str(text) {
    Ok(event) => event,
    Err(err) => {
      eprintln!("Azad UI: failed to decode event {text:?}: {err}");
      return;
    }
  };
  handle_ui_event(event);
}

fn handle_ui_event(event: UiEvent) {
  match app_event_for_ui_event(&event) {
    UiEventAction::Send(app_event) => {
      crate::app::send_event(app_event);
    }
    UiEventAction::SetDownloadPausedImmediate(paused) => {
      if !crate::app::set_download_paused_immediate(paused) {
        crate::app::send_event(AppEvent::SettingsSetDownloadPaused(paused));
      }
    }
    UiEventAction::OpenPermission(permission) => open_permission_settings(permission),
    UiEventAction::Ignore => {}
  }
}

enum UiEventAction {
  Send(AppEvent),
  SetDownloadPausedImmediate(bool),
  OpenPermission(String),
  Ignore,
}

fn app_event_for_ui_event(event: &UiEvent) -> UiEventAction {
  let bool_value = || event.bool_value.unwrap_or(false);
  let index = || event.index.unwrap_or(0);
  match (event.surface.as_str(), event.action.as_str()) {
    ("app", "quit") => UiEventAction::Send(AppEvent::ShutdownRequested),
    ("onboarding", "getStarted") => UiEventAction::Send(AppEvent::OnboardingGetStarted),
    ("onboarding", "downloadModel") => UiEventAction::Send(AppEvent::OnboardingDownloadModel),
    ("onboarding", "pauseDownload") => UiEventAction::SetDownloadPausedImmediate(true),
    ("onboarding", "resumeDownload") => UiEventAction::SetDownloadPausedImmediate(false),
    ("onboarding", "setListenModifier") => {
      UiEventAction::Send(AppEvent::OnboardingSetListenModifier {
        bit: event.bit.unwrap_or(0),
        enabled: bool_value(),
      })
    }
    ("onboarding", "openPermission") | ("settings", "openPermission") => {
      UiEventAction::OpenPermission(event.permission.clone().unwrap_or_default())
    }
    ("onboarding", "requestPermission") | ("settings", "requestPermission") => {
      UiEventAction::Send(AppEvent::RequestPermission(event.permission.clone().unwrap_or_default()))
    }

    ("settings", "toggleRunOnStartup") => {
      UiEventAction::Send(AppEvent::SettingsToggleRunOnStartup(bool_value()))
    }
    ("settings", "selectStartupListenMode") => UiEventAction::Send(
      AppEvent::SettingsSelectStartupListenMode(StartupListenMode::from_ui_index(index() as i64)),
    ),
    ("settings", "toggleDebugStats") => {
      UiEventAction::Send(AppEvent::SettingsToggleDebugStats(bool_value()))
    }
    ("settings", "setActivationLevel") => {
      UiEventAction::Send(AppEvent::SettingsSetActivationLevel(index() as i64))
    }
    ("settings", "selectPasteMethod") => UiEventAction::Send(AppEvent::SettingsSelectPasteMethod(
      PasteMethod::from_ui_index(index() as i64),
    )),
    ("settings", "selectAutoSubmit") => UiEventAction::Send(AppEvent::SettingsSelectAutoSubmit(
      AutoSubmitMode::from_ui_index(index() as i64),
    )),
    ("settings", "selectOverlayPosition") => UiEventAction::Send(
      AppEvent::SettingsSelectOverlayPosition(OverlayPosition::from_ui_index(index() as i64)),
    ),
    ("settings", "toggleHistory") => {
      UiEventAction::Send(AppEvent::SettingsToggleHistory(bool_value()))
    }
    ("settings", "toggleAppendTrailingSpace") => {
      UiEventAction::Send(AppEvent::SettingsToggleAppendTrailingSpace(bool_value()))
    }
    ("settings", "toggleDeduplicateWords") => {
      UiEventAction::Send(AppEvent::SettingsToggleDeduplicateWords(bool_value()))
    }
    ("settings", "toggleConvertNumberWords") => {
      UiEventAction::Send(AppEvent::SettingsToggleConvertNumberWords(bool_value()))
    }
    ("settings", "toggleConvertSpokenEmoji") => {
      UiEventAction::Send(AppEvent::SettingsToggleConvertSpokenEmoji(bool_value()))
    }
    ("settings", "toggleLowercaseExceptUppercaseWords") => {
      UiEventAction::Send(AppEvent::SettingsToggleLowercaseExceptUppercaseWords(bool_value()))
    }
    ("settings", "toggleRemoveHesitations") => {
      UiEventAction::Send(AppEvent::SettingsToggleRemoveHesitations(bool_value()))
    }
    ("settings", "setListenModifier") => UiEventAction::Send(AppEvent::SettingsSetListenModifier {
      bit: event.bit.unwrap_or(0),
      enabled: bool_value(),
    }),
    ("settings", "toggleConnector") => UiEventAction::Send(AppEvent::SettingsToggleConnector {
      index: index(),
      enabled: bool_value(),
    }),
    ("settings", "addRemovedWord") => {
      UiEventAction::Send(AppEvent::SettingsAddRemovedWord(event.value.clone().unwrap_or_default()))
    }
    ("settings", "removeRemovedWord") => UiEventAction::Send(AppEvent::SettingsRemoveRemovedWord(
      event.value.clone().unwrap_or_default(),
    )),
    ("settings", "refresh") => UiEventAction::Send(AppEvent::SettingsRefresh),
    ("settings", "downloadModel") => UiEventAction::Send(AppEvent::SettingsDownloadModel(
      event.pack_id.clone().unwrap_or_default(),
    )),
    ("settings", "pauseDownload") => UiEventAction::SetDownloadPausedImmediate(true),
    ("settings", "resumeDownload") => UiEventAction::SetDownloadPausedImmediate(false),
    ("settings", "windowClosed") => {
      SETTINGS_WINDOW_OPEN.store(false, Ordering::Release);
      UiEventAction::Ignore
    }
    _ => UiEventAction::Ignore,
  }
}

fn open_permission_settings(permission: String) {
  let anchor = match permission.as_str() {
    "accessibility" => "Privacy_Accessibility",
    "microphone" => "Privacy_Microphone",
    _ => return,
  };
  let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
  if let Err(err) = Command::new("/usr/bin/open").arg(url).spawn() {
    eprintln!("Azad UI: failed to open System Settings: {err}");
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn test_event(surface: &str, action: &str) -> UiEvent {
    UiEvent {
      surface: surface.to_string(),
      action: action.to_string(),
      bool_value: Some(true),
      index: Some(1),
      bit: Some(4),
      value: Some("um".to_string()),
      pack_id: Some("nemotron-3.5-asr-streaming-0.6b".to_string()),
      permission: Some("microphone".to_string()),
    }
  }

  #[test]
  fn known_welcome_and_settings_controller_events_are_mapped() {
    let actions = [
      ("onboarding", "getStarted"),
      ("onboarding", "downloadModel"),
      ("onboarding", "pauseDownload"),
      ("onboarding", "resumeDownload"),
      ("onboarding", "setListenModifier"),
      ("onboarding", "requestPermission"),
      ("settings", "toggleRunOnStartup"),
      ("settings", "selectStartupListenMode"),
      ("settings", "toggleDebugStats"),
      ("settings", "setActivationLevel"),
      ("settings", "selectPasteMethod"),
      ("settings", "selectAutoSubmit"),
      ("settings", "selectOverlayPosition"),
      ("settings", "toggleHistory"),
      ("settings", "toggleAppendTrailingSpace"),
      ("settings", "toggleDeduplicateWords"),
      ("settings", "toggleConvertNumberWords"),
      ("settings", "toggleConvertSpokenEmoji"),
      ("settings", "toggleLowercaseExceptUppercaseWords"),
      ("settings", "toggleRemoveHesitations"),
      ("settings", "setListenModifier"),
      ("settings", "toggleConnector"),
      ("settings", "addRemovedWord"),
      ("settings", "removeRemovedWord"),
      ("settings", "refresh"),
      ("settings", "downloadModel"),
      ("settings", "pauseDownload"),
      ("settings", "resumeDownload"),
      ("settings", "requestPermission"),
      ("app", "quit"),
    ];

    for (surface, action) in actions {
      if matches!(app_event_for_ui_event(&test_event(surface, action)), UiEventAction::Ignore) {
        panic!("{surface}/{action} was ignored");
      }
    }
  }

  #[test]
  fn settings_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "selectPasteMethod".to_string(),
      bool_value: None,
      index: Some(1),
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsSelectPasteMethod(PasteMethod::DirectTyping)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_startup_listen_mode_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "selectStartupListenMode".to_string(),
      bool_value: None,
      index: Some(2),
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsSelectStartupListenMode(
        StartupListenMode::RestoreLast,
      )) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_history_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "toggleHistory".to_string(),
      bool_value: Some(false),
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsToggleHistory(false)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_deduplicate_words_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "toggleDeduplicateWords".to_string(),
      bool_value: Some(false),
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsToggleDeduplicateWords(false)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_convert_number_words_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "toggleConvertNumberWords".to_string(),
      bool_value: Some(true),
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsToggleConvertNumberWords(true)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_convert_spoken_emoji_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "toggleConvertSpokenEmoji".to_string(),
      bool_value: Some(true),
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsToggleConvertSpokenEmoji(true)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_activation_level_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "setActivationLevel".to_string(),
      bool_value: None,
      index: Some(42),
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsSetActivationLevel(42)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_lowercase_except_uppercase_words_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "toggleLowercaseExceptUppercaseWords".to_string(),
      bool_value: Some(true),
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsToggleLowercaseExceptUppercaseWords(true)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_remove_hesitations_event_maps_to_app_event() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "toggleRemoveHesitations".to_string(),
      bool_value: Some(false),
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::SettingsToggleRemoveHesitations(false)) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn app_quit_event_maps_to_shutdown_request() {
    let event = UiEvent {
      surface: "app".to_string(),
      action: "quit".to_string(),
      bool_value: None,
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::ShutdownRequested) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn permission_event_stays_out_of_controller_queue() {
    let event = UiEvent {
      surface: "onboarding".to_string(),
      action: "openPermission".to_string(),
      bool_value: None,
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: Some("microphone".to_string()),
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::OpenPermission(permission) => assert_eq!(permission, "microphone"),
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn request_permission_event_goes_to_controller_queue() {
    let event = UiEvent {
      surface: "onboarding".to_string(),
      action: "requestPermission".to_string(),
      bool_value: None,
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: Some("accessibility".to_string()),
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::Send(AppEvent::RequestPermission(permission)) => {
        assert_eq!(permission, "accessibility")
      }
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn onboarding_pause_download_event_uses_immediate_download_control() {
    let event = UiEvent {
      surface: "onboarding".to_string(),
      action: "pauseDownload".to_string(),
      bool_value: None,
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::SetDownloadPausedImmediate(true) => {}
      _ => panic!("unexpected event mapping"),
    }
  }

  #[test]
  fn settings_resume_download_event_uses_immediate_download_control() {
    let event = UiEvent {
      surface: "settings".to_string(),
      action: "resumeDownload".to_string(),
      bool_value: None,
      index: None,
      bit: None,
      value: None,
      pack_id: None,
      permission: None,
    };
    match app_event_for_ui_event(&event) {
      UiEventAction::SetDownloadPausedImmediate(false) => {}
      _ => panic!("unexpected event mapping"),
    }
  }
}
