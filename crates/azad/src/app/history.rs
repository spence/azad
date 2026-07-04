use crate::input_log::InputLogEvent;
use crate::platform;
use azad_text::{PasteTextOptions, build_paste_text};

use super::{AppController, effective_removed_words};

const HISTORY_SEARCH_LIMIT: usize = 1000;

impl AppController {
  pub(super) fn handle_arrow_navigate(&mut self, direction: i32) {
    self.log_input_event(InputLogEvent::ArrowNavigate { direction });
    // Up only pivots into history while opt+space is actively held. VAD-only
    // sessions must let Up flow through to the focused app underneath.
    if !self.history_browsing && direction == -1 && self.overlay_visible && self.manual_hold_active
    {
      self.enter_history_mode();
      return;
    }
    if !self.history_browsing {
      return;
    }
    // Up/Down inside the expanded view collapses back to the list view and
    // performs the navigation step in one keystroke.
    if self.history_expanded {
      self.history_expanded = false;
    }
    let count = self.transcript_index.as_ref().map(|i| i.entry_count()).unwrap_or(0);
    if count == 0 {
      return;
    }
    // The renderer reports the last visible window so selection movement can
    // keep a small cushion from the top and bottom edges.
    const LAG: usize = 2;
    let last_start = platform::last_history_visible_start();
    let last_count = platform::last_history_visible_count().max(1);
    match direction {
      -1 => {
        if self.history_browse_index + 1 < count {
          self.history_browse_index += 1;
          if self.history_browse_index + LAG >= last_start + last_count {
            self.history_visible_start =
              (self.history_browse_index + 1 + LAG).saturating_sub(last_count);
          }
        }
      }
      1 if self.history_browse_index > 0 => {
        self.history_browse_index -= 1;
        if self.history_browse_index < last_start + LAG {
          self.history_visible_start = self.history_browse_index.saturating_sub(LAG);
        }
      }
      _ => {}
    }
    self.render_history_overlay();
  }

  pub(super) fn handle_history_collapse(&mut self) {
    self.log_input_event(InputLogEvent::HistoryCollapse);
    if !self.history_browsing {
      return;
    }
    // Left arrow collapses expanded content back to the list. Esc is the
    // explicit full-dismiss path.
    if self.history_expanded {
      self.history_expanded = false;
      self.render_history_overlay();
    }
  }

  pub(super) fn handle_history_expand(&mut self) {
    self.log_input_event(InputLogEvent::HistoryExpand);
    if !self.history_browsing || self.history_expanded {
      return;
    }
    let count = self.transcript_index.as_ref().map(|i| i.entry_count()).unwrap_or(0);
    if count == 0 {
      return;
    }
    // If the selected entry is fully visible already, expanding would not
    // reveal anything.
    if !platform::last_history_selected_truncated() {
      return;
    }
    self.history_expanded = true;
    self.render_history_overlay();
  }

  fn render_history_overlay(&self) {
    let Some(index) = &self.transcript_index else {
      if self.debug_stats_enabled {
        eprintln!("AZAD_HISTORY_RENDER action=no_index browse_index={}", self.history_browse_index);
      }
      platform::set_overlay_history_content(&[], 0, 0, false);
      return;
    };
    let hits = index.search(&self.history_search_query, HISTORY_SEARCH_LIMIT);
    let entries: Vec<platform::HistoryEntryView<'_>> = hits
      .iter()
      .map(|h| platform::HistoryEntryView {
        text: h.final_text.as_str(),
        match_ranges: h.match_ranges.clone(),
        ts_ms: h.ts_ms,
        char_count: h.final_text.chars().count(),
      })
      .collect();
    let visible = entries.len();
    let selected = self.history_browse_index.min(visible.saturating_sub(1));
    let visible_start = self.history_visible_start.min(visible.saturating_sub(1));
    if self.debug_stats_enabled {
      let preview = entries
        .first()
        .map(|e| &e.text[..e.text.len().min(40)])
        .unwrap_or("(no entries)");
      eprintln!(
        "AZAD_HISTORY_RENDER mode={} filtered={} selected={} \
         visible_start={} query={:?} first_preview={:?}",
        if self.history_expanded { "expanded" } else { "list" },
        entries.len(),
        selected,
        visible_start,
        self.history_search_query,
        preview,
      );
    }
    platform::set_overlay_history_content(&entries, selected, visible_start, self.history_expanded);
  }

  fn selected_history_entry_text(&self) -> Option<String> {
    let index = self.transcript_index.as_ref()?;
    let hits = index.search(&self.history_search_query, HISTORY_SEARCH_LIMIT);
    hits.into_iter().nth(self.history_browse_index).map(|h| h.final_text)
  }

