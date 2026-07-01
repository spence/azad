pub mod cpal_input;
pub mod decoded_input;
pub mod wav_input;

use anyhow::Result;

#[derive(Debug, Clone, Copy)]
pub struct AudioSpec {
  pub sample_rate: u32,
  pub channels: u16,
}

#[derive(Debug, Clone)]
pub struct AudioChunk {
  pub start_frame: u64, // absolute, frames (not samples)
  pub frames: Vec<f32>, // interleaved, len = frames * channels
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AudioHealth {
  pub produced_frames: u64,
  pub dropped_frames: u64,
  pub backlog_frames: u64,
  pub wallclock_gap_frames: u64,
  pub worst_wallclock_gap_frames: u64,
  pub worst_backlog_frames: u64,
}

pub trait AudioInput {
  fn spec(&self) -> AudioSpec;
  fn read_chunk(&mut self) -> Result<Option<AudioChunk>>;
  fn health(&self) -> AudioHealth;
}
