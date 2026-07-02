use crate::audio::{AudioChunk, AudioHealth, AudioInput, AudioSpec};
use crate::pipeline::PipelineControls;
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PREFERRED_INPUT_BUFFER_FRAMES: u32 = 2048;

/// Upper bound on how long `read_chunk` blocks between wake signals. The
/// consumer is normally woken by the CPAL callback on audio or by
/// `set_capture_enabled` on a state flip; this timeout bounds lost wakeups and
/// CLI replay paths without `PipelineControls`.
const WAKE_BACKSTOP: Duration = Duration::from_millis(50);

fn preferred_fixed_buffer_frames(range_min: u32, range_max: u32) -> Option<u32> {
  if range_min == 0 || range_max == 0 || range_min > range_max {
    return None;
  }
  Some(PREFERRED_INPUT_BUFFER_FRAMES.clamp(range_min, range_max))
}

#[derive(Debug, Clone)]
pub struct CpalInputConfig {
  pub chunk_ms: u32,
  pub buffer_ms: u32,
  pub capture_enabled: Option<Arc<PipelineControls>>,
  pub shutdown: Option<Arc<AtomicBool>>,
}

impl Default for CpalInputConfig {
  fn default() -> Self {
    Self { chunk_ms: 20, buffer_ms: 120_000, capture_enabled: None, shutdown: None }
  }
}

pub struct CpalInput {
  spec: AudioSpec,
  chunk_frames: usize,
  consumer: HeapCons<f32>,

  produced_samples: Arc<AtomicU64>,
  dropped_samples: Arc<AtomicU64>,
  ended: Arc<AtomicBool>,
  stream_error: Arc<Mutex<Option<String>>>,

  start: Instant,
  popped_samples: u64,
  baseline_deficit_samples: Option<u64>,
  last_gap_samples: u64,
  worst_gap_samples: u64,
  worst_backlog_samples: u64,
  capture_enabled: Option<Arc<PipelineControls>>,
  shutdown: Option<Arc<AtomicBool>>,
  capture_active: bool,
  /// True once we've emitted `AZAD_AUDIO_FIRST_NONZERO` for the latest
  /// false→true `capture_enabled` transition. Reset on every fresh enable
  /// so the first non-trivial-RMS chunk after each wake gets logged.
  first_audio_logged: bool,

  // The CoreAudio input unit. Started only while capture is active —
  // `open_with_device` starts it solely when capture is on at open time,
  // and `start_capture`/`stop_capture` play/pause it thereafter, so the
  // macOS mic indicator tracks listen state. Has no readers but must stay
  // alive: dropping the `Stream` tears down the CPAL callback that feeds
  // the ring buffer.
  #[allow(dead_code)]
  stream: Stream,
}

impl CpalInput {
  pub fn list_input_devices() -> Result<Vec<cpal::Device>> {
    let host = cpal::default_host();
    let devs = host
      .input_devices()
      .context("failed to enumerate input devices")?
      .collect::<Vec<_>>();
    Ok(devs)
  }

  pub fn open_with_device(device: cpal::Device, cfg: CpalInputConfig) -> Result<Self> {
    let supported =
      device.default_input_config().context("failed to query default input config")?;

    let sample_format = supported.sample_format();
    let mut stream_cfg: cpal::StreamConfig = supported.clone().into();
    if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
      if let Some(target) = preferred_fixed_buffer_frames(*min, *max) {
        stream_cfg.buffer_size = cpal::BufferSize::Fixed(target);
      } else {
        eprintln!(
          "asr: invalid input buffer range reported by backend min={} max={}; using backend default",
          min, max
        );
      }
    }

    let sr = stream_cfg.sample_rate;
    let ch = stream_cfg.channels;
    if sr == 0 || ch == 0 {
      return Err(anyhow!("invalid input stream config (sr/ch=0)"));
    }

    let chunk_frames = ((sr as u64) * (cfg.chunk_ms.max(1) as u64) / 1000).max(1) as usize;
    let buf_frames = ((sr as u64) * (cfg.buffer_ms.max(1000) as u64) / 1000).max(1) as usize;
    let cap_samples = buf_frames
      .saturating_mul(ch as usize)
      .max(chunk_frames.saturating_mul(ch as usize) * 4);

    let rb = HeapRb::<f32>::new(cap_samples);
    let (prod, cons) = rb.split();

    let produced_samples = Arc::new(AtomicU64::new(0));
    let dropped_samples = Arc::new(AtomicU64::new(0));
    let ended = Arc::new(AtomicBool::new(false));
    let stream_error = Arc::new(Mutex::new(None));

