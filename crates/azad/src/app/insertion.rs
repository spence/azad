use std::time::Instant;

use azad_text::{PasteTextOptions, build_paste_text};

use crate::input_log::InputLogEvent;
use crate::metrics_log::{self, MetricsLogEvent, MetricsLogRecord, TranscriptMode};
use crate::platform::{self, InsertStart, PacedTyping, PasteResult};
use crate::settings::{AutoSubmitMode, PasteMethod};
use crate::typing::TypingProgress;

use super::{AppController, auto_submit_mode_label, effective_removed_words};
use super::{paste_method_label, paste_result_label};
use super::{preview_text_for_metrics, transcript_mode_label};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct InsertionSource {
  pub session_id: u64,
  pub turn_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InsertOrigin {
  Normal { dismiss: bool },
  SplitTop,
  Raw { dismiss: bool },
  SessionEnd,
  History,
}

impl InsertOrigin {
  fn owns_overlay(self) -> bool {
    matches!(
      self,
      Self::Normal { dismiss: true }
        | Self::Raw { dismiss: true }
        | Self::SessionEnd
        | Self::History
    )
  }

  fn records_history(self) -> bool {
    matches!(self, Self::Normal { .. } | Self::SplitTop)
  }
}

pub(super) struct InsertionRequest {
  id: u64,
  source: Option<InsertionSource>,
  origin: InsertOrigin,
  mode: TranscriptMode,
  text: String,
  payload: String,
  history_draft: String,
  method: PasteMethod,
  rate: u32,
  submit: AutoSubmitMode,
  paste_delay_ms: u64,
  requested_at: Instant,
  turn_started_at: Option<Instant>,
  debug_stats: bool,
}

pub(super) struct ActiveInsertion {
  request: InsertionRequest,
  typing: PacedTyping,
  started_at: Instant,
}

impl AppController {
  fn insertion_source(&self, turn_id: u64) -> InsertionSource {
    InsertionSource {
      session_id: self.current_session_id().unwrap_or(self.next_session_id.saturating_sub(1)),
      turn_id,
    }
  }

  pub(super) fn insertion_source_handled(&self, turn_id: u64) -> bool {
    self.handled_insertions.contains(&self.insertion_source(turn_id))
  }

  pub(super) fn request_paste(
    &mut self,
    turn_id: u64,
    mode: TranscriptMode,
    text: &str,
    origin: InsertOrigin,
  ) {
    if self.insertion_source_handled(turn_id) {
      return;
    }
    let text = self.strip_active_trigger(text);
    let removed_words =
      effective_removed_words(&self.removed_words, self.remove_hesitations_on_paste);
    let payload = build_paste_text(
      &text,
      PasteTextOptions {
        append_trailing_space: self.append_trailing_space_on_paste,
        removed_words: &removed_words,
        deduplicate_words: self.deduplicate_words_on_paste,
        convert_number_words: self.convert_number_words_on_paste,
        convert_spoken_emoji: self.convert_spoken_emoji_on_paste,
        lowercase_except_uppercase_words: self.lowercase_except_uppercase_words_on_paste,
      },
    );
    let request =
      self.new_insertion(Some(self.insertion_source(turn_id)), origin, mode, text, payload);
    self.admit_insertion(request);
  }

  pub(super) fn request_history_insertion(&mut self, text: String) {
    let request =
      self.new_insertion(None, InsertOrigin::History, TranscriptMode::Normal, text.clone(), text);
    self.admit_insertion(request);
  }

  fn new_insertion(
    &mut self,
    source: Option<InsertionSource>,
    origin: InsertOrigin,
    mode: TranscriptMode,
    text: String,
    payload: String,
  ) -> InsertionRequest {
    let id = self.next_insertion_id;
    self.next_insertion_id = self.next_insertion_id.checked_add(1).expect("insertion id exhausted");
    let turn_started_at = source.and_then(|source| self.turn_started_at.remove(&source.turn_id));
    InsertionRequest {
      id,
      source,
      origin,
      mode,
      text,
      payload,
      history_draft: self.finalizing_draft.clone(),
      method: self.paste_method,
      rate: self.typing_key_rate,
      submit: self.auto_submit_mode,
      paste_delay_ms: self.cfg.paste_delay_ms,
      requested_at: Instant::now(),
      turn_started_at,
      debug_stats: self.debug_stats_enabled,
    }
  }

  fn admit_insertion(&mut self, request: InsertionRequest) {
    if self.active_insertion.is_some()
      || !self.insertion_queue.is_empty()
      || (request.rate > 0 && request.method != PasteMethod::ClipboardPaste)
    {
      if let Some(source) = request.source {
        self.handled_insertions.insert(source);
      }
    }
    if request.origin.owns_overlay() {
      self.insertion_overlay_owner = Some(request.id);
    }
    self.insertion_queue.push_back(request);
    self.start_next_insertion();
  }

  fn start_next_insertion(&mut self) {
    while self.active_insertion.is_none() {
      let Some(request) = self.insertion_queue.pop_front() else { return };
      let started_at = Instant::now();
      if request.method != PasteMethod::ClipboardPaste {
        let payload_json = serde_json::to_string(&request.payload).unwrap_or_default();
        eprintln!(
          "AZAD_DIRECT_PAYLOAD turn_id={} transcript_mode={} method={} chars={} payload={}",
          request.source.map_or(0, |s| s.turn_id),
          transcript_mode_label(request.mode),
          paste_method_label(request.method),
          request.payload.chars().count(),
          payload_json
        );
      }
      match platform::start_insert(
        &request.payload,
        request.method,
        request.paste_delay_ms,
        request.rate,
      ) {
        InsertStart::Finished(result) => {
          self.complete_insertion(request, result, started_at, false)
        }
        InsertStart::PacedFailed(result) => {
          self.complete_insertion(request, result, started_at, true)
        }
        InsertStart::Paced(mut typing) => match typing.advance(Instant::now()) {
          TypingProgress::Waiting(after) => {
            let id = request.id;
            self.active_insertion = Some(ActiveInsertion { request, typing, started_at });
            self.render_insertion_overlay();
            platform::schedule_typing_timer(id, after);
            return;
          }
          progress => {
            drop(typing);
            let result = if progress == TypingProgress::Complete {
              PasteResult::Pasted
            } else {
              PasteResult::InputEventFailed
            };
            self.complete_insertion(request, result, started_at, true);
          }
        },
      }
    }
  }

  pub(super) fn advance_insertion(&mut self, request_id: u64, now: Instant) {
    let Some(active) =
      self.active_insertion.as_mut().filter(|active| active.request.id == request_id)
    else {
      return;
    };
    match active.typing.advance(now) {
      TypingProgress::Waiting(after) => platform::schedule_typing_timer(request_id, after),
      progress => {
        let ActiveInsertion { request, typing, started_at } = self.active_insertion.take().unwrap();
        drop(typing);
        let result = if progress == TypingProgress::Complete {
          PasteResult::Pasted
        } else {
          PasteResult::InputEventFailed
        };
        self.complete_insertion(request, result, started_at, true);
        self.start_next_insertion();
      }
    }
  }

  pub(super) fn cancel_insertion_output(&mut self) {
    platform::cancel_typing_timer();
    self.insertion_overlay_owner = None;
    if let Some(ActiveInsertion { request, typing, started_at }) = self.active_insertion.take() {
      drop(typing);
      self.complete_insertion(request, PasteResult::Cancelled, started_at, true);
    }
    while let Some(request) = self.insertion_queue.pop_front() {
      self.complete_insertion(request, PasteResult::Cancelled, Instant::now(), true);
    }
  }

  pub(super) fn render_insertion_overlay(&self) -> bool {
    let Some(owner) = self.insertion_overlay_owner else { return false };
    if !self.overlay_visible {
      return true;
    }
    let request = self
      .active_insertion
      .as_ref()
      .map(|active| &active.request)
      .filter(|request| request.id == owner)
      .or_else(|| self.insertion_queue.iter().find(|request| request.id == owner));
    let Some(request) = request else { return false };
    let text = self.stream_display_text(&request.text);
    platform::hide_overlay_top();
    platform::set_overlay_stream_content(
      &text,
      &self.activity_history,
      Some(self.busy_border_phase),
      request.mode == TranscriptMode::Raw,
      false,
      "",
      "",
      "",
    );
    true
  }

  fn complete_insertion(
    &mut self,
    request: InsertionRequest,
    result: PasteResult,
    started_at: Instant,
    paced: bool,
  ) {
    let insert_duration_ms = started_at.elapsed().as_millis() as u64;
    let submit_started = Instant::now();
    let should_submit = result == PasteResult::Pasted
      || (request.origin == InsertOrigin::History && !paced && result != PasteResult::Cancelled);
    let auto_submit_sent = should_submit && request.submit != AutoSubmitMode::Off;
    let auto_submit_ok = !should_submit || platform::send_auto_submit(request.submit);
    let auto_submit_duration_ms =
      if should_submit { submit_started.elapsed().as_millis() as u64 } else { 0 };
    let paste_duration_ms = request.requested_at.elapsed().as_millis() as u64;
    if request.origin != InsertOrigin::History {
      let turn_id = request.source.map_or(0, |source| source.turn_id);
      eprintln!(
        "AZAD_INSERT_PERF turn_id={} transcript_mode={} method={} chars={} result={} insert_ms={} auto_submit_mode={} auto_submit_sent={} auto_submit_ms={} auto_submit_ok={} total_ms={}",
        turn_id,
        transcript_mode_label(request.mode),
        paste_method_label(request.method),
        request.payload.chars().count(),
        paste_result_label(result),
        insert_duration_ms,
        auto_submit_mode_label(request.submit),
        auto_submit_sent,
        auto_submit_duration_ms,
        auto_submit_ok,
        paste_duration_ms
      );
      if request.debug_stats {
        let transcription_duration_ms = request
          .turn_started_at
          .map_or(0, |started| started.elapsed().as_millis() as u64);
        let _ =
          metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::PasteCompleted {
            turn_id,
            mode: request.mode,
            paste_duration_ms,
            result: paste_result_label(result).into(),
          }));
        if transcription_duration_ms > 0 {
          let _ =
            metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::TurnCompleted {
              turn_id,
              mode: request.mode,
              transcription_duration_ms,
            }));
        }
        let _ = metrics_log::append_record(&MetricsLogRecord::new(MetricsLogEvent::TurnSnapshot {
          turn_id,
          mode: request.mode,
          transcription_duration_ms,
          text_preview: preview_text_for_metrics(&request.text, 45),
        }));
      }
      self.log_input_event(InputLogEvent::PasteAttempt {
        turn_id,
        mode: if request.mode == TranscriptMode::Raw { "raw" } else { "normal" },
        source: paste_result_label(result),
        paste_ok: result == PasteResult::Pasted,
      });
      if result == PasteResult::Pasted {
        if let Some(source) = request.source {
          if source.session_id
            == self.current_session_id().unwrap_or(self.next_session_id.saturating_sub(1))
          {
            self.last_pasted_turn_id = Some(source.turn_id);
          }
          if request.origin.records_history() && self.history_enabled {
            if let Some(index) = self.transcript_index.as_mut() {
              index.append(source.turn_id, &request.history_draft, &request.text);
            }
          }
        }
      }
    }
    if result == PasteResult::AccessibilityRequired && request.origin != InsertOrigin::History {
      self.disable_listening_due_to_accessibility();
    }
    if result != PasteResult::Pasted && result != PasteResult::Cancelled {
      eprintln!("Azad: transcript insertion failed ({})", paste_result_label(result));
    }
    if auto_submit_sent && !auto_submit_ok {
      eprintln!(
        "Azad: failed to send auto-submit key event (mode={})",
        auto_submit_mode_label(request.submit)
      );
    }
    if self.insertion_overlay_owner == Some(request.id) {
      self.insertion_overlay_owner = None;
      match request.origin {
        InsertOrigin::Normal { dismiss: true } => self.dismiss_overlay_after_standard_dictation(),
        InsertOrigin::Raw { dismiss: true } | InsertOrigin::SessionEnd => self.hide_overlay(),
        InsertOrigin::History => self.exit_history_mode(),
        _ => {}
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use crate::config::AzadConfig;
  use crate::platform::TestOutput;
  use crate::speech::{SpeechEvent, SpeechSession};

  use super::*;

  fn controller() -> AppController {
    platform::take_test_output();
    platform::fail_typing_after_for_test(None);
    let mut app = AppController::new(AzadConfig::default());
    app.session = Some(SpeechSession::test(7));
    app.next_session_id = 8;
    app.overlay_visible = true;
    app.current_turn_id = Some(1);
    app.latest_seen_turn_id = 1;
    app.latest_draft = "ab".into();
    app.paste_method = PasteMethod::DirectTyping;
    app.typing_key_rate = 80;
    app.auto_submit_mode = AutoSubmitMode::Enter;
    app.append_trailing_space_on_paste = false;
    app.remove_hesitations_on_paste = false;
    app.debug_stats_enabled = false;
    app.history_enabled = true;
    app
  }

  fn finish(app: &mut AppController) {
    let mut now = Instant::now() + Duration::from_secs(1);
    let mut ticks = 0;
    while let Some(active) = &app.active_insertion {
      let id = active.request.id;
      app.advance_insertion(id, now);
      now += Duration::from_secs(1);
      ticks += 1;
      assert!(ticks < 1000);
    }
  }

  fn final_text(turn_id: u64, text: &str) -> SpeechEvent {
    SpeechEvent::FinalText { session_id: 7, turn_id, text: text.into() }
  }

  #[test]
  fn overlapping_typing_paste_and_history_keep_snapshot_and_completion_order() {
    let mut app = controller();
    app.paste_method = PasteMethod::DirectTypingAndCopyClipboard;
    app.request_paste(1, TranscriptMode::Normal, "ab", InsertOrigin::Normal { dismiss: true });
    assert_eq!(platform::take_test_output(), [TestOutput::Pair('a')]);
    assert_eq!(app.last_pasted_turn_id, None);
    assert_eq!(app.transcript_index.as_ref().unwrap().entry_count(), 0);
    app.paste_method = PasteMethod::DirectTyping;
    app.typing_key_rate = 320;
    app.auto_submit_mode = AutoSubmitMode::CtrlEnter;
    app.request_paste(2, TranscriptMode::Normal, "cd", InsertOrigin::SplitTop);
    app.paste_method = PasteMethod::ClipboardPaste;
    app.auto_submit_mode = AutoSubmitMode::ShiftEnter;
    app.request_paste(3, TranscriptMode::Normal, "ef", InsertOrigin::SplitTop);
    app.paste_method = PasteMethod::DirectTyping;
    app.typing_key_rate = 0;
    app.auto_submit_mode = AutoSubmitMode::Off;
    app.request_history_insertion("gh".into());
    assert!(platform::take_test_output().is_empty());
    assert_eq!(app.active_insertion.as_ref().unwrap().request.rate, 80);
    finish(&mut app);
    assert_eq!(
      platform::take_test_output(),
      [
        TestOutput::Pair('b'),
        TestOutput::Copy("ab".into()),
        TestOutput::Submit(AutoSubmitMode::Enter),
        TestOutput::Pair('c'),
        TestOutput::Pair('d'),
        TestOutput::Submit(AutoSubmitMode::CtrlEnter),
        TestOutput::Insert("ef".into(), PasteMethod::ClipboardPaste),
        TestOutput::Submit(AutoSubmitMode::ShiftEnter),
        TestOutput::Insert("gh".into(), PasteMethod::DirectTyping),
        TestOutput::Submit(AutoSubmitMode::Off),
      ]
    );
    assert_eq!(app.transcript_index.as_ref().unwrap().entry_count(), 3);
  }

  #[test]
  fn newer_speech_keeps_overlay_and_duplicate_final_cannot_mutate_it() {
    let mut app = controller();
    app.handle_speech_event(final_text(1, "ab"));
    assert!(app.active_insertion.is_some());
    app.handle_speech_event(SpeechEvent::DraftUpdated {
      session_id: 7,
      turn_id: 2,
      committed: "new utterance".into(),
      live: String::new(),
    });
    assert_eq!(app.insertion_overlay_owner, None);
    app.handle_speech_event(final_text(1, "late duplicate"));
    assert_eq!(app.latest_draft, "new utterance");
    finish(&mut app);
    assert!(app.overlay_visible);
    assert_eq!(app.current_turn_id, Some(2));
    assert_eq!(app.latest_draft, "new utterance");
    let history = app.transcript_index.as_ref().unwrap().search("", 10);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].final_text, "ab");
  }

  #[test]
  fn partial_failure_is_not_retried_by_session_end() {
    let mut app = controller();
    app.paste_method = PasteMethod::DirectTypingAndCopyClipboard;
    platform::fail_typing_after_for_test(Some(1));
    app.handle_speech_event(final_text(1, "abc"));
    finish(&mut app);
    platform::fail_typing_after_for_test(None);
    assert_eq!(platform::take_test_output(), [TestOutput::Pair('a')]);
    assert_eq!(app.last_pasted_turn_id, None);
    assert_eq!(app.transcript_index.as_ref().unwrap().entry_count(), 0);
    app.handle_speech_event(SpeechEvent::SessionEnded { session_id: 7 });
    assert!(platform::take_test_output().is_empty());
    assert!(app.active_insertion.is_none());
  }

  #[test]
  fn escape_discards_tail_and_late_callbacks_cannot_retype_prefix() {
    let mut app = controller();
    app.handle_speech_event(final_text(1, "abc"));
    let stale_id = app.active_insertion.as_ref().unwrap().request.id;
    app.request_paste(2, TranscriptMode::Raw, "queued", InsertOrigin::Raw { dismiss: true });
    app.handle_overlay_cancel();
    app.handle_speech_event(final_text(1, "abc"));
    app.handle_speech_event(final_text(2, "queued"));
    app.advance_insertion(stale_id, Instant::now() + Duration::from_secs(10));
    assert_eq!(platform::take_test_output(), [TestOutput::Pair('a')]);
    assert!(app.active_insertion.is_none());
    assert!(app.insertion_queue.is_empty());
    assert_eq!(app.last_pasted_turn_id, None);
    assert!(!app.overlay_visible);
  }

  #[test]
  fn raw_and_history_wait_for_success_and_do_not_add_history() {
    let mut app = controller();
    assert!(app.try_finalize_with_raw_text(Some(1)));
    assert_eq!(platform::take_test_output(), [TestOutput::Pair('a')]);
    assert!(app.overlay_visible);
    assert_eq!(app.last_pasted_turn_id, None);
    finish(&mut app);
    assert!(!app.overlay_visible);
    assert_eq!(app.last_pasted_turn_id, Some(1));
    assert_eq!(app.transcript_index.as_ref().unwrap().entry_count(), 0);

    app.overlay_visible = true;
    app.history_browsing = true;
    app.request_history_insertion("xy".into());
    assert!(app.history_browsing);
    finish(&mut app);
    assert!(!app.history_browsing);
    assert!(!app.overlay_visible);
    assert_eq!(app.transcript_index.as_ref().unwrap().entry_count(), 0);
  }

  #[test]
  fn history_takeover_and_new_request_are_not_dismissed_by_old_completion() {
    let mut app = controller();
    app.handle_speech_event(final_text(1, "ab"));
    let old_id = app.active_insertion.as_ref().unwrap().request.id;
    app.manual_hold_active = true;
    app.handle_arrow_navigate(-1);
    assert!(app.history_browsing);
    assert_eq!(app.insertion_overlay_owner, None);
    app.advance_insertion(old_id, Instant::now() + Duration::from_secs(1));
    assert!(app.history_browsing);
    assert!(app.overlay_visible);
    app.request_history_insertion("xy".into());
    let new_id = app.active_insertion.as_ref().unwrap().request.id;
    platform::take_test_output();
    app.advance_insertion(old_id, Instant::now() + Duration::from_secs(10));
    assert!(platform::take_test_output().is_empty());
    assert_eq!(app.active_insertion.as_ref().unwrap().request.id, new_id);
    finish(&mut app);
  }

  #[test]
  fn max_and_clipboard_preserve_synchronous_paths() {
    for (method, rate) in [(PasteMethod::DirectTyping, 0), (PasteMethod::ClipboardPaste, 20)] {
      let mut app = controller();
      app.paste_method = method;
      app.typing_key_rate = rate;
      app.handle_speech_event(final_text(1, "ab"));
      assert!(app.active_insertion.is_none());
      assert_eq!(
        platform::take_test_output(),
        [TestOutput::Insert("ab".into(), method), TestOutput::Submit(AutoSubmitMode::Enter),]
      );
      assert_eq!(app.last_pasted_turn_id, Some(1));
      assert!(!app.overlay_visible);
    }
  }

  #[test]
  fn synchronous_history_failure_preserves_listening_and_submit_baseline() {
    let mut app = controller();
    app.always_listening_enabled = true;
    let request = app.new_insertion(
      None,
      InsertOrigin::History,
      TranscriptMode::Normal,
      "text".into(),
      "text".into(),
    );
    app.complete_insertion(request, PasteResult::AccessibilityRequired, Instant::now(), false);
    assert!(app.always_listening_enabled);
    assert_eq!(platform::take_test_output(), [TestOutput::Submit(AutoSubmitMode::Enter)]);
    let request = app.new_insertion(
      None,
      InsertOrigin::History,
      TranscriptMode::Normal,
      "text".into(),
      "text".into(),
    );
    app.complete_insertion(request, PasteResult::InputEventFailed, Instant::now(), true);
    assert!(platform::take_test_output().is_empty());
  }

  #[test]
  fn history_navigation_restores_search_after_paced_selection() {
    let mut app = controller();
    app.transcript_index.as_mut().unwrap().append(10, "", "first");
    app.transcript_index.as_mut().unwrap().append(11, "", "second");
    app.manual_hold_active = true;
    app.handle_arrow_navigate(-1);
    app.paste_from_history();
    assert!(app.active_insertion.is_some());
    assert!(!platform::history_key_capture_for_test());
    let owner = app.insertion_overlay_owner;
    app.handle_history_collapse();
    assert_eq!(app.insertion_overlay_owner, owner);
    app.handle_arrow_navigate(-1);
    assert!(platform::history_key_capture_for_test());
    assert_eq!(app.insertion_overlay_owner, None);
    finish(&mut app);
    assert!(app.history_browsing);
    assert!(platform::history_key_capture_for_test());
    app.handle_history_search_append("query");
    assert_eq!(app.history_search_query, "query");
    app.exit_history_mode();
  }
}
