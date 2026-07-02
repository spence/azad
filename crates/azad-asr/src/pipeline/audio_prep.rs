use crate::audio::{AudioHealth, AudioSpec};

use super::AudioHealthView;

pub(super) fn levels_dbfs(samples: &[f32]) -> (f32, f32) {
  if samples.is_empty() {
    return (-120.0, -120.0);
  }

  let mut sum_sq = 0.0f64;
  let mut peak = 0.0f32;
  for &s in samples {
    let v = if s.is_finite() { s } else { 0.0 };
    let a = v.abs();
    if a > peak {
      peak = a;
    }
    sum_sq += (v as f64) * (v as f64);
  }
  let rms = (sum_sq / samples.len() as f64).sqrt() as f32;

  let rms_db = if rms <= 0.0 { -120.0 } else { 20.0 * rms.log10() };
  let peak_db = if peak <= 0.0 { -120.0 } else { 20.0 * peak.log10() };

  (rms_db, peak_db)
}

pub(super) fn health_to_view(h: AudioHealth, spec: AudioSpec) -> AudioHealthView {
  let sr = spec.sample_rate.max(1) as u64;
  AudioHealthView {
    gap_ms: (h.wallclock_gap_frames * 1000) / sr,
    worst_gap_ms: (h.worst_wallclock_gap_frames * 1000) / sr,
    dropped_ms: (h.dropped_frames * 1000) / sr,
    backlog_ms: (h.backlog_frames * 1000) / sr,
    worst_backlog_ms: (h.worst_backlog_frames * 1000) / sr,
  }
}

pub(super) fn round_up_to_chunk(n: usize, chunk: usize) -> usize {
  if chunk == 0 {
    return n;
  }
  if n % chunk == 0 { n } else { n + (chunk - (n % chunk)) }
}

#[derive(Default)]
pub(super) struct SampleQueue {
  buf: Vec<f32>,
  off: usize,
}

impl SampleQueue {
  pub(super) fn available(&self) -> usize {
    self.buf.len().saturating_sub(self.off)
  }

  pub(super) fn push(&mut self, xs: &[f32]) {
    self.buf.extend_from_slice(xs);
  }

  pub(super) fn push_zeros(&mut self, n: usize) {
    if n == 0 {
      return;
    }
    self.buf.extend(std::iter::repeat_n(0.0f32, n));
  }

  pub(super) fn peek(&self, n: usize) -> &[f32] {
    let n = n.min(self.available());
    &self.buf[self.off..self.off + n]
  }

  pub(super) fn pop(&mut self, n: usize) {
    let n = n.min(self.available());
    self.off += n;
    if self.off > 32_768 {
      self.compact();
    }
  }

  fn compact(&mut self) {
    if self.off == 0 {
      return;
    }
    self.buf.copy_within(self.off.., 0);
    let new_len = self.buf.len().saturating_sub(self.off);
    self.buf.truncate(new_len);
    self.off = 0;
  }
}

pub(super) struct AudioPrep {
  in_spec: AudioSpec,
  out_sr: u32,

  // Scratch buffer for downmix (mono at in_sr).
  mono: Vec<f32>,
  resampler: LinearResampler,
}

impl AudioPrep {
  pub(super) fn new(in_spec: AudioSpec, out_sr: u32) -> Self {
    let resampler = LinearResampler::new(in_spec.sample_rate, out_sr);
    Self { in_spec, out_sr, mono: Vec::new(), resampler }
  }

  pub(super) fn process_interleaved_into(&mut self, interleaved: &[f32], out: &mut SampleQueue) {
    let ch = self.in_spec.channels.max(1) as usize;

    self.mono.clear();
    self.mono.reserve(interleaved.len() / ch);

    if ch == 1 {
      for &s in interleaved {
        self.mono.push(if s.is_finite() { s } else { 0.0 });
      }
    } else {
      for frame in interleaved.chunks_exact(ch) {
        let mut sum = 0.0f32;
        for &s in frame {
          sum += if s.is_finite() { s } else { 0.0 };
        }
        self.mono.push(sum / (ch as f32));
      }
    }

    if self.in_spec.sample_rate == self.out_sr {
      out.push(&self.mono);
      return;
    }

    self.resampler.push(&self.mono);
    self.resampler.pull_into(out);
  }
}