  pub(super) fn paste_from_history(&mut self) {
    if let Some(text) = self.selected_history_entry_text() {
      let removed_words =
        effective_removed_words(&self.removed_words, self.remove_hesitations_on_paste);
      let paste_text = build_paste_text(
        &text,
        PasteTextOptions {
          append_trailing_space: self.append_trailing_space_on_paste,
          removed_words: &removed_words,
          deduplicate_words: self.deduplicate_words_on_paste,
          convert_number_words: self.convert_number_words_on_paste,
          lowercase_except_uppercase_words: self.lowercase_except_uppercase_words_on_paste,
        },
      );
      // Release search key capture before firing synthetic paste keystrokes so
      // the focused app receives the chord instead of the history search field.
      platform::set_overlay_key_input_enabled(false);
      let _ = platform::insert_text(&paste_text, self.paste_method, self.cfg.paste_delay_ms);
      let _ = platform::send_auto_submit(self.auto_submit_mode);
    }
    // Exit even on an empty-state release so the overlay closes.
    self.exit_history_mode();
  }

  pub(super) fn handle_history_search_changed(&mut self, query: String) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "changed",
      chars_appended: Some(query.chars().count()),
    });
    if !self.history_browsing {
      return;
    }
    if self.history_search_query == query {
      return;
    }
    self.history_search_query = query;
    self.after_history_search_change();
  }

  pub(super) fn handle_history_search_append(&mut self, s: &str) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "append",
      chars_appended: Some(s.chars().count()),
    });
    if !self.history_browsing {
      return;
    }
    self.history_search_query.push_str(s);
    self.after_history_search_change();
  }

  pub(super) fn handle_history_search_backspace(&mut self) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "backspace",
      chars_appended: None,
    });
    if !self.history_browsing {
      return;
    }
    if self.history_search_query.pop().is_none() {
      return;
    }
    self.after_history_search_change();
  }

  pub(super) fn handle_history_search_delete_word(&mut self) {
    self.log_input_event(InputLogEvent::HistorySearchEdit {
      kind: "delete_word",
      chars_appended: None,
    });
    if !self.history_browsing || self.history_search_query.is_empty() {
      return;
    }
    let trimmed = self.history_search_query.trim_end().to_string();
    if let Some(idx) = trimmed.rfind(char::is_whitespace) {
      let cut = trimmed[..idx].trim_end().len();
      self.history_search_query.truncate(cut);
    } else {
      self.history_search_query.clear();
    }
    self.after_history_search_change();
  }

  pub(super) fn handle_history_search_clear(&mut self) {
    self.log_input_event(InputLogEvent::HistorySearchEdit { kind: "clear", chars_appended: None });
    if !self.history_browsing || self.history_search_query.is_empty() {
      return;
    }
    self.history_search_query.clear();
    self.after_history_search_change();
  }

  fn after_history_search_change(&mut self) {
    self.history_browse_index = 0;
    self.history_visible_start = 0;
    self.history_expanded = false;
    platform::set_overlay_search_query(&self.history_search_query);
    self.render_history_overlay();
  }

  fn enter_history_mode(&mut self) {
    // History owns the overlay and pauses capture so a simultaneous in-flight
    // turn cannot paste while the user is browsing saved transcripts.
    self.manual_hold_active = false;
    self.hold_saw_speech = false;
    if let Some(session) = &self.session {
      session.release_manual_hold();
      session.cancel_current_turn();
      session.set_capture_enabled(false);
    }
    self.latest_draft.clear();
    self.finalizing_draft.clear();
    self.finalizing_turn_id = None;
    self.finalizing_deadline = None;
    self.history_browsing = true;
    self.history_browse_index = 0;
    self.history_visible_start = 0;
    self.history_expanded = false;
    self.history_search_query.clear();
    platform::set_arrow_left_hotkey_enabled(true);
    platform::set_arrow_right_hotkey_enabled(true);
    platform::reset_click_outside_tracker();
    platform::set_overlay_search_query("");
    platform::set_overlay_key_input_enabled(true);
    if !self.overlay_visible {
      platform::show_overlay();
      self.overlay_visible = true;
    }
    self.render_history_overlay();
  }

  pub(super) fn exit_history_mode(&mut self) {
    self.history_browsing = false;
    self.history_browse_index = 0;
    self.history_visible_start = 0;
    self.history_expanded = false;
    self.history_search_query.clear();
    self.overlay_visible = false;
    platform::set_overlay_key_input_enabled(false);
    platform::set_overlay_search_query("");
    platform::set_arrow_left_hotkey_enabled(false);
    platform::set_arrow_right_hotkey_enabled(false);
    platform::hide_overlay();
    if let Some(session) = &self.session {
      session.set_capture_enabled(true);
    }
  }
}
