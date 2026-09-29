//! Derives the capture helper's key context from Azad's interaction surfaces.
//!
//! Shared by the app and the isolated interaction harness so both drive the helper's policy
//! from the same overlay state.

use azad_capture::policy::KeyContext;

/// Which Azad surfaces currently own keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeySurfaces {
  /// Listen chord modifier mask (`MOD_*`); Space with these held is the listen hotkey.
  pub listen_modifiers: u8,
  /// The overlay is up: Escape cancels, Enter finalizes, Up/Down navigate.
  pub overlay: bool,
  /// History browsing: Left collapses and Right expands the selection.
  pub history: bool,
  /// The history search field takes typing.
  pub search_input: bool,
}

pub fn key_context(surfaces: KeySurfaces) -> KeyContext {
  KeyContext {
    listen_modifiers: surfaces.listen_modifiers,
    escape: surfaces.overlay,
    enter: surfaces.overlay,
    arrows: surfaces.overlay,
    arrow_left: surfaces.history,
    arrow_right: surfaces.history,
    search_input: surfaces.search_input,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hidden_overlay_claims_only_the_listen_chord() {
    let context = key_context(KeySurfaces { listen_modifiers: 4, ..KeySurfaces::default() });
    assert_eq!(context, KeyContext { listen_modifiers: 4, ..KeyContext::default() });
  }

  #[test]
  fn history_adds_left_right_and_search() {
    let context = key_context(KeySurfaces {
      listen_modifiers: 4,
      overlay: true,
      history: true,
      search_input: true,
    });
    assert!(context.escape && context.enter && context.arrows);
    assert!(context.arrow_left && context.arrow_right && context.search_input);
  }
}
