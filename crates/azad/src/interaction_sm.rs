pub const DEFAULT_DOUBLE_TAP_WINDOW_MS: u64 = 450;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionConfig {
  pub double_tap_window_ms: u64,
}

impl Default for InteractionConfig {
  fn default() -> Self {
    Self { double_tap_window_ms: DEFAULT_DOUBLE_TAP_WINDOW_MS }
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeSnapshot {
  pub always_listening_enabled: bool,
  pub has_active_speech_turn: bool,
  pub has_turn_context: bool,
  pub has_started_turn: bool,
  pub overlay_visible: bool,
  pub manual_hold_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionInput {
  HoldPressed { now_ms: u64, snapshot: RuntimeSnapshot },
  HoldReleased { snapshot: RuntimeSnapshot },
  FinalizePressed { overlay_visible: bool },
  MenuToggleAlwaysListening,
  SpeechFinalized,
  SpeechIdle { manual_hold_active: bool },
  OverlayCancelled,
  TurnReset,
  SessionReset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionEffect {
  InterruptAndToggleAlwaysListening,
  MenuToggleAlwaysListening,
  ActivateManualHold { reset_turn_state: bool, release_should_finalize: bool },
  ReleaseManualHold { should_finalize: bool, has_started_turn: bool },
  FinalizeFromHotkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionState {
  config: InteractionConfig,
  last_hold_press_at_ms: Option<u64>,
  release_should_finalize: bool,
  manual_finalize_pending: bool,
}

impl Default for InteractionState {
  fn default() -> Self {
    Self::new(InteractionConfig::default())
  }
}

impl InteractionState {
  pub fn new(config: InteractionConfig) -> Self {
    Self {
      config,
      last_hold_press_at_ms: None,
      release_should_finalize: false,
      manual_finalize_pending: false,
    }
  }

  #[cfg(test)]
  pub fn manual_finalize_pending(&self) -> bool {
    self.manual_finalize_pending
  }

  pub fn reduce(&mut self, input: InteractionInput) -> Vec<InteractionEffect> {
    match input {
      InteractionInput::HoldPressed { now_ms, snapshot } => {
        let is_double_tap = self
          .last_hold_press_at_ms
          .is_some_and(|last| now_ms.saturating_sub(last) <= self.config.double_tap_window_ms);
        self.last_hold_press_at_ms = Some(now_ms);

        // Double-tap toggle is allowed from any runtime snapshot as long as
        // we are not carrying a pending finalize from a prior spoken hold.
        if is_double_tap && !self.manual_finalize_pending {
          self.release_should_finalize = false;
          self.manual_finalize_pending = false;
          return vec![InteractionEffect::InterruptAndToggleAlwaysListening];
        }

        let preserve_active_turn = self.manual_finalize_pending
          || (snapshot.always_listening_enabled && snapshot.has_active_speech_turn);
        let release_should_finalize = self.manual_finalize_pending || !preserve_active_turn;
        self.release_should_finalize = release_should_finalize;
        self.manual_finalize_pending = false;
        vec![InteractionEffect::ActivateManualHold {
          reset_turn_state: !preserve_active_turn,
          release_should_finalize,
        }]
      }
      InteractionInput::HoldReleased { snapshot } => {
        let should_finalize = self.release_should_finalize;
        self.release_should_finalize = false;
        self.manual_finalize_pending =
          should_finalize && snapshot.has_started_turn && snapshot.has_turn_context;

        vec![InteractionEffect::ReleaseManualHold {
          should_finalize,
          has_started_turn: snapshot.has_started_turn,
        }]
      }
      InteractionInput::FinalizePressed { overlay_visible } => {
        if !overlay_visible {
          return Vec::new();
        }
        self.manual_finalize_pending = true;
        vec![InteractionEffect::FinalizeFromHotkey]
      }
      InteractionInput::MenuToggleAlwaysListening => {
        vec![InteractionEffect::MenuToggleAlwaysListening]
      }
      InteractionInput::SpeechFinalized => {
        self.manual_finalize_pending = false;
        Vec::new()
      }
      InteractionInput::SpeechIdle { manual_hold_active } => {
        if !manual_hold_active {
          self.manual_finalize_pending = false;
        }
        Vec::new()
      }
      InteractionInput::OverlayCancelled | InteractionInput::TurnReset => {
        self.release_should_finalize = false;
        self.manual_finalize_pending = false;
        Vec::new()
      }
      InteractionInput::SessionReset => {
        self.release_should_finalize = false;
        self.manual_finalize_pending = false;
        self.last_hold_press_at_ms = None;
        Vec::new()
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{
    DEFAULT_DOUBLE_TAP_WINDOW_MS,
    InteractionEffect,
    InteractionInput,
    InteractionState,
    RuntimeSnapshot,
  };

  fn assert_core_state(
    sm: &InteractionState,
    last_hold_press_at_ms: Option<u64>,
    release_should_finalize: bool,
    manual_finalize_pending: bool,
  ) {
    assert_eq!(sm.last_hold_press_at_ms, last_hold_press_at_ms);
    assert_eq!(sm.release_should_finalize, release_should_finalize);
    assert_eq!(sm.manual_finalize_pending, manual_finalize_pending);
  }

  fn snapshot(
    always_listening_enabled: bool,
    has_active_speech_turn: bool,
    has_turn_context: bool,
    has_started_turn: bool,
    overlay_visible: bool,
  ) -> RuntimeSnapshot {
    RuntimeSnapshot {
      always_listening_enabled,
      has_active_speech_turn,
      has_turn_context,
      has_started_turn,
      overlay_visible,
      manual_hold_active: false,
    }
  }

  #[test]
  fn double_tap_from_idle_toggles_mode_without_starting_hold() {
    let mut sm = InteractionState::default();

    let first_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(first_press, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: true,
      release_should_finalize: true,
    }]);

    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, false, false, false),
    });

    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000 + DEFAULT_DOUBLE_TAP_WINDOW_MS - 1,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(second_press, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);
  }

  #[test]
  fn active_vad_double_tap_toggles_and_does_not_force_finalize_on_release() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(true, true, true, true, true),
    });

    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1100,
      snapshot: snapshot(true, true, true, true, true),
    });
    assert_eq!(second_press, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);

    let release = sm
      .reduce(InteractionInput::HoldReleased { snapshot: snapshot(true, true, true, true, true) });
    assert_eq!(release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: false,
      has_started_turn: true
    }]);
  }

  #[test]
  fn vad_off_rapid_repress_preserves_pending_finalize_context() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let first_release = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert_eq!(first_release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: true
    }]);
    assert!(sm.manual_finalize_pending());

    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1100,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(second_press, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: false,
      release_should_finalize: true,
    }]);

    let _ = sm.reduce(InteractionInput::SpeechFinalized);
    assert!(!sm.manual_finalize_pending());

    let second_release = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert_eq!(second_release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: true
    }]);
  }

  #[test]
  fn no_speech_release_does_not_mark_finalize_pending() {
    let mut sm = InteractionState::default();
    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });

    let release = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: false
    }]);
    assert!(!sm.manual_finalize_pending());
  }

  #[test]
  fn manual_hold_only_release_does_not_block_idle_double_tap_toggle() {
    let mut sm = InteractionState::default();
    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });

    let first_release = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, false, true, true),
    });
    assert_eq!(first_release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: true
    }]);
    assert!(!sm.manual_finalize_pending());

    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000 + DEFAULT_DOUBLE_TAP_WINDOW_MS - 1,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(second_press, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);
  }

  #[test]
  fn menu_toggle_emits_toggle_effect() {
    let mut sm = InteractionState::default();
    let effects = sm.reduce(InteractionInput::MenuToggleAlwaysListening);
    assert_eq!(effects, vec![InteractionEffect::MenuToggleAlwaysListening]);
  }

  #[test]
  fn speech_idle_clears_manual_finalize_pending() {
    let mut sm = InteractionState::default();
    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert!(sm.manual_finalize_pending());

    let _ = sm.reduce(InteractionInput::SpeechIdle { manual_hold_active: false });
    assert!(!sm.manual_finalize_pending());
  }

  #[test]
  fn active_vad_single_press_release_is_assist_only() {
    let mut sm = InteractionState::default();

    let press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(true, true, true, true, true),
    });
    assert_eq!(press, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: false,
      release_should_finalize: false,
    }]);

    let release = sm
      .reduce(InteractionInput::HoldReleased { snapshot: snapshot(true, true, true, true, true) });
    assert_eq!(release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: false,
      has_started_turn: true
    }]);
    assert!(!sm.manual_finalize_pending());
  }

  #[test]
  fn listen_on_idle_hold_session_finalizes_on_release() {
    let mut sm = InteractionState::default();

    let press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(true, false, false, false, false),
    });
    assert_eq!(press, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: true,
      release_should_finalize: true,
    }]);

    let release = sm
      .reduce(InteractionInput::HoldReleased { snapshot: snapshot(true, false, true, true, true) });
    assert_eq!(release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: true
    }]);
  }

  #[test]
  fn double_tap_window_expiry_prevents_toggle() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, false, false, false),
    });

    let press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000 + DEFAULT_DOUBLE_TAP_WINDOW_MS + 1,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(press, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: true,
      release_should_finalize: true,
    }]);
  }

  #[test]
  fn double_tap_from_vad_on_idle_toggles_without_starting_hold() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(true, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(true, false, false, false, false),
    });

    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000 + DEFAULT_DOUBLE_TAP_WINDOW_MS - 1,
      snapshot: snapshot(true, false, false, false, false),
    });
    assert_eq!(second_press, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);
  }

  #[test]
  fn session_reset_breaks_double_tap_chain() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::SessionReset);

    let press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000 + DEFAULT_DOUBLE_TAP_WINDOW_MS - 1,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(press, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: true,
      release_should_finalize: true,
    }]);
  }

  #[test]
  fn finalize_pressed_requires_overlay_visibility() {
    let mut sm = InteractionState::default();

    let hidden = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: false });
    assert!(hidden.is_empty());
    assert!(!sm.manual_finalize_pending());

    let visible = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_eq!(visible, vec![InteractionEffect::FinalizeFromHotkey]);
    assert!(sm.manual_finalize_pending());
  }

  #[test]
  fn speech_idle_with_manual_hold_keeps_finalize_pending() {
    let mut sm = InteractionState::default();
    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert!(sm.manual_finalize_pending());

    let _ = sm.reduce(InteractionInput::SpeechIdle { manual_hold_active: true });
    assert!(sm.manual_finalize_pending());
  }

  #[test]
  fn overlay_cancelled_clears_release_finalize_before_release() {
    let mut sm = InteractionState::default();
    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });

    let _ = sm.reduce(InteractionInput::OverlayCancelled);

    let release = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert_eq!(release, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: false,
      has_started_turn: true
    }]);
    assert!(!sm.manual_finalize_pending());
  }

  #[test]
  fn turn_reset_clears_pending_finalize_context() {
    let mut sm = InteractionState::default();
    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert!(sm.manual_finalize_pending());

    let _ = sm.reduce(InteractionInput::TurnReset);
    assert!(!sm.manual_finalize_pending());

    let press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000 + DEFAULT_DOUBLE_TAP_WINDOW_MS + 1,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(press, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: true,
      release_should_finalize: true,
    }]);
  }

  #[test]
  fn vad_off_double_tap_with_turn_context_toggles_when_no_finalize_pending() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, true, true, true),
    });
    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1100,
      snapshot: snapshot(false, false, true, true, true),
    });
    assert_eq!(second_press, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);
  }

  #[test]
  fn stale_turn_context_without_started_turn_does_not_block_enable_toggle() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, true, false, true),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, false, true),
    });

    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1100,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(second_press, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);
  }

  #[test]
  fn double_tap_with_started_turn_toggles_when_finalize_not_pending() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(true, false, true, true, true),
    });

    let second_press = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1100,
      snapshot: snapshot(true, false, true, true, true),
    });
    assert_eq!(second_press, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);
  }

  #[test]
  fn hold_pressed_single_press_updates_press_timestamp_and_finalize_flag() {
    let mut sm = InteractionState::default();

    let effects = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 4242,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(effects, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: true,
      release_should_finalize: true,
    }]);
    assert_core_state(&sm, Some(4242), true, false);
  }

  #[test]
  fn hold_pressed_with_pending_finalize_preserves_turn_then_clears_pending_state() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert_core_state(&sm, Some(1000), false, true);

    let effects = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1100,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(effects, vec![InteractionEffect::ActivateManualHold {
      reset_turn_state: false,
      release_should_finalize: true,
    }]);
    assert_core_state(&sm, Some(1100), true, false);
  }

  #[test]
  fn hold_pressed_double_tap_toggle_clears_finalize_flags() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_core_state(&sm, Some(1000), false, false);

    let effects = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1100,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_eq!(effects, vec![InteractionEffect::InterruptAndToggleAlwaysListening]);
    assert_core_state(&sm, Some(1100), false, false);
  }

  #[test]
  fn hold_released_sets_pending_finalize_only_for_started_turn_with_context() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_core_state(&sm, Some(1000), true, false);

    let effects = sm.reduce(InteractionInput::HoldReleased {
      snapshot: snapshot(false, false, true, true, true),
    });
    assert_eq!(effects, vec![InteractionEffect::ReleaseManualHold {
      should_finalize: true,
      has_started_turn: true
    }]);
    assert_core_state(&sm, Some(1000), false, true);
  }

  #[test]
  fn finalize_pressed_only_mutates_pending_when_overlay_visible() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    assert_core_state(&sm, Some(1000), true, false);

    let hidden = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: false });
    assert!(hidden.is_empty());
    assert_core_state(&sm, Some(1000), true, false);

    let visible = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_eq!(visible, vec![InteractionEffect::FinalizeFromHotkey]);
    assert_core_state(&sm, Some(1000), true, true);
  }

  #[test]
  fn menu_toggle_does_not_mutate_interaction_state() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_core_state(&sm, Some(1000), true, true);

    let effects = sm.reduce(InteractionInput::MenuToggleAlwaysListening);
    assert_eq!(effects, vec![InteractionEffect::MenuToggleAlwaysListening]);
    assert_core_state(&sm, Some(1000), true, true);
  }

  #[test]
  fn speech_finalized_clears_pending_without_touching_other_fields() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_core_state(&sm, Some(1000), true, true);

    let effects = sm.reduce(InteractionInput::SpeechFinalized);
    assert!(effects.is_empty());
    assert_core_state(&sm, Some(1000), true, false);
  }

  #[test]
  fn speech_idle_clears_pending_only_when_hold_is_not_active() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_core_state(&sm, Some(1000), true, true);

    let effects = sm.reduce(InteractionInput::SpeechIdle { manual_hold_active: true });
    assert!(effects.is_empty());
    assert_core_state(&sm, Some(1000), true, true);

    let effects = sm.reduce(InteractionInput::SpeechIdle { manual_hold_active: false });
    assert!(effects.is_empty());
    assert_core_state(&sm, Some(1000), true, false);
  }

  #[test]
  fn overlay_cancelled_and_turn_reset_clear_finalize_flags_but_keep_press_timestamp() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_core_state(&sm, Some(1000), true, true);

    let effects = sm.reduce(InteractionInput::OverlayCancelled);
    assert!(effects.is_empty());
    assert_core_state(&sm, Some(1000), false, false);

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 2000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_core_state(&sm, Some(2000), true, true);

    let effects = sm.reduce(InteractionInput::TurnReset);
    assert!(effects.is_empty());
    assert_core_state(&sm, Some(2000), false, false);
  }

  #[test]
  fn session_reset_clears_press_timestamp_and_finalize_flags() {
    let mut sm = InteractionState::default();

    let _ = sm.reduce(InteractionInput::HoldPressed {
      now_ms: 1000,
      snapshot: snapshot(false, false, false, false, false),
    });
    let _ = sm.reduce(InteractionInput::FinalizePressed { overlay_visible: true });
    assert_core_state(&sm, Some(1000), true, true);

    let effects = sm.reduce(InteractionInput::SessionReset);
    assert!(effects.is_empty());
    assert_core_state(&sm, None, false, false);
  }
}