struct LinearResampler {
  step: f64, // input samples per output sample
  pos: f64,  // position (in input samples) relative to `off`
  buf: Vec<f32>,
  off: usize,
  tmp: Vec<f32>,
}

impl LinearResampler {
  fn new(in_sr: u32, out_sr: u32) -> Self {
    let in_sr = in_sr.max(1);
    let out_sr = out_sr.max(1);
    Self { step: in_sr as f64 / out_sr as f64, pos: 0.0, buf: Vec::new(), off: 0, tmp: Vec::new() }
  }

  fn push(&mut self, xs: &[f32]) {
    self.buf.extend_from_slice(xs);
  }

  fn pull_into(&mut self, out: &mut SampleQueue) {
    let avail = self.buf.len().saturating_sub(self.off);
    if avail < 2 {
      return;
    }

    self.tmp.clear();
    // Upper bound: in worst case we output about `avail / step` samples.
    let est = ((avail as f64) / self.step).ceil() as usize;
    self.tmp.reserve(est.min(16_384));

    while self.pos + 1.0 < (avail as f64) {
      let i0 = self.pos.floor() as usize;
      let frac = self.pos - (i0 as f64);
      let a = self.buf[self.off + i0];
      let b = self.buf[self.off + i0 + 1];
      let y = a as f64 + (b as f64 - a as f64) * frac;
      self.tmp.push(y as f32);
      self.pos += self.step;
    }

    out.push(&self.tmp);

    let drop = self.pos.floor() as usize;
    self.off = self.off.saturating_add(drop);
    self.pos -= drop as f64;

    self.compact();
  }

  fn compact(&mut self) {
    if self.off == 0 {
      return;
    }
    if self.off > 16_384 {
      self.buf.copy_within(self.off.., 0);
      let new_len = self.buf.len().saturating_sub(self.off);
      self.buf.truncate(new_len);
      self.off = 0;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{AudioPrep, SampleQueue, round_up_to_chunk};
  use crate::audio::AudioSpec;

  #[test]
  fn sample_queue_pop_and_compact_preserves_remaining_samples() {
    let mut q = SampleQueue::default();
    q.push(&[1.0, 2.0, 3.0, 4.0]);
    q.pop(2);
    q.compact();
    assert_eq!(q.peek(4), &[3.0, 4.0]);
    assert_eq!(q.available(), 2);
  }

  #[test]
  fn audio_prep_downmixes_and_sanitizes_non_finite_samples() {
    let spec = AudioSpec { sample_rate: 16_000, channels: 2 };
    let mut prep = AudioPrep::new(spec, 16_000);
    let mut out = SampleQueue::default();
    prep.process_interleaved_into(&[1.0, f32::NAN, f32::INFINITY, -1.0], &mut out);
    assert_eq!(out.peek(8), &[0.5, -0.5]);
  }

  #[test]
  fn audio_prep_identity_sample_rate_pushes_all_mono_samples() {
    let spec = AudioSpec { sample_rate: 16_000, channels: 1 };
    let mut prep = AudioPrep::new(spec, 16_000);
    let mut out = SampleQueue::default();
    prep.process_interleaved_into(&[0.0, 0.25, -0.25], &mut out);
    assert_eq!(out.peek(8), &[0.0, 0.25, -0.25]);
  }

  #[test]
  fn audio_prep_downsamples_48k_to_16k_shape() {
    let spec = AudioSpec { sample_rate: 48_000, channels: 1 };
    let mut prep = AudioPrep::new(spec, 16_000);
    let mut out = SampleQueue::default();
    prep.process_interleaved_into(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &mut out);
    assert_eq!(out.peek(8), &[0.0, 3.0]);
  }

  #[test]
  fn round_up_to_chunk_handles_zero_and_partial_chunks() {
    assert_eq!(round_up_to_chunk(7, 0), 7);
    assert_eq!(round_up_to_chunk(8, 4), 8);
    assert_eq!(round_up_to_chunk(9, 4), 12);
  }
}
