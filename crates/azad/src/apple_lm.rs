//! Hey Azad on-device language path: availability probe + allowlisted intents.
//!
//! The optional `azad-apple-lm` Swift helper talks to Apple Foundation Models when
//! present. Intent *application* always runs in Rust through the same settings
//! handlers as the Text replacement checkboxes. A deterministic heuristic covers
//! the closed tool catalog when the helper is missing so the real apply path stays
//! testable and usable offline.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Availability of the on-device language model / helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityState {
  Available,
  AppleIntelligenceNotEnabled,
  ModelNotReady,
  DeviceNotEligible,
  Unavailable,
}

impl AvailabilityState {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Available => "available",
      Self::AppleIntelligenceNotEnabled => "appleIntelligenceNotEnabled",
      Self::ModelNotReady => "modelNotReady",
      Self::DeviceNotEligible => "deviceNotEligible",
      Self::Unavailable => "unavailable",
    }
  }

  pub fn from_str_loose(s: &str) -> Self {
    match s {
      "available" => Self::Available,
      "appleIntelligenceNotEnabled" => Self::AppleIntelligenceNotEnabled,
      "modelNotReady" => Self::ModelNotReady,
      "deviceNotEligible" => Self::DeviceNotEligible,
      _ => Self::Unavailable,
    }
  }

  /// Enable checkbox: allow when the model is ready, or when we can still serve
  /// the closed catalog via the local heuristic (helper missing / OS too old).
  pub fn can_enable_connector(self) -> bool {
    !matches!(self, Self::DeviceNotEligible)
  }

  pub fn show_open_settings(self) -> bool {
    matches!(self, Self::AppleIntelligenceNotEnabled | Self::ModelNotReady)
  }

  pub fn message(self) -> &'static str {
    match self {
      Self::Available => "Apple Intelligence is ready.",
      Self::AppleIntelligenceNotEnabled => {
        "Turn on Apple Intelligence in System Settings. Siri can remain off."
      }
      Self::ModelNotReady => {
        "Apple Intelligence is downloading — keep this Mac on Wi‑Fi and power."
      }
      Self::DeviceNotEligible => {
        "This Mac doesn’t support Apple Intelligence. Hey Azad isn’t available here."
      }
      Self::Unavailable => {
        "On-device model not linked yet — voice settings still work for common phrases."
      }
    }
  }

  pub fn setup_overlay_message(self) -> &'static str {
    match self {
      Self::AppleIntelligenceNotEnabled => {
        "Turn on Apple Intelligence in System Settings (Apple Intelligence & Siri). Siri can stay off."
      }
      Self::ModelNotReady => {
        "Apple Intelligence is downloading — keep this Mac on Wi‑Fi and power."
      }
      Self::DeviceNotEligible => {
        "This Mac doesn’t support Apple Intelligence, so voice settings changes aren’t available."
      }
      Self::Unavailable => "Using built-in phrase matching for text-replacement settings.",
      Self::Available => "Ready.",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityReport {
  pub state: AvailabilityState,
  #[serde(default)]
  pub detail: Option<String>,
}

/// Snapshot of Text replacement settings passed into the interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSettingsSnapshot {
  pub trailing_space: bool,
  pub deduplicate_words: bool,
  pub convert_number_words: bool,
  pub convert_spoken_emoji: bool,
  pub lowercase_except_uppercase: bool,
  pub remove_hesitations: bool,
  pub removed_words: Vec<String>,
}

/// Allowlisted settings toggles for Hey Azad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSettingId {
  TrailingSpace,
  DeduplicateWords,
  ConvertNumberWords,
  ConvertSpokenEmoji,
  LowercaseExceptUppercase,
  RemoveHesitations,
}

