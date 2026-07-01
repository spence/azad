use crate::pipeline::{AudioHealthView, DebugStatsEvent, MeterView, StatusView};

/// Why the engine started a turn. Crosses the renderer boundary so consumers
/// can show different UI affordances for VAD-detected vs manual-trigger
/// starts. Distinct from the engine-internal `TurnStartReason` so we don't
/// leak pipeline-internal types across modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStartedReason {
  /// VAD threshold confirmed speech.
  Vad,
  /// Force-start path — `request_force_start` was consumed (manual hold,
  /// push-to-talk, or any other code path that explicitly requests a turn
  /// without waiting for VAD).
  Manual,
}

#[derive(Debug, Clone)]
pub enum RenderEvent {
  Status(StatusView),
  SpeechStartedByVad,
  /// Fires on every turn-start, regardless of `TurnStartReason`. Pairs with
  /// `SpeechStartedByVad` (which only fires for VAD-detected starts) so the
  /// renderer can arm overlay state for manual-trigger turns that would
  /// otherwise have no engine-side cue.
  TurnStarted {
    reason: TurnStartedReason,
  },
  CaptureHealth(AudioHealthView),
  Meter(MeterView),
  Active {
    id: u64,
    committed: String,
    live: String,
  },
  Finalizing {
    id: u64,
    text: String,
  },
  /// The finalize started by a tentative-finalize entry was undone — the user
  /// kept talking. The consumer should clear any "finalizing" UI state for this
  /// turn (e.g. the pulsing border) and return to the live listening overlay.
  FinalizingCancelled {
    id: u64,
  },
  FinalLine {
    id: u64,
    text: String,
  },
  ReplaceLine {
    id: u64,
    text: String,
  },
  DebugStats(DebugStatsEvent),
  Error {
    message: String,
  },
}

pub trait Renderer: Send + Sync {
  fn emit(&self, ev: RenderEvent);
}
