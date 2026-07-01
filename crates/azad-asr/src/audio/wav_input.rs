use crate::audio::{AudioChunk, AudioHealth, AudioInput, AudioSpec};
use anyhow::{Context, Result, anyhow};
use hound::{SampleFormat, WavReader};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub struct WavInput {
  spec: AudioSpec,
  chunk_frames: usize,
  start_frame: u64,
  produced_frames: u64,
  reader: WavReader<BufReader<File>>,
  sample_format: SampleFormat,
  bits_per_sample: u16,
}

impl WavInput {
  pub fn open(path: impl AsRef<Path>, chunk_ms: u32) -> Result<Self> {
    let path = path.as_ref();
    let reader =
      WavReader::open(path).with_context(|| format!("failed to open wav: {}", path.display()))?;
    let spec0 = reader.spec();

    if spec0.sample_rate == 0 || spec0.channels == 0 {
      return Err(anyhow!("invalid wav spec (sr/ch=0)"));
    }

    let chunk_frames = ((spec0.sample_rate as u64) * (chunk_ms as u64) / 1000).max(1) as usize;

    Ok(Self {
      spec: AudioSpec { sample_rate: spec0.sample_rate, channels: spec0.channels },
      chunk_frames,
      start_frame: 0,
      produced_frames: 0,
      sample_format: spec0.sample_format,
      bits_per_sample: spec0.bits_per_sample,
      reader,
    })
  }
}

impl AudioInput for WavInput {
  fn spec(&self) -> AudioSpec {
    self.spec
  }

  fn read_chunk(&mut self) -> Result<Option<AudioChunk>> {
    let channels = self.spec.channels as usize;
    let want_samples = self.chunk_frames * channels;

    let mut frames = Vec::with_capacity(want_samples);

    match self.sample_format {
      SampleFormat::Float => {
        for s in self.reader.samples::<f32>().take(want_samples) {
          frames.push(s?);
        }
      }
      SampleFormat::Int => {
        // Prefer the common 16-bit path.
        if self.bits_per_sample <= 16 {
          for s in self.reader.samples::<i16>().take(want_samples) {
            let v = s? as f32 / 32768.0;
            frames.push(v);
          }
        } else {
          // Fallback for 24/32-bit PCM: scale by full-scale range.
          let denom = (1u64 << (self.bits_per_sample.saturating_sub(1) as u32)) as f32;
          for s in self.reader.samples::<i32>().take(want_samples) {
            let v = s? as f32 / denom;
            frames.push(v.clamp(-1.0, 1.0));
          }
        }
      }
    }

    if frames.is_empty() {
      return Ok(None);
    }

    let n_frames = frames.len() / channels;
    let chunk = AudioChunk { start_frame: self.start_frame, frames };
    self.start_frame = self.start_frame.saturating_add(n_frames as u64);
    self.produced_frames = self.produced_frames.saturating_add(n_frames as u64);
    Ok(Some(chunk))
  }

  fn health(&self) -> AudioHealth {
    AudioHealth {
      produced_frames: self.produced_frames,
      dropped_frames: 0,
      backlog_frames: 0,
      wallclock_gap_frames: 0,
      worst_wallclock_gap_frames: 0,
      worst_backlog_frames: 0,
    }
  }
}
