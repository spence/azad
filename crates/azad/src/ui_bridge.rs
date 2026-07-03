use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::AppEvent;
use crate::settings::{AutoSubmitMode, OverlayPosition, PasteMethod};
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
    UiEventAction::OpenPermission(permission) => open_permission_settings(permission),
    UiEventAction::Ignore => {}
  }
}

enum UiEventAction {
  Send(AppEvent),
  OpenPermission(String),
  Ignore,
}

fn app_event_for_ui_event(event: &UiEvent) -> UiEventAction {
  let bool_value = || event.bool_value.unwrap_or(false);
  let index = || event.index.unwrap_or(0);
  match (event.surface.as_str(), event.action.as_str()) {
    ("onboarding", "getStarted") => UiEventAction::Send(AppEvent::OnboardingGetStarted),
    ("onboarding", "setTrigger") => {
      UiEventAction::Send(AppEvent::OnboardingSetTrigger(index() == 0))
    }
    ("onboarding", "toggleHistory") => {
      UiEventAction::Send(AppEvent::OnboardingToggleHistory(bool_value()))
    }
    ("onboarding", "toggleAppendTrailingSpace") => {
      UiEventAction::Send(AppEvent::OnboardingToggleAppendTrailingSpace(bool_value()))
    }
    ("onboarding", "setOverlayPosition") => UiEventAction::Send(
      AppEvent::OnboardingSetOverlayPosition(OverlayPosition::from_ui_index(index() as i64)),
    ),
    ("onboarding", "toggleLogin") => {
      UiEventAction::Send(AppEvent::OnboardingToggleLogin(bool_value()))
    }
    ("onboarding", "downloadModel") => UiEventAction::Send(AppEvent::OnboardingDownloadModel),
    ("onboarding", "selectDevice") => {
      UiEventAction::Send(AppEvent::OnboardingSelectDevice(index()))
    }
    ("onboarding", "setListenModifier") => {
      UiEventAction::Send(AppEvent::OnboardingSetListenModifier {
        bit: event.bit.unwrap_or(0),
        enabled: bool_value(),
      })
    }
    ("onboarding", "openPermission") | ("settings", "openPermission") => {
      UiEventAction::OpenPermission(event.permission.clone().unwrap_or_default())
    }

    ("settings", "toggleRunOnStartup") => {
      UiEventAction::Send(AppEvent::SettingsToggleRunOnStartup(bool_value()))
    }
    ("settings", "toggleDebugStats") => {
      UiEventAction::Send(AppEvent::SettingsToggleDebugStats(bool_value()))
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
    ("settings", "toggleAppendTrailingSpace") => {
      UiEventAction::Send(AppEvent::SettingsToggleAppendTrailingSpace(bool_value()))
    }
    ("settings", "toggleDeduplicateWords") => {
      UiEventAction::Send(AppEvent::SettingsToggleDeduplicateWords(bool_value()))
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
    ("settings", "cancelDownload") => UiEventAction::Send(AppEvent::SettingsCancelDownload),
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
}
