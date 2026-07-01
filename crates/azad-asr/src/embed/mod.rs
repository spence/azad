use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use crate::audio::cpal_input::{CpalInput, CpalInputConfig};
use crate::devices::{open_default_input_device, open_input_device_by_id};
use crate::pipeline::{
  DebugStatsEvent, EngineState, PipelineConfig, PipelineControls, PipelineRunOptions,
  run_pipeline_with_options,
};
use crate::render::{RenderEvent, Renderer};

#[derive(Debug, Clone)]
pub struct SessionConfig {
  pub device_id: Option<String>,
  pub chunk_ms: u32,
  pub buffer_ms: u32,
  pub auto_vad_enabled: bool,
  pub capture_enabled: bool,
  pub debug_stats_enabled: bool,
  pub native_engine_logs_enabled: bool,
  pub pipeline: PipelineConfig,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
  SessionStarted,
  Listening,
  SpeechStartedByVad,
  /// Fires on every turn-start, regardless of reason. Used by the renderer
  /// to arm overlay state for `Manual` (force_start) turns that wouldn't
  /// otherwise fire `SpeechStartedByVad`. Both events fire for `Vad` starts.
  TurnStarted {
    reason: crate::render::TurnStartedReason,
  },
  DraftUpdated {
    turn_id: u64,
    committed: String,
    live: String,
  },
  Finalizing {
    turn_id: u64,
    current_draft: String,
  },
  /// Pulses the finalize state off — the consumer should clear "finalizing" UI
  /// (border pulse, deadline tracking) for this turn and return to the live
  /// listening overlay. Emitted when a tentative-finalize is undone by the
  /// recovery window.
  FinalizingCancelled {
    turn_id: u64,
  },
  FinalText {
    turn_id: u64,
    text: String,
  },
  SessionEnded,
  Error {
    message: String,
  },
  Status {
    state: EngineState,
    detail: String,
  },
  Meter {
    peak_db: f32,
    vad_speech: bool,
    vad_prob: f32,
  },
  DebugStats {
    event: DebugStatsEvent,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionControl {
  StartOrResumeManualHold,
  ReleaseManualHold,
  SetAutoVadEnabled(bool),
  SetCaptureEnabled(bool),
  SetDebugStatsEnabled(bool),
  FinalizeCurrentTurn,
  CancelCurrentTurn,
  CancelSession,
}

pub trait SessionSink: Send + Sync {
  fn on_event(&self, event: SessionEvent);
}

pub trait SessionHandle: Send + Sync {
  fn control(&self, cmd: SessionControl) -> Result<()>;
  fn shutdown(&self) -> Result<()>;
  /// Read-only view of the engine's `capture_enabled` flag. Returns
  /// `false` for handles that don't have a live `PipelineControls`
  /// (e.g. file-driven embed sessions used in tests).
  fn capture_enabled(&self) -> bool {
    false
  }
}

pub fn spawn_session(
  cfg: SessionConfig,
  sink: Arc<dyn SessionSink>,
) -> Result<Arc<dyn SessionHandle>> {
  crate::logging::set_native_logging_enabled(cfg.native_engine_logs_enabled);
  eprintln!(
    "asr: native engine logs {}",
    if cfg.native_engine_logs_enabled { "enabled" } else { "suppressed" }
  );

  let controls = Arc::new(PipelineControls::default());
  controls.set_auto_vad_enabled(cfg.auto_vad_enabled);
  controls.set_capture_enabled(cfg.capture_enabled);
  controls.set_debug_stats_enabled(cfg.debug_stats_enabled);
  let shutdown = Arc::new(AtomicBool::new(false));

  let handle: Arc<dyn SessionHandle> = Arc::new(LiveSessionHandle {
    controls: Arc::clone(&controls),
    shutdown: Arc::clone(&shutdown),
  });

  std::thread::spawn(move || {
    sink.on_event(SessionEvent::SessionStarted);

    let run_result: Result<()> = (|| {
      let device = match cfg.device_id.as_deref() {
        Some(id) => open_input_device_by_id(id)?,
        None => open_default_input_device()?,
      };

      let mut input = CpalInput::open_with_device(
        device,
        CpalInputConfig {
          chunk_ms: cfg.chunk_ms.max(1),
          buffer_ms: cfg.buffer_ms.max(1000),
          capture_enabled: Some(Arc::clone(&controls)),
          shutdown: Some(Arc::clone(&shutdown)),
        },
      )
      .context("failed to open microphone capture")?;

      let renderer: Arc<dyn Renderer> = Arc::new(SinkRenderer { sink: Arc::clone(&sink) });
      run_pipeline_with_options(
        &mut input,
        renderer,
        cfg.pipeline,
        Arc::clone(&shutdown),
        PipelineRunOptions {
          controls: Some(Arc::clone(&controls)),
          // Keep capture alive across turns so the embedding app can stay
          // continuously listening without tearing down/reopening the device.
          stop_after_turn: false,
        },
      )
    })();

    if let Err(err) = run_result {
      sink.on_event(SessionEvent::Error { message: err.to_string() });
    }

    sink.on_event(SessionEvent::SessionEnded);
  });

  Ok(handle)
}

struct LiveSessionHandle {
  controls: Arc<PipelineControls>,
  shutdown: Arc<AtomicBool>,
}

impl SessionHandle for LiveSessionHandle {
  fn control(&self, cmd: SessionControl) -> Result<()> {
    match cmd {
      SessionControl::StartOrResumeManualHold => {
        self.controls.set_manual_hold_active(true);
        self.controls.request_force_start();
      }
      SessionControl::ReleaseManualHold => {
        self.controls.set_manual_hold_active(false);
      }
      SessionControl::SetAutoVadEnabled(enabled) => {
        self.controls.set_auto_vad_enabled(enabled);
      }
      SessionControl::SetCaptureEnabled(enabled) => {
        self.controls.set_capture_enabled(enabled);
      }
      SessionControl::SetDebugStatsEnabled(enabled) => {
        self.controls.set_debug_stats_enabled(enabled);
      }
      SessionControl::FinalizeCurrentTurn => {
        self.controls.request_force_finish();
      }
      SessionControl::CancelCurrentTurn => {
        self.controls.set_manual_hold_active(false);
        self.controls.request_cancel_turn();
      }
      SessionControl::CancelSession => {
        self.controls.set_manual_hold_active(false);
        self.shutdown.store(true, Ordering::Relaxed);
      }
    }
    Ok(())
  }

  fn shutdown(&self) -> Result<()> {
    self.shutdown.store(true, Ordering::Relaxed);
    Ok(())
  }

  fn capture_enabled(&self) -> bool {
    self.controls.capture_enabled()
  }
}

struct SinkRenderer {
  sink: Arc<dyn SessionSink>,
}

impl Renderer for SinkRenderer {
  fn emit(&self, ev: RenderEvent) {
    match ev {
      RenderEvent::Status(v) => {
        self
          .sink
          .on_event(SessionEvent::Status { state: v.state, detail: v.detail.clone() });
        if v.state == EngineState::Speech {
          self.sink.on_event(SessionEvent::Listening);
        }
      }
      RenderEvent::SpeechStartedByVad => {
        self.sink.on_event(SessionEvent::SpeechStartedByVad);
      }
      RenderEvent::TurnStarted { reason } => {
        self.sink.on_event(SessionEvent::TurnStarted { reason });
      }
      RenderEvent::Active { id, committed, live } => {
        self.sink.on_event(SessionEvent::DraftUpdated { turn_id: id, committed, live });
      }
      RenderEvent::Finalizing { id, text } => {
        self
          .sink
          .on_event(SessionEvent::Finalizing { turn_id: id, current_draft: text });
      }
      RenderEvent::FinalizingCancelled { id } => {
        self.sink.on_event(SessionEvent::FinalizingCancelled { turn_id: id });
      }
      RenderEvent::FinalLine { id, text } => {
        self.sink.on_event(SessionEvent::DraftUpdated {
          turn_id: id,
          committed: text,
          live: String::new(),
        });
      }
      RenderEvent::ReplaceLine { id, text } => {
        self.sink.on_event(SessionEvent::FinalText { turn_id: id, text });
      }
      RenderEvent::Error { message } => {
        self.sink.on_event(SessionEvent::Error { message });
      }
      RenderEvent::Meter(v) => {
        self.sink.on_event(SessionEvent::Meter {
          peak_db: v.peak_db,
          vad_speech: v.vad_speech,
          vad_prob: v.vad_prob,
        });
      }
      RenderEvent::DebugStats(event) => {
        self.sink.on_event(SessionEvent::DebugStats { event });
      }
      RenderEvent::CaptureHealth(_) => {}
    }
  }
}