    let capture_active = cfg.capture_enabled.as_ref().map(|c| c.capture_enabled()).unwrap_or(true);
    let stream = Self::build_stream(
      &device,
      sample_format,
      &stream_cfg,
      prod,
      Arc::clone(&produced_samples),
      Arc::clone(&dropped_samples),
      Arc::clone(&ended),
      Arc::clone(&stream_error),
      cfg.capture_enabled.clone(),
    )?;
    // Start the CoreAudio input unit only when capture is on at open time.
    // Starting it unconditionally lit the macOS mic indicator even with
    // listening off: `sync_capture_state` only pauses on a true→false
    // transition, so a stream opened already-playing-but-disabled never got
    // paused and the dot stayed on for the whole session.
    if capture_active {
      stream.play().context("failed to start input stream")?;
    }
    eprintln!(
      "asr: cpal input opened; mic {}",
      if capture_active { "started" } else { "idle (capture off)" }
    );

    Ok(Self {
      spec: AudioSpec { sample_rate: sr, channels: ch },
      chunk_frames,
      consumer: cons,
      produced_samples,
      dropped_samples,
      ended,
      stream_error,
      start: Instant::now(),
      popped_samples: 0,
      baseline_deficit_samples: None,
      last_gap_samples: 0,
      worst_gap_samples: 0,
      worst_backlog_samples: 0,
      capture_enabled: cfg.capture_enabled,
      shutdown: cfg.shutdown,
      capture_active,
      first_audio_logged: false,
      stream,
    })
  }

  fn build_stream(
    device: &Device,
    sample_format: SampleFormat,
    stream_cfg: &StreamConfig,
    producer: HeapProd<f32>,
    produced_samples: Arc<AtomicU64>,
    dropped_samples: Arc<AtomicU64>,
    ended: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
    notify: Option<Arc<PipelineControls>>,
  ) -> Result<Stream> {
    let err_ended = Arc::clone(&ended);
    let err_store = Arc::clone(&stream_error);
    let err_fn = move |err: cpal::StreamError| {
      eprintln!("ERROR: audio input stream error: {err}");
      if let Ok(mut slot) = err_store.lock() {
        *slot = Some(err.to_string());
      }
      err_ended.store(true, Ordering::Relaxed);
    };

    let stream = match sample_format {
      SampleFormat::F32 => {
        let mut producer_cb = producer;
        let produced_cb = Arc::clone(&produced_samples);
        let dropped_cb = Arc::clone(&dropped_samples);
        let notify_cb = notify.clone();
        device.build_input_stream(
          stream_cfg,
          move |data: &[f32], _info| {
            produced_cb.fetch_add(data.len() as u64, Ordering::Relaxed);
            let written = producer_cb.push_slice(data);
            if written < data.len() {
              dropped_cb.fetch_add((data.len() - written) as u64, Ordering::Relaxed);
            }
            if let Some(n) = &notify_cb {
              n.notify_audio();
            }
          },
          err_fn,
          None,
        )?
      }
      SampleFormat::I16 => {
        let mut producer_cb = producer;
        let produced_cb = Arc::clone(&produced_samples);
        let dropped_cb = Arc::clone(&dropped_samples);
        let mut scratch: Vec<f32> = Vec::new();
        let notify_cb = notify.clone();
        device.build_input_stream(
          stream_cfg,
          move |data: &[i16], _info| {
            produced_cb.fetch_add(data.len() as u64, Ordering::Relaxed);
            scratch.resize(data.len(), 0.0);
            for (i, &s) in data.iter().enumerate() {
              scratch[i] = (s as f32) / 32768.0;
            }
            let written = producer_cb.push_slice(&scratch);
            if written < scratch.len() {
              dropped_cb.fetch_add((scratch.len() - written) as u64, Ordering::Relaxed);
            }
            if let Some(n) = &notify_cb {
              n.notify_audio();
            }
          },
          err_fn,
          None,
        )?
      }
      SampleFormat::U16 => {
        let mut producer_cb = producer;
        let produced_cb = Arc::clone(&produced_samples);
        let dropped_cb = Arc::clone(&dropped_samples);
        let mut scratch: Vec<f32> = Vec::new();
        let notify_cb = notify.clone();
        device.build_input_stream(
          stream_cfg,
          move |data: &[u16], _info| {
            produced_cb.fetch_add(data.len() as u64, Ordering::Relaxed);
            scratch.resize(data.len(), 0.0);
            for (i, &s) in data.iter().enumerate() {
              // Map [0, 65535] -> [-1, 1]
              scratch[i] = (s as f32 / 65535.0) * 2.0 - 1.0;
            }
            let written = producer_cb.push_slice(&scratch);
            if written < scratch.len() {
              dropped_cb.fetch_add((scratch.len() - written) as u64, Ordering::Relaxed);
            }
            if let Some(n) = &notify_cb {
              n.notify_audio();
            }
          },
          err_fn,
          None,
        )?
      }
      other => return Err(anyhow!("unsupported sample format: {other:?}")),
    };

    // Do not start the unit here — `open_with_device` plays it only when
    // capture is active, so the mic indicator tracks listen state.
    Ok(stream)
  }

  fn clear_backlog(&mut self) {
    let mut scratch = vec![0.0f32; self.chunk_frames.max(1) * self.spec.channels as usize];
    while self.consumer.occupied_len() > 0 {
      let popped = self.consumer.pop_slice(&mut scratch);
      if popped == 0 {
        break;
      }
    }
  }

  fn start_capture(&mut self) {
    // Resume the CPAL stream so CoreAudio re-acquires the input device
    // handle and macOS shows the mic indicator. The stream-warm
    // optimisation (commit 2f10a79) was traded against the user-visible
    // privacy issue of the orange mic dot staying on while listening
    // is off; the seed-grace + pre-roll fallbacks from that commit
    // still mitigate the cold-start lag they targeted.
    if let Err(err) = self.stream.play() {
      eprintln!("Azad: failed to resume CPAL stream on capture-enable: {err}");
    }
    self.capture_active = true;
    self.clear_backlog();
    // Re-arm the cold-start audio log so the next non-trivial-RMS chunk
    // after this wake fires `AZAD_AUDIO_FIRST_NONZERO`.
    self.first_audio_logged = false;
  }

  fn stop_capture(&mut self) {
    // Pause the CPAL stream so CoreAudio releases the input device
    // handle and macOS clears the orange mic indicator. The flag flip
    // alone is no longer enough — leaving the stream warm kept the
    // device "open" from the OS's perspective even with capture off.
    self.capture_active = false;
    if let Err(err) = self.stream.pause() {
      eprintln!("Azad: failed to pause CPAL stream on capture-disable: {err}");
    }
    self.clear_backlog();
  }

  fn sync_capture_state(&mut self) {
    let desired = self.capture_enabled.as_ref().map(|c| c.capture_enabled()).unwrap_or(true);
    if desired == self.capture_active {
      return;
    }

    if desired {
      self.start_capture();
    } else {
      self.stop_capture();
    }
  }

  fn shutdown_requested(&self) -> bool {
    self.shutdown.as_ref().map(|flag| flag.load(Ordering::Relaxed)).unwrap_or(false)
  }

  /// Block until the CPAL callback pushes samples, capture state flips, or the
  /// backstop elapses. Falls back to a short sleep when no `PipelineControls`
  /// is wired, which is the case for CLI replay.
  fn wait_wake(&self) {
    match self.capture_enabled.as_ref() {
      Some(controls) => controls.wait_for_wake(WAKE_BACKSTOP),
      None => std::thread::sleep(Duration::from_millis(5)),
    }
  }

  fn stream_error_message(&self) -> Option<String> {
    self.stream_error.lock().ok().and_then(|slot| slot.clone())
  }
}

