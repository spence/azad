pub const HOLD_DOUBLE_TAP_WINDOW_MS: u64 = 450;

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
pub enum HotkeyInput {
    HoldPressed {
        now_ms: u64,
        snapshot: RuntimeSnapshot,
    },
    HoldReleased {
        snapshot: RuntimeSnapshot,
    },
    FinalizePressed {
        overlay_visible: bool,
    },
    MenuToggleAlwaysListening,
    SpeechFinalized,
    SpeechIdle {
        manual_hold_active: bool,
    },
    OverlayCancelled,
    TurnReset,
    SessionReset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEffect {
    ToggleAlwaysListening,
    CompletePureToggleGesture,
    ActivateManualHold {
        reset_turn_state: bool,
        release_should_finalize: bool,
    },
    ReleaseManualHold {
        should_finalize: bool,
        has_started_turn: bool,
    },
    FinalizeFromHotkey,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HotkeyState {
    last_hold_press_at_ms: Option<u64>,
    release_should_finalize: bool,
    manual_finalize_pending: bool,
}

impl HotkeyState {
    #[cfg(test)]
    pub fn manual_finalize_pending(&self) -> bool {
        self.manual_finalize_pending
    }

    pub fn reduce(&mut self, input: HotkeyInput) -> Vec<HotkeyEffect> {
        match input {
            HotkeyInput::HoldPressed { now_ms, snapshot } => {
                let is_double_tap = self
                    .last_hold_press_at_ms
                    .is_some_and(|last| now_ms.saturating_sub(last) <= HOLD_DOUBLE_TAP_WINDOW_MS);
                self.last_hold_press_at_ms = Some(now_ms);

                let has_turn_context = snapshot.has_turn_context || self.manual_finalize_pending;
                let should_toggle_always_listening =
                    is_double_tap && (snapshot.always_listening_enabled || !has_turn_context);

                let mut effects = Vec::new();
                if should_toggle_always_listening {
                    effects.push(HotkeyEffect::ToggleAlwaysListening);
                }

                // Pure mode-toggle gesture from idle context should not also begin hold mode.
                if should_toggle_always_listening && !has_turn_context {
                    self.release_should_finalize = false;
                    self.manual_finalize_pending = false;
                    effects.push(HotkeyEffect::CompletePureToggleGesture);
                    return effects;
                }

                let preserve_active_turn = self.manual_finalize_pending
                    || (snapshot.always_listening_enabled && snapshot.has_active_speech_turn);
                let toggled_off_active_vad_turn = should_toggle_always_listening
                    && snapshot.always_listening_enabled
                    && snapshot.has_active_speech_turn;
                let release_should_finalize = self.manual_finalize_pending
                    || !preserve_active_turn
                    || toggled_off_active_vad_turn;
                self.release_should_finalize = release_should_finalize;
                self.manual_finalize_pending = false;

                effects.push(HotkeyEffect::ActivateManualHold {
                    reset_turn_state: !preserve_active_turn,
                    release_should_finalize,
                });
                effects
            }
            HotkeyInput::HoldReleased { snapshot } => {
                let should_finalize = self.release_should_finalize;
                self.release_should_finalize = false;
                // Keep "pending finalize context" only when real turn context exists.
                // A manual-hold press/release used as a pure double-tap gesture should not
                // poison the next press by forcing has_turn_context=true.
                self.manual_finalize_pending = should_finalize && snapshot.has_turn_context;

                vec![HotkeyEffect::ReleaseManualHold {
                    should_finalize,
                    has_started_turn: snapshot.has_started_turn,
                }]
            }
            HotkeyInput::FinalizePressed { overlay_visible } => {
                if !overlay_visible {
                    return Vec::new();
                }
                self.manual_finalize_pending = true;
                vec![HotkeyEffect::FinalizeFromHotkey]
            }
            HotkeyInput::MenuToggleAlwaysListening => vec![HotkeyEffect::ToggleAlwaysListening],
            HotkeyInput::SpeechFinalized => {
                self.manual_finalize_pending = false;
                Vec::new()
            }
            HotkeyInput::SpeechIdle { manual_hold_active } => {
                if !manual_hold_active {
                    self.manual_finalize_pending = false;
                }
                Vec::new()
            }
            HotkeyInput::OverlayCancelled | HotkeyInput::TurnReset => {
                self.release_should_finalize = false;
                self.manual_finalize_pending = false;
                Vec::new()
            }
            HotkeyInput::SessionReset => {
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
        HotkeyEffect, HotkeyInput, HotkeyState, RuntimeSnapshot, HOLD_DOUBLE_TAP_WINDOW_MS,
    };

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
        let mut sm = HotkeyState::default();

        let first_press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            first_press,
            vec![HotkeyEffect::ActivateManualHold {
                reset_turn_state: true,
                release_should_finalize: true,
            }]
        );

        let _ = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, false, false, false),
        });

