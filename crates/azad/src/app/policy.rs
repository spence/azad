use std::time::{Duration, Instant};

use asr::pipeline::EngineState;

use crate::platform;

use super::{SESSION_DEGRADED_THRESHOLD, SESSION_IMMEDIATE_RETRY_LIMIT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RawFinalizeUiPlan {
  pub(super) hide_overlay: bool,
  pub(super) disable_capture: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ManualHoldReleasePlan {
  pub(super) capture_enabled: bool,
  pub(super) action: ManualHoldReleaseAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManualHoldReleaseAction {
  KeepLive,
  HideOverlay,
  FinalizeTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionRecoveryState {
  Healthy,
  Recovering,
  Degraded,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ListenToggleNotice {
  pub(super) enabled: bool,
  pub(super) started_at: Instant,
  pub(super) duration: Duration,
}

pub(super) fn raw_finalize_ui_plan(
  always_listening_enabled: bool,
  manual_hold_active: bool,
  forced_by_finalize_hotkey: bool,
) -> RawFinalizeUiPlan {
  RawFinalizeUiPlan {
    hide_overlay: forced_by_finalize_hotkey || !manual_hold_active,
    disable_capture: !always_listening_enabled && !manual_hold_active,
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DraftOverlayAction {
  /// Show the overlay now and clear the pending flag.
  Show,
  /// Cancel-suppression is still active. Leave the pending flag as-is so a later
  /// DraftUpdate past the window can still bring the overlay up.
  KeepPendingForLater,
  /// Nothing to show (overlay already visible, or never pending). Clear the flag.
  Clear,
}

/// Decision predicate for the `SpeechEvent::TurnStarted` handler arm. Returns
/// true when the renderer should arm `overlay_pending_vad_text` so the next
/// non-empty `DraftUpdated` brings the live overlay up.
///
/// VAD-driven turns are handled fully by `SpeechStartedByVad` (which arms the
/// flag itself with full side effects). Manual holds normally show the overlay
/// synchronously; an engine-side `Manual` start arms first-text reveal only as
/// a recovery path when that overlay is unexpectedly hidden.
pub(super) fn turn_started_should_arm_pending(
  reason: asr::render::TurnStartedReason,
  overlay_visible: bool,
) -> bool {
  matches!(reason, asr::render::TurnStartedReason::Manual) && !overlay_visible
}

/// Decide what to do with the overlay when a non-empty `DraftUpdated` arrives.
///
/// `pending` is the armed `overlay_pending_vad_text` latch. `eligible_to_show` is a
/// *fresh* recomputation of "this turn is one we should surface live text for" —
/// always-listening-with-overlay-on-start, or manual hold, and not history-browsing.
/// We show when the overlay is hidden and EITHER the latch is armed OR the turn is
/// eligible. The `eligible_to_show` arm makes this self-healing: the latch can be lost
/// by any of ~14 clear sites (notice teardown, turn resets, session rebuild, …) between
/// turn-start and the first draft, but as long as we have real transcribed text for an
/// eligible turn we still bring the overlay up. This is what closes the recurring
/// "no overlay during streaming, flash at the end" class of bug rather than chasing each
/// trigger that drops the latch.
///
/// `cancel_suppression_active` still wins: after Escape we suppress the show for
/// `CANCEL_VAD_SHOW_SUPPRESSION_MS` and keep the latch intact (a later DraftUpdate past
/// the window re-evaluates), so a quick Escape-then-talk doesn't bounce the overlay back.
pub(super) fn draft_update_overlay_action(
  pending: bool,
  overlay_visible: bool,
  cancel_suppression_active: bool,
  eligible_to_show: bool,
) -> DraftOverlayAction {
  if cancel_suppression_active {
    DraftOverlayAction::KeepPendingForLater
  } else if !overlay_visible && (pending || eligible_to_show) {
    DraftOverlayAction::Show
  } else {
    DraftOverlayAction::Clear
  }
}

pub(super) fn manual_hold_release_plan(
  always_listening_enabled: bool,
  should_finalize: bool,
  has_started_turn: bool,
) -> ManualHoldReleasePlan {
  let action = if should_finalize {
    if has_started_turn {
      ManualHoldReleaseAction::FinalizeTurn
    } else {
      ManualHoldReleaseAction::HideOverlay
    }
  } else {
    ManualHoldReleaseAction::KeepLive
  };

  let capture_enabled =
    always_listening_enabled || matches!(action, ManualHoldReleaseAction::FinalizeTurn);

  ManualHoldReleasePlan { capture_enabled, action }
}

pub(super) fn should_latch_raw_on_hold_release(
  raw_requested: bool,
  action: ManualHoldReleaseAction,
) -> bool {
  raw_requested && matches!(action, ManualHoldReleaseAction::FinalizeTurn)
}

pub(super) fn should_ignore_finalizing_event(
  raw_handled_turn_id: Option<u64>,
  turn_id: u64,
) -> bool {
  raw_handled_turn_id == Some(turn_id)
}

pub(super) fn split_overlay_active_for_turns(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
) -> bool {
  finalizing_turn_id
    .zip(current_turn_id)
    .is_some_and(|(finalizing, current)| current > finalizing)
}

pub(super) fn split_overlay_visible_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
) -> bool {
  split_overlay_active_for_turns(finalizing_turn_id, current_turn_id)
    && !live_draft.trim().is_empty()
}

pub(super) fn split_overlay_visible_with_hold_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
  hold_active: bool,
) -> bool {
  split_overlay_visible_for_state(finalizing_turn_id, current_turn_id, live_draft)
    || (hold_active && !live_draft.trim().is_empty())
}

pub(super) fn split_overlay_visible_with_vad_hint_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
  hold_active: bool,
  saw_vad_start_during_finalizing: bool,
  finalizing_draft: &str,
) -> bool {
  // The VAD-hint branch is purely a carryover from a prior turn — it must NOT fire
  // when the live and finalizing drafts hold the same text, otherwise the renderer
  // paints two overlays with identical content (one busy, one idle). The genuine
  // turn-advance and hold paths still run via `_with_hold_for_state` above, which
  // gates on `current > finalizing` or `manual_hold_active` — neither of which can
  // produce a duplicate-text render.
  split_overlay_visible_with_hold_for_state(
    finalizing_turn_id,
    current_turn_id,
    live_draft,
    hold_active,
  ) || (finalizing_turn_id.is_some()
    && saw_vad_start_during_finalizing
    && !live_draft.trim().is_empty()
    && !draft_matches_finalized_text(live_draft, finalizing_draft))
}

pub(super) fn draft_matches_finalized_text(live_draft: &str, finalized_text: &str) -> bool {
  let live_tokens = live_draft
    .split_whitespace()
    .map(|token| token.to_ascii_lowercase())
    .collect::<Vec<_>>();
  let final_tokens = finalized_text
    .split_whitespace()
    .map(|token| token.to_ascii_lowercase())
    .collect::<Vec<_>>();

  if live_tokens.is_empty() || final_tokens.is_empty() {
    return false;
  }
  if live_tokens == final_tokens {
    return true;
  }

  let min_len = live_tokens.len().min(final_tokens.len());
  let max_len = live_tokens.len().max(final_tokens.len());
  let lcp = live_tokens.iter().zip(final_tokens.iter()).take_while(|(a, b)| a == b).count();

  if lcp == min_len {
    // One side is a strict token-prefix of the other.
    return true;
  }

  // Treat near-identical beginnings as the same finalized lane.
  // This prevents VAD-hint-only split mode from getting stuck on replayed
  // same-turn drafts that differ only by minor tail edits/punctuation.
  lcp * 100 >= min_len * 85 && (max_len - min_len) <= 2
}

pub(super) fn split_overlay_visible_with_live_divergence_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
  finalizing_draft: &str,
) -> bool {
  split_overlay_active_for_turns(finalizing_turn_id, current_turn_id)
    && !live_draft.trim().is_empty()
    && !finalizing_draft.trim().is_empty()
    && !draft_matches_finalized_text(live_draft, finalizing_draft)
}

pub(super) fn split_top_completion_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  live_draft: &str,
  hold_active: bool,
  saw_vad_start_during_finalizing: bool,
  finalized_turn_id: u64,
  finalized_text: &str,
) -> bool {
  let live_draft = live_draft.trim();
  if live_draft.is_empty() || finalizing_turn_id != Some(finalized_turn_id) {
    return false;
  }

  if split_overlay_active_for_turns(finalizing_turn_id, current_turn_id) {
    return true;
  }

  if hold_active {
    return true;
  }

  if saw_vad_start_during_finalizing {
    return !draft_matches_finalized_text(live_draft, finalized_text);
  }

  false
}

pub(super) fn raw_finalize_target_turn_id_for_state(
  finalizing_turn_id: Option<u64>,
  current_turn_id: Option<u64>,
  latest_seen_turn_id: u64,
  live_draft: &str,
) -> Option<u64> {
  if split_overlay_visible_for_state(finalizing_turn_id, current_turn_id, live_draft) {
    current_turn_id
  } else {
    finalizing_turn_id
      .or(current_turn_id)
      .or_else(|| (latest_seen_turn_id > 0).then_some(latest_seen_turn_id))
  }
}

pub(super) fn next_current_turn_id(current_turn_id: Option<u64>, incoming_turn_id: u64) -> u64 {
  current_turn_id
    .map(|current| current.max(incoming_turn_id))
    .unwrap_or(incoming_turn_id)
}

pub(super) fn has_turn_context_for_snapshot(
  engine_state: EngineState,
  current_turn_id: Option<u64>,
  finalizing_turn_id: Option<u64>,
  latest_draft: &str,
) -> bool {
  engine_state == EngineState::Speech
    || current_turn_id.is_some()
    || finalizing_turn_id.is_some()
    || !latest_draft.trim().is_empty()
}

pub(super) fn has_actionable_turn_context_for_snapshot(
  engine_state: EngineState,
  current_turn_id: Option<u64>,
  finalizing_turn_id: Option<u64>,
  latest_draft: &str,
  overlay_visible: bool,
  manual_hold_active: bool,
) -> bool {
  if !has_turn_context_for_snapshot(engine_state, current_turn_id, finalizing_turn_id, latest_draft)
  {
    return false;
  }

  // Ignore stale post-turn ids/text once UI is fully idle; they should not
  // block an idle double-tap from toggling always-listening back on.
  engine_state == EngineState::Speech
    || finalizing_turn_id.is_some()
    || overlay_visible
    || manual_hold_active
}

pub(super) fn has_started_turn_for_snapshot(
  manual_hold_active: bool,
  hold_saw_speech: bool,
  engine_state: EngineState,
  finalizing_turn_id: Option<u64>,
  latest_draft: &str,
) -> bool {
  if manual_hold_active {
    return hold_saw_speech;
  }
  // Outside active hold, treat engine speech/finalizing as active turn
  // progress for state decisions.
  if engine_state == EngineState::Speech || finalizing_turn_id.is_some() {
    return true;
  }
  !latest_draft.trim().is_empty()
}

pub(super) fn final_text_has_user_visible_context(
  turn_id: u64,
  current_turn_id: Option<u64>,
  finalizing_turn_id: Option<u64>,
  overlay_visible: bool,
  manual_hold_active: bool,
  latest_draft: &str,
  finalizing_draft: &str,
) -> bool {
  overlay_visible
    || manual_hold_active
    || (current_turn_id == Some(turn_id) && !latest_draft.trim().is_empty())
    || (finalizing_turn_id == Some(turn_id) && !finalizing_draft.trim().is_empty())
}

pub(super) fn listen_toggle_notice(
  enabled: bool,
) -> (&'static str, Vec<platform::OverlayNoticeSegment>) {
  if enabled { ("Listen ENABLED", Vec::new()) } else { ("Listen DISABLED", Vec::new()) }
}

pub(super) fn is_stream_fault_message(message: &str) -> bool {
  let msg = message.to_ascii_lowercase();
  msg.contains("audio input stream ended after error")
    || msg.contains("audio input stream error")
    || msg.contains("failed to open microphone capture")
    || msg.contains("failed to resume cpal stream")
    || msg.contains("requested device is no longer available")
    || msg.contains("input device not found for id")
    || msg.contains("no default input device available")
    || msg.contains("does not support input")
}

pub(super) fn recovery_state_for_fault_count(faults_in_window: usize) -> SessionRecoveryState {
  if faults_in_window >= SESSION_DEGRADED_THRESHOLD {
    SessionRecoveryState::Degraded
  } else if faults_in_window > 0 {
    SessionRecoveryState::Recovering
  } else {
    SessionRecoveryState::Healthy
  }
}

pub(super) fn allow_immediate_restart_for_fault_count(faults_in_window: usize) -> bool {
  faults_in_window <= SESSION_IMMEDIATE_RETRY_LIMIT
}