#[cfg(test)]
mod tests {
  use super::preferred_fixed_buffer_frames;

  #[test]
  fn preferred_fixed_buffer_frames_uses_preferred_in_valid_range() {
    assert_eq!(preferred_fixed_buffer_frames(128, 4096), Some(2048));
  }

  #[test]
  fn preferred_fixed_buffer_frames_clamps_to_min_when_range_above_preferred() {
    assert_eq!(preferred_fixed_buffer_frames(4096, 8192), Some(4096));
  }

  #[test]
  fn preferred_fixed_buffer_frames_clamps_to_max_when_range_below_preferred() {
    assert_eq!(preferred_fixed_buffer_frames(32, 512), Some(512));
  }

  #[test]
  fn preferred_fixed_buffer_frames_rejects_invalid_ranges() {
    assert_eq!(preferred_fixed_buffer_frames(29, 0), None);
    assert_eq!(preferred_fixed_buffer_frames(512, 256), None);
    assert_eq!(preferred_fixed_buffer_frames(0, 256), None);
  }
}

impl AudioInput for CpalInput {
  fn spec(&self) -> AudioSpec {
    self.spec
  }

  fn read_chunk(&mut self) -> Result<Option<AudioChunk>> {
    let channels = self.spec.channels as usize;
    let want_samples = self.chunk_frames * channels;

    loop {
      if self.shutdown_requested() {
        return Ok(None);
      }
      if self.ended.load(Ordering::Relaxed) {
        let message = self
          .stream_error_message()
          .unwrap_or_else(|| "unknown stream failure".to_string());
        return Err(anyhow!("audio input stream ended after error: {message}"));
      }
      self.sync_capture_state();
      if !self.capture_active {
        // Stream is still running. Drain whatever's accumulated to a
        // scratch sink so the ring buffer doesn't back up; sleep a beat
        // and re-check the desired state.
        if self.consumer.occupied_len() > 0 {
          self.clear_backlog();
        }
        // Paused stream produces no callback signal; `set_capture_enabled`
        // wakes us the instant capture is re-enabled, and the backstop bounds
        // how long we linger before re-checking shutdown/state.
        self.wait_wake();
        continue;
      }

      // Blocking-ish read: wait for enough samples. Capture happens in callback thread;
      // this loop only drains the ring buffer.
      while self.consumer.occupied_len() < want_samples {
        if self.shutdown_requested() {
          return Ok(None);
        }
        self.sync_capture_state();
        if !self.capture_active {
          break;
        }
        if self.ended.load(Ordering::Relaxed) {
          let message = self
            .stream_error_message()
            .unwrap_or_else(|| "unknown stream failure".to_string());
          return Err(anyhow!("audio input stream ended after error: {message}"));
        }
        // Woken by the CPAL callback as soon as a buffer lands; the backstop is
        // just a lost-wakeup safety net.
        self.wait_wake();
      }
      if !self.capture_active {
        continue;
      }

      let mut frames = vec![0.0f32; want_samples];
      let popped = self.consumer.pop_slice(&mut frames);
      if popped == 0 {
        return Ok(None);
      }
      frames.truncate(popped);

      let n_frames = frames.len() / channels;
      self.popped_samples = self.popped_samples.saturating_add((n_frames * channels) as u64);

      // Cold-start observability: the first chunk with non-trivial energy
      // after a fresh `capture_enabled` true. Tells us whether macOS /
      // CoreAudio / the mic hardware is delivering real audio promptly
      // after wake, or whether we get a long silent prefix.
      if !self.first_audio_logged {
        if let Some(controls) = self.capture_enabled.as_ref() {
          if controls.debug_stats_enabled() {
            if let Some(enable_at) = controls.capture_enabled_since() {
              let mut sum_sq = 0.0f64;
              let mut peak = 0.0f32;
              for &s in &frames {
                sum_sq += (s as f64) * (s as f64);
                let abs = s.abs();
                if abs > peak {
                  peak = abs;
                }
              }
              let rms = (sum_sq / (frames.len().max(1) as f64)).sqrt();
              if rms > 0.001 {
                eprintln!(
                  "AZAD_AUDIO_FIRST_NONZERO ms_since_enable={} rms={:.4} peak={:.4}",
                  enable_at.elapsed().as_millis(),
                  rms,
                  peak,
                );
                self.first_audio_logged = true;
              }
            }
          }
        }
      }

      // We don't currently track an absolute sample clock from the device. Use wall-clock gap
      // to detect upstream drops (or callback starvation) separately from ring overflow drops.
      let avail_samples = self.consumer.occupied_len() as u64;
      self.worst_backlog_samples = self.worst_backlog_samples.max(avail_samples);

      let expected_samples =
        (self.start.elapsed().as_secs_f64() * (self.spec.sample_rate as f64) * (channels as f64))
          .round() as u64;
      let produced = self.produced_samples.load(Ordering::Relaxed);
      let raw_deficit = expected_samples.saturating_sub(produced);
      let baseline = *self.baseline_deficit_samples.get_or_insert(raw_deficit);
      let gap_samples = raw_deficit.saturating_sub(baseline);
      self.last_gap_samples = gap_samples;
      self.worst_gap_samples = self.worst_gap_samples.max(gap_samples);

      return Ok(Some(AudioChunk {
        start_frame: 0, // not used for live sources
        frames,
      }));
    }
  }

  fn health(&self) -> AudioHealth {
    let channels = self.spec.channels.max(1) as u64;
    let produced_samples = self.produced_samples.load(Ordering::Relaxed);
    let dropped_samples = self.dropped_samples.load(Ordering::Relaxed);
    let backlog_samples = self.consumer.occupied_len() as u64;

    AudioHealth {
      produced_frames: produced_samples / channels,
      dropped_frames: dropped_samples / channels,
      backlog_frames: backlog_samples / channels,
      wallclock_gap_frames: self.last_gap_samples / channels,
      worst_wallclock_gap_frames: self.worst_gap_samples / channels,
      worst_backlog_frames: self.worst_backlog_samples / channels,
    }
  }
}
