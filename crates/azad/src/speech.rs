use std::sync::Arc;

use anyhow::Result;
use asr::embed::{
  SessionConfig, SessionControl, SessionEvent as ToonSessionEvent, SessionHandle,
  SessionSink as ToonSessionSink, spawn_session,
};
use asr::pipeline::{DebugStatsEvent, EngineState};

#[derive(Debug, Clone)]
pub enum SpeechEvent {
  SessionStarted {
    session_id: u64,
  },
  Listening {
    session_id: u64,
  },
  SpeechStartedByVad {
    session_id: u64,
  },
  /// Fires on every turn-start, regardless of reason. Used by the renderer
  /// to arm overlay state for `Manual` (force_start) turns that wouldn't
  /// otherwise fire `SpeechStartedByVad`.
  TurnStarted {
    session_id: u64,
    reason: asr::render::TurnStartedReason,
  },
  DraftUpdated {
    session_id: u64,
    turn_id: u64,
    committed: String,
    live: String,
  },
  Finalizing {
    session_id: u64,
    turn_id: u64,
    current_draft: String,
  },
  /// The tentative-finalize was undone by recovery. The handler should clear
  /// any "finalizing" UI state for `turn_id` and return the overlay to live
  /// listening — the user kept talking.
  FinalizingCancelled {
    session_id: u64,
    turn_id: u64,
  },
  FinalText {
    session_id: u64,
    turn_id: u64,
    text: String,
  },
  SessionEnded {
    session_id: u64,
  },
  Error {
    session_id: u64,
    message: String,
  },
  Status {
    session_id: u64,
    state: EngineState,
    detail: String,
  },
  Meter {
    session_id: u64,
    peak_db: f32,
    vad_speech: bool,
    vad_prob: f32,
  },
  DebugStats {
    session_id: u64,
    event: DebugStatsEvent,
  },
}

pub struct SpeechSession {
  pub session_id: u64,
  handle: Arc<dyn SessionHandle>,
}

impl SpeechSession {
  #[cfg(test)]
  pub(crate) fn test(session_id: u64) -> Self {
    Self { session_id, handle: Arc::new(TestSessionHandle) }
  }

  pub fn start_or_resume_manual_hold(&self) {
    let _ = self.handle.control(SessionControl::StartOrResumeManualHold);
  }

  pub fn release_manual_hold(&self) {
    let _ = self.handle.control(SessionControl::ReleaseManualHold);
  }

  pub fn set_auto_vad_enabled(&self, enabled: bool) {
    let _ = self.handle.control(SessionControl::SetAutoVadEnabled(enabled));
  }

  pub fn set_capture_enabled(&self, enabled: bool) {
    let _ = self.handle.control(SessionControl::SetCaptureEnabled(enabled));
  }

  pub fn set_debug_stats_enabled(&self, enabled: bool) {
    let _ = self.handle.control(SessionControl::SetDebugStatsEnabled(enabled));
  }

  pub fn finalize_current_turn(&self) {
    let _ = self.handle.control(SessionControl::FinalizeCurrentTurn);
  }

  pub fn cancel_current_turn(&self) {
    let _ = self.handle.control(SessionControl::CancelCurrentTurn);
  }

  pub fn cancel(&self) {
    let _ = self.handle.control(SessionControl::CancelSession);
  }

  pub fn capture_enabled(&self) -> bool {
    self.handle.capture_enabled()
  }
}

#[cfg(test)]
struct TestSessionHandle;

#[cfg(test)]
impl SessionHandle for TestSessionHandle {
  fn control(&self, _cmd: SessionControl) -> Result<()> {
    Ok(())
  }

  fn shutdown(&self) -> Result<()> {
    Ok(())
  }
}

pub fn spawn_speech_session(
  session_id: u64,
  cfg: SessionConfig,
  event_handler: Arc<dyn Fn(SpeechEvent) + Send + Sync>,
) -> Result<SpeechSession> {
  let sink: Arc<dyn ToonSessionSink> =
    Arc::new(ForwardingSink { session_id, handler: event_handler });
  let handle = spawn_session(cfg, sink)?;
  Ok(SpeechSession { session_id, handle })
}

struct ForwardingSink {
  session_id: u64,
  handler: Arc<dyn Fn(SpeechEvent) + Send + Sync>,
}

impl ToonSessionSink for ForwardingSink {
  fn on_event(&self, event: ToonSessionEvent) {
    let mapped = match event {
      ToonSessionEvent::SessionStarted => {
        SpeechEvent::SessionStarted { session_id: self.session_id }
      }
      ToonSessionEvent::Listening => SpeechEvent::Listening { session_id: self.session_id },
      ToonSessionEvent::SpeechStartedByVad => {
        SpeechEvent::SpeechStartedByVad { session_id: self.session_id }
      }
      ToonSessionEvent::TurnStarted { reason } => {
        SpeechEvent::TurnStarted { session_id: self.session_id, reason }
      }
      ToonSessionEvent::DraftUpdated { turn_id, committed, live } => {
        SpeechEvent::DraftUpdated { session_id: self.session_id, turn_id, committed, live }
      }
      ToonSessionEvent::Finalizing { turn_id, current_draft } => {
        SpeechEvent::Finalizing { session_id: self.session_id, turn_id, current_draft }
      }
      ToonSessionEvent::FinalizingCancelled { turn_id } => {
        SpeechEvent::FinalizingCancelled { session_id: self.session_id, turn_id }
      }
      ToonSessionEvent::FinalText { turn_id, text } => {
        SpeechEvent::FinalText { session_id: self.session_id, turn_id, text }
      }
      ToonSessionEvent::SessionEnded => SpeechEvent::SessionEnded { session_id: self.session_id },
      ToonSessionEvent::Error { message } => {
        SpeechEvent::Error { session_id: self.session_id, message }
      }
      ToonSessionEvent::Status { state, detail } => {
        SpeechEvent::Status { session_id: self.session_id, state, detail }
      }
      ToonSessionEvent::Meter { peak_db, vad_speech, vad_prob } => {
        SpeechEvent::Meter { session_id: self.session_id, peak_db, vad_speech, vad_prob }
      }
      ToonSessionEvent::DebugStats { event } => {
        SpeechEvent::DebugStats { session_id: self.session_id, event }
      }
    };
    (self.handler)(mapped);
  }
}