        let second_press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000 + HOLD_DOUBLE_TAP_WINDOW_MS - 1,
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            second_press,
            vec![
                HotkeyEffect::ToggleAlwaysListening,
                HotkeyEffect::CompletePureToggleGesture,
            ]
        );
    }

    #[test]
    fn active_vad_double_tap_transitions_to_manual_finalize_on_release() {
        let mut sm = HotkeyState::default();

        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(true, true, true, true, true),
        });

        let second_press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1100,
            snapshot: snapshot(true, true, true, true, true),
        });
        assert_eq!(
            second_press,
            vec![
                HotkeyEffect::ToggleAlwaysListening,
                HotkeyEffect::ActivateManualHold {
                    reset_turn_state: false,
                    release_should_finalize: true,
                },
            ]
        );

        let release = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, true, true, true, true),
        });
        assert_eq!(
            release,
            vec![HotkeyEffect::ReleaseManualHold {
                should_finalize: true,
                has_started_turn: true,
            }]
        );
    }

    #[test]
    fn vad_off_rapid_repress_preserves_pending_finalize_context() {
        let mut sm = HotkeyState::default();

        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });
        let first_release = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, true, true, true),
        });
        assert_eq!(
            first_release,
            vec![HotkeyEffect::ReleaseManualHold {
                should_finalize: true,
                has_started_turn: true,
            }]
        );
        assert!(sm.manual_finalize_pending());

        let second_press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1100,
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            second_press,
            vec![HotkeyEffect::ActivateManualHold {
                reset_turn_state: false,
                release_should_finalize: true,
            }]
        );

        let _ = sm.reduce(HotkeyInput::SpeechFinalized);
        assert!(!sm.manual_finalize_pending());

        let second_release = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, true, true, true),
        });
        assert_eq!(
            second_release,
            vec![HotkeyEffect::ReleaseManualHold {
                should_finalize: true,
                has_started_turn: true,
            }]
        );
    }

    #[test]
    fn no_speech_release_does_not_mark_finalize_pending() {
        let mut sm = HotkeyState::default();
        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });

        let release = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            release,
            vec![HotkeyEffect::ReleaseManualHold {
                should_finalize: true,
                has_started_turn: false,
            }]
        );
        assert!(!sm.manual_finalize_pending());
    }

    #[test]
    fn manual_hold_only_release_does_not_block_idle_double_tap_toggle() {
        let mut sm = HotkeyState::default();
        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });

        // Manual-hold can report "started turn" before any concrete turn context exists.
        // That should not poison the next press and block idle double-tap toggle.
        let first_release = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, false, true, true),
        });
        assert_eq!(
            first_release,
            vec![HotkeyEffect::ReleaseManualHold {
                should_finalize: true,
                has_started_turn: true,
            }]
        );
        assert!(!sm.manual_finalize_pending());

        let second_press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000 + HOLD_DOUBLE_TAP_WINDOW_MS - 1,
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            second_press,
            vec![
                HotkeyEffect::ToggleAlwaysListening,
                HotkeyEffect::CompletePureToggleGesture,
            ]
        );
    }

    #[test]
    fn menu_toggle_emits_toggle_effect() {
        let mut sm = HotkeyState::default();
        let effects = sm.reduce(HotkeyInput::MenuToggleAlwaysListening);
        assert_eq!(effects, vec![HotkeyEffect::ToggleAlwaysListening]);
    }

    #[test]
    fn speech_idle_clears_manual_finalize_pending() {
        let mut sm = HotkeyState::default();
        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });
        let _ = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, true, true, true),
        });
        assert!(sm.manual_finalize_pending());

        let _ = sm.reduce(HotkeyInput::SpeechIdle {
            manual_hold_active: false,
        });
        assert!(!sm.manual_finalize_pending());
    }

    #[test]
    fn active_vad_single_press_release_is_assist_only() {
        let mut sm = HotkeyState::default();

        let press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(true, true, true, true, true),
        });
        assert_eq!(
            press,
            vec![HotkeyEffect::ActivateManualHold {
                reset_turn_state: false,
                release_should_finalize: false,
            }]
        );

        let release = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(true, true, true, true, true),
        });
        assert_eq!(
            release,
            vec![HotkeyEffect::ReleaseManualHold {
                should_finalize: false,
                has_started_turn: true,
            }]
        );
        assert!(!sm.manual_finalize_pending());
    }

    #[test]
    fn double_tap_window_expiry_prevents_toggle() {
        let mut sm = HotkeyState::default();

        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });
        let _ = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, false, false, false),
        });

        let press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000 + HOLD_DOUBLE_TAP_WINDOW_MS + 1,
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            press,
            vec![HotkeyEffect::ActivateManualHold {
                reset_turn_state: true,
                release_should_finalize: true,
            }]
        );
    }

    #[test]
    fn double_tap_from_vad_on_idle_toggles_without_starting_hold() {
        let mut sm = HotkeyState::default();

        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(true, false, false, false, false),
        });
        let _ = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(true, false, false, false, false),
        });

        let second_press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000 + HOLD_DOUBLE_TAP_WINDOW_MS - 1,
            snapshot: snapshot(true, false, false, false, false),
        });
        assert_eq!(
            second_press,
            vec![
                HotkeyEffect::ToggleAlwaysListening,
                HotkeyEffect::CompletePureToggleGesture,
            ]
        );
    }

    #[test]
    fn session_reset_breaks_double_tap_chain() {
        let mut sm = HotkeyState::default();

        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });
        let _ = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, false, false, false),
        });
        let _ = sm.reduce(HotkeyInput::SessionReset);

        let press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000 + HOLD_DOUBLE_TAP_WINDOW_MS - 1,
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            press,
            vec![HotkeyEffect::ActivateManualHold {
                reset_turn_state: true,
                release_should_finalize: true,
            }]
        );
    }

    #[test]
    fn finalize_pressed_requires_overlay_visibility() {
        let mut sm = HotkeyState::default();

        let hidden = sm.reduce(HotkeyInput::FinalizePressed {
            overlay_visible: false,
        });
        assert!(hidden.is_empty());
        assert!(!sm.manual_finalize_pending());

        let visible = sm.reduce(HotkeyInput::FinalizePressed {
            overlay_visible: true,
        });
        assert_eq!(visible, vec![HotkeyEffect::FinalizeFromHotkey]);
        assert!(sm.manual_finalize_pending());
    }

    #[test]
    fn speech_idle_with_manual_hold_keeps_finalize_pending() {
        let mut sm = HotkeyState::default();
        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });
        let _ = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, true, true, true),
        });
        assert!(sm.manual_finalize_pending());

        let _ = sm.reduce(HotkeyInput::SpeechIdle {
            manual_hold_active: true,
        });
        assert!(sm.manual_finalize_pending());
    }

    #[test]
    fn overlay_cancelled_clears_release_finalize_before_release() {
        let mut sm = HotkeyState::default();
        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });

        let _ = sm.reduce(HotkeyInput::OverlayCancelled);

        let release = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, true, true, true),
        });
        assert_eq!(
            release,
            vec![HotkeyEffect::ReleaseManualHold {
                should_finalize: false,
                has_started_turn: true,
            }]
        );
        assert!(!sm.manual_finalize_pending());
    }

    #[test]
    fn turn_reset_clears_pending_finalize_context() {
        let mut sm = HotkeyState::default();
        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, false, false, false),
        });
        let _ = sm.reduce(HotkeyInput::HoldReleased {
            snapshot: snapshot(false, false, true, true, true),
        });
        assert!(sm.manual_finalize_pending());

        let _ = sm.reduce(HotkeyInput::TurnReset);
        assert!(!sm.manual_finalize_pending());

        let press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000 + HOLD_DOUBLE_TAP_WINDOW_MS + 1,
            snapshot: snapshot(false, false, false, false, false),
        });
        assert_eq!(
            press,
            vec![HotkeyEffect::ActivateManualHold {
                reset_turn_state: true,
                release_should_finalize: true,
            }]
        );
    }

    #[test]
    fn vad_off_double_tap_with_turn_context_does_not_toggle() {
        let mut sm = HotkeyState::default();

        let _ = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1000,
            snapshot: snapshot(false, false, true, true, true),
        });
        let second_press = sm.reduce(HotkeyInput::HoldPressed {
            now_ms: 1100,
            snapshot: snapshot(false, false, true, true, true),
        });
        assert_eq!(
            second_press,
            vec![HotkeyEffect::ActivateManualHold {
                reset_turn_state: true,
                release_should_finalize: true,
            }]
        );
    }
}