impl TextSettingId {
  pub fn label(self) -> &'static str {
    match self {
      Self::TrailingSpace => "trailing space after paste",
      Self::DeduplicateWords => "repeated-word removal",
      Self::ConvertNumberWords => "number text replacement",
      Self::ConvertSpokenEmoji => "spoken emoji conversion",
      Self::LowercaseExceptUppercase => "lowercase (except uppercase words)",
      Self::RemoveHesitations => "hesitation removal",
    }
  }

  pub fn parse(s: &str) -> Option<Self> {
    match s {
      "trailing_space" | "trailingSpace" => Some(Self::TrailingSpace),
      "deduplicate_words" | "deduplicateWords" | "repeated_words" => Some(Self::DeduplicateWords),
      "convert_number_words" | "convertNumberWords" | "numbers" | "number_words" => {
        Some(Self::ConvertNumberWords)
      }
      "convert_spoken_emoji" | "convertSpokenEmoji" | "emoji" => Some(Self::ConvertSpokenEmoji),
      "lowercase_except_uppercase" | "lowercaseExceptUppercase" | "casing" | "lowercase" => {
        Some(Self::LowercaseExceptUppercase)
      }
      "remove_hesitations" | "removeHesitations" | "hesitations" => Some(Self::RemoveHesitations),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AzadIntent {
  SetTextSetting { setting: TextSettingId, enabled: bool },
  ManageRemovedWord { action: RemovedWordAction, word: String },
  Unsupported { message: String },
  Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovedWordAction {
  Add,
  Remove,
}

impl AzadIntent {
  /// User-facing confirmation / status line for the overlay.
  pub fn confirmation_label(&self) -> String {
    match self {
      Self::SetTextSetting { setting, enabled } => {
        if *enabled {
          format!("Enabled {}", setting.label())
        } else {
          format!("Disabled {}", setting.label())
        }
      }
      Self::ManageRemovedWord { action: RemovedWordAction::Add, word } => {
        format!("Added “{word}” to removed words")
      }
      Self::ManageRemovedWord { action: RemovedWordAction::Remove, word } => {
        format!("Removed “{word}” from removed words")
      }
      Self::Unsupported { message } => message.clone(),
      Self::Help => {
        "Try: “disable number text replacement”, “turn on emoji”, or “add the word basically”."
          .to_string()
      }
    }
  }

  pub fn is_actionable(&self) -> bool {
    matches!(self, Self::SetTextSetting { .. } | Self::ManageRemovedWord { .. })
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperAvailabilityResponse {
  state: String,
  #[serde(default)]
  detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperInterpretResponse {
  #[serde(default)]
  ok: bool,
  #[serde(default)]
  intent: Option<serde_json::Value>,
  #[serde(default)]
  error: Option<String>,
  #[serde(default)]
  message: Option<String>,
}

/// Locate `azad-apple-lm` next to the app binary, then on PATH.
pub fn resolve_helper_path() -> Option<PathBuf> {
  if let Ok(override_path) = std::env::var("AZAD_APPLE_LM_HELPER") {
    let p = PathBuf::from(override_path);
    if p.is_file() {
      return Some(p);
    }
  }
  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      let candidate = dir.join("azad-apple-lm");
      if candidate.is_file() {
        return Some(candidate);
      }
    }
  }
  which("azad-apple-lm")
}

fn which(name: &str) -> Option<PathBuf> {
  let path = std::env::var_os("PATH")?;
  for dir in std::env::split_paths(&path) {
    let candidate = dir.join(name);
    if candidate.is_file() {
      return Some(candidate);
    }
  }
  None
}

/// Probe helper + Foundation Models availability. Falls back when the helper is missing.
pub fn probe_availability() -> AvailabilityReport {
  let Some(helper) = resolve_helper_path() else {
    return AvailabilityReport {
      state: AvailabilityState::Unavailable,
      detail: Some("azad-apple-lm helper not found".into()),
    };
  };
  match run_helper_json(&helper, &serde_json::json!({"cmd": "availability"})) {
    Ok(value) => parse_availability_response(&value),
    Err(err) => AvailabilityReport { state: AvailabilityState::Unavailable, detail: Some(err) },
  }
}

fn parse_availability_response(value: &serde_json::Value) -> AvailabilityReport {
  match serde_json::from_value::<HelperAvailabilityResponse>(value.clone()) {
    Ok(resp) => AvailabilityReport {
      state: AvailabilityState::from_str_loose(&resp.state),
      detail: resp.detail,
    },
    Err(_) => {
      if let Some(state) = value.get("state").and_then(|s| s.as_str()) {
        AvailabilityReport {
          state: AvailabilityState::from_str_loose(state),
          detail: value.get("detail").and_then(|d| d.as_str()).map(|s| s.to_string()),
        }
      } else {
        AvailabilityReport {
          state: AvailabilityState::Unavailable,
          detail: Some("malformed availability response".into()),
        }
      }
    }
  }
}

/// Interpret a clean query into an allowlisted intent.
///
/// Order: empty → help; helper interpret when available; else heuristic catalog matcher.
pub fn interpret_query(query: &str, snapshot: &TextSettingsSnapshot) -> AzadIntent {
  let trimmed = query.trim();
  if trimmed.is_empty() {
    return AzadIntent::Help;
  }

  if let Some(helper) = resolve_helper_path() {
    let req = serde_json::json!({
      "cmd": "interpret",
      "query": trimmed,
      "context": snapshot,
    });
    if let Ok(value) = run_helper_json(&helper, &req) {
      if let Some(intent) = parse_helper_intent(&value) {
        // Prefer model/helper only when it produced an actionable tool; otherwise fall
        // through to the closed-catalog heuristic so common phrases still work while
        // Apple Intelligence is off or downloading.
        if intent.is_actionable() || matches!(intent, AzadIntent::Help) {
          return intent;
        }
      }
    }
  }

  interpret_query_heuristic(trimmed, snapshot)
}

fn parse_helper_intent(value: &serde_json::Value) -> Option<AzadIntent> {
  let resp: HelperInterpretResponse = serde_json::from_value(value.clone()).ok()?;
  if let Some(err) = resp.error.filter(|e| !e.is_empty()) {
    return Some(AzadIntent::Unsupported { message: err });
  }
  if let Some(intent_val) = resp.intent {
    return parse_intent_value(&intent_val);
  }
  if let Some(message) = resp.message.filter(|m| !m.is_empty()) {
    return Some(AzadIntent::Unsupported { message });
  }
  None
}

/// Parse a JSON intent object (helper output or tests).
pub fn parse_intent_value(value: &serde_json::Value) -> Option<AzadIntent> {
  if let Some(kind) = value.get("kind").and_then(|k| k.as_str()) {
    match kind {
      "set_text_setting" | "setTextSetting" => {
        let setting =
          value.get("setting").and_then(|s| s.as_str()).and_then(TextSettingId::parse)?;
        let enabled = value.get("enabled").and_then(|e| e.as_bool())?;
        return Some(AzadIntent::SetTextSetting { setting, enabled });
      }
      "manage_removed_word" | "manageRemovedWord" => {
        let action = match value.get("action").and_then(|a| a.as_str())? {
          "add" => RemovedWordAction::Add,
          "remove" => RemovedWordAction::Remove,
          _ => return None,
        };
        let word = value
          .get("word")
          .and_then(|w| w.as_str())
          .map(|w| w.trim().to_ascii_lowercase())
          .filter(|w| !w.is_empty())?;
        return Some(AzadIntent::ManageRemovedWord { action, word });
      }
      "unsupported" => {
        let message = value
          .get("message")
          .and_then(|m| m.as_str())
          .unwrap_or("I can only change text-replacement settings right now.")
          .to_string();
        return Some(AzadIntent::Unsupported { message });
      }
      "help" => return Some(AzadIntent::Help),
      _ => {}
    }
  }
  if let Some(action) = value.get("action").and_then(|a| a.as_str()) {
    match action {
      "set" | "toggle" | "set_text_setting" => {
        let setting =
          value.get("setting").and_then(|s| s.as_str()).and_then(TextSettingId::parse)?;
        let enabled = value.get("enabled").and_then(|e| e.as_bool())?;
        return Some(AzadIntent::SetTextSetting { setting, enabled });
      }
      "add_removed_word" | "add" => {
        let word = value
          .get("word")
          .and_then(|w| w.as_str())
          .map(|w| w.trim().to_ascii_lowercase())
          .filter(|w| !w.is_empty())?;
        return Some(AzadIntent::ManageRemovedWord { action: RemovedWordAction::Add, word });
      }
      "remove_removed_word" | "remove" => {
        let word = value
          .get("word")
          .and_then(|w| w.as_str())
          .map(|w| w.trim().to_ascii_lowercase())
          .filter(|w| !w.is_empty())?;
        return Some(AzadIntent::ManageRemovedWord { action: RemovedWordAction::Remove, word });
      }
      "help" => return Some(AzadIntent::Help),
      _ => {}
    }
  }
  None
}

/// Deterministic closed-catalog interpreter for spoken text-replacement commands.
pub fn interpret_query_heuristic(query: &str, _snapshot: &TextSettingsSnapshot) -> AzadIntent {
  let q = normalize_query(query);
  if q.is_empty() {
    return AzadIntent::Help;
  }

  if let Some(intent) = parse_removed_word_command(&q) {
    return intent;
  }

  let enabled = match detect_enablement(&q) {
    Some(v) => v,
    None => {
      return AzadIntent::Unsupported {
        message:
          "Say enable or disable with a text-replacement setting (numbers, emoji, hesitations, …)."
            .into(),
      };
    }
  };

  if let Some(setting) = detect_setting(&q) {
    return AzadIntent::SetTextSetting { setting, enabled };
  }

  AzadIntent::Unsupported {
    message: "I can change text-replacement settings: numbers, emoji, hesitations, trailing space, repeated words, casing, or removed words.".into(),
  }
}

fn normalize_query(query: &str) -> String {
  query
    .chars()
    .map(
      |c| {
        if c.is_ascii_alphanumeric() || c.is_whitespace() { c.to_ascii_lowercase() } else { ' ' }
      },
    )
    .collect::<String>()
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
}

fn detect_enablement(q: &str) -> Option<bool> {
  let disable_markers = ["disable", "turn off", "switch off", "stop", "don't", "do not", "no more"];
  let enable_markers = ["enable", "turn on", "switch on", "start", "use", "activate"];
  for m in disable_markers {
    if q.contains(m) {
      return Some(false);
    }
  }
  for m in enable_markers {
    if q.contains(m) {
      return Some(true);
    }
  }
  None
}

fn detect_setting(q: &str) -> Option<TextSettingId> {
  let checks: &[(&[&str], TextSettingId)] = &[
    (
      &[
        "number text",
        "number replacement",
        "numbers",
        "number words",
        "convert number",
        "automatic number",
      ],
      TextSettingId::ConvertNumberWords,
    ),
    (&["emoji", "spoken emoji"], TextSettingId::ConvertSpokenEmoji),
    (&["hesitation", "um ah", "filler"], TextSettingId::RemoveHesitations),
    (
      &["trailing space", "append space", "space after paste", "trailing spaces"],
      TextSettingId::TrailingSpace,
    ),
    (
      &["repeated word", "duplicate word", "deduplicate", "repeat word"],
      TextSettingId::DeduplicateWords,
    ),
    (&["lowercase", "casing", "case conversion"], TextSettingId::LowercaseExceptUppercase),
  ];
  for (phrases, id) in checks {
    if phrases.iter().any(|p| q.contains(p)) {
      return Some(*id);
    }
  }
  None
}

fn parse_removed_word_command(q: &str) -> Option<AzadIntent> {
  if q.contains("add") && (q.contains("removed") || q.contains("remove list") || q.contains("word"))
  {
    if let Some(word) = extract_word_after(q, &["add the word", "add word", "add"]) {
      if !is_setting_noise_word(&word) {
        return Some(AzadIntent::ManageRemovedWord { action: RemovedWordAction::Add, word });
      }
    }
  }
  if (q.contains("remove") || q.contains("delete") || q.contains("stop removing"))
    && (q.contains("word") || q.contains("removed"))
  {
    if let Some(word) = extract_word_after(
      q,
      &[
        "stop removing",
        "remove the word",
        "delete the word",
        "remove word",
        "delete word",
        "remove",
        "delete",
      ],
    ) {
      if !is_setting_noise_word(&word) {
        return Some(AzadIntent::ManageRemovedWord { action: RemovedWordAction::Remove, word });
      }
    }
  }
  None
}

fn extract_word_after(q: &str, prefixes: &[&str]) -> Option<String> {
  for prefix in prefixes {
    if let Some(rest) = q.find(prefix).map(|i| &q[i + prefix.len()..]) {
      let rest = rest.trim().trim_start_matches("the ").trim_start_matches("word ");
      let token = rest
        .split_whitespace()
        .next()
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_ascii_lowercase())
        .filter(|t| !t.is_empty())?;
      if matches!(
        token.as_str(),
        "to" | "from" | "the" | "a" | "an" | "my" | "removed" | "words" | "list"
      ) {
        continue;
      }
      return Some(token);
    }
  }
  None
}

fn is_setting_noise_word(word: &str) -> bool {
  matches!(
    word,
    "number"
      | "numbers"
      | "emoji"
      | "hesitation"
      | "hesitations"
      | "space"
      | "spaces"
      | "trailing"
      | "repeated"
      | "duplicate"
      | "lowercase"
      | "casing"
      | "text"
      | "replacement"
      | "automatic"
  )
}

fn run_helper_json(
  helper: &Path,
  request: &serde_json::Value,
) -> Result<serde_json::Value, String> {
  let mut child = Command::new(helper)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|e| format!("spawn helper: {e}"))?;

  {
    let stdin = child.stdin.as_mut().ok_or_else(|| "helper stdin missing".to_string())?;
    let line = format!("{request}\n");
    stdin.write_all(line.as_bytes()).map_err(|e| format!("write helper: {e}"))?;
  }
  drop(child.stdin.take());

  let stdout = child.stdout.take().ok_or_else(|| "helper stdout missing".to_string())?;
  let mut reader = BufReader::new(stdout);
  let mut line = String::new();
  reader.read_line(&mut line).map_err(|e| format!("read helper: {e}"))?;

  let _ = wait_timeout(&mut child, Duration::from_secs(30));

  let line = line.trim();
  if line.is_empty() {
    return Err("helper returned empty response".into());
  }
  serde_json::from_str(line).map_err(|e| format!("helper json: {e}"))
}

fn wait_timeout(
  child: &mut std::process::Child,
  timeout: Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
  let start = Instant::now();
  loop {
    if let Some(status) = child.try_wait()? {
      return Ok(Some(status));
    }
    if start.elapsed() >= timeout {
      let _ = child.kill();
      let _ = child.wait();
      return Ok(None);
    }
    std::thread::sleep(Duration::from_millis(20));
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn empty_snapshot() -> TextSettingsSnapshot {
    TextSettingsSnapshot {
      trailing_space: false,
      deduplicate_words: false,
      convert_number_words: true,
      convert_spoken_emoji: false,
      lowercase_except_uppercase: false,
      remove_hesitations: true,
      removed_words: vec![],
    }
  }

  #[test]
  fn heuristic_disables_number_replacement() {
    let intent = interpret_query_heuristic(
      "I want you to disable the automatic number text replacement",
      &empty_snapshot(),
    );
    assert_eq!(
      intent,
      AzadIntent::SetTextSetting { setting: TextSettingId::ConvertNumberWords, enabled: false }
    );
    assert_eq!(intent.confirmation_label(), "Disabled number text replacement");
  }

  #[test]
  fn heuristic_enables_emoji() {
    let intent = interpret_query_heuristic("turn on spoken emoji", &empty_snapshot());
    assert_eq!(
      intent,
      AzadIntent::SetTextSetting { setting: TextSettingId::ConvertSpokenEmoji, enabled: true }
    );
  }

  #[test]
  fn heuristic_disables_hesitations() {
    let intent = interpret_query_heuristic("disable hesitations", &empty_snapshot());
    assert_eq!(
      intent,
      AzadIntent::SetTextSetting { setting: TextSettingId::RemoveHesitations, enabled: false }
    );
  }

  #[test]
  fn heuristic_trailing_space() {
    let intent = interpret_query_heuristic("enable trailing space after paste", &empty_snapshot());
    assert_eq!(
      intent,
      AzadIntent::SetTextSetting { setting: TextSettingId::TrailingSpace, enabled: true }
    );
  }

  #[test]
  fn heuristic_add_removed_word() {
    let intent =
      interpret_query_heuristic("add the word basically to removed words", &empty_snapshot());
    assert_eq!(
      intent,
      AzadIntent::ManageRemovedWord { action: RemovedWordAction::Add, word: "basically".into() }
    );
  }

  #[test]
  fn heuristic_remove_removed_word() {
    let intent =
      interpret_query_heuristic("remove the word basically from removed words", &empty_snapshot());
    assert_eq!(
      intent,
      AzadIntent::ManageRemovedWord { action: RemovedWordAction::Remove, word: "basically".into() }
    );
  }

  #[test]
  fn empty_query_is_help() {
    assert_eq!(interpret_query("", &empty_snapshot()), AzadIntent::Help);
  }

  #[test]
  fn parse_intent_value_set_text_setting() {
    let v = serde_json::json!({
      "kind": "set_text_setting",
      "setting": "convert_number_words",
      "enabled": false
    });
    assert_eq!(
      parse_intent_value(&v),
      Some(AzadIntent::SetTextSetting {
        setting: TextSettingId::ConvertNumberWords,
        enabled: false,
      })
    );
  }

  #[test]
  fn availability_state_gates() {
    assert!(AvailabilityState::Available.can_enable_connector());
    assert!(AvailabilityState::Unavailable.can_enable_connector());
    assert!(!AvailabilityState::DeviceNotEligible.can_enable_connector());
    assert!(AvailabilityState::AppleIntelligenceNotEnabled.show_open_settings());
    assert!(!AvailabilityState::DeviceNotEligible.show_open_settings());
  }

  #[test]
  fn interpret_query_uses_heuristic_without_helper() {
    // SAFETY: test-only; clears optional helper override so we exercise the real entry.
    unsafe { std::env::remove_var("AZAD_APPLE_LM_HELPER") };
    let intent =
      interpret_query("disable the automatic number text replacement", &empty_snapshot());
    assert_eq!(
      intent,
      AzadIntent::SetTextSetting { setting: TextSettingId::ConvertNumberWords, enabled: false }
    );
  }
}
