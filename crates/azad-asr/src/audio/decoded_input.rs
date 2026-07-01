use crate::audio::{AudioChunk, AudioHealth, AudioInput, AudioSpec};
use anyhow::{Context, Result, anyhow};
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// An `AudioInput` backed by a fully-decoded audio file.
///
/// This is primarily for debugging: it lets `asr transcribe-file` accept common formats
/// (m4a, mp3, etc.) without depending on ffmpeg. For WAV, prefer `WavInput` for streaming IO.
pub struct DecodedInput {
  spec: AudioSpec,
  chunk_frames: usize,
  start_frame: u64,
  produced_frames: u64,
  cursor_samples: usize,
  samples: Vec<f32>, // interleaved f32, normalized-ish
  _path: PathBuf,
}

impl DecodedInput {
  pub fn decode(path: impl AsRef<Path>, chunk_ms: u32) -> Result<Self> {
    let path = path.as_ref();
    let (samples, spec) = decode_file_to_f32(path)?;

    let chunk_frames =
      ((spec.sample_rate as u64) * (chunk_ms.max(1) as u64) / 1000).max(1) as usize;

    Ok(Self {
      spec,
      chunk_frames,
      start_frame: 0,
      produced_frames: 0,
      cursor_samples: 0,
      samples,
      _path: path.to_path_buf(),
    })
  }
}

impl AudioInput for DecodedInput {
  fn spec(&self) -> AudioSpec {
    self.spec
  }

  fn read_chunk(&mut self) -> Result<Option<AudioChunk>> {
    if self.cursor_samples >= self.samples.len() {
      return Ok(None);
    }

    let channels = self.spec.channels.max(1) as usize;
    let want_samples = self.chunk_frames * channels;

    let end = (self.cursor_samples + want_samples).min(self.samples.len());
    let frames = self.samples[self.cursor_samples..end].to_vec();

    let n_frames = frames.len() / channels;
    let chunk = AudioChunk { start_frame: self.start_frame, frames };

    self.cursor_samples = end;
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

fn decode_file_to_f32(path: &Path) -> Result<(Vec<f32>, AudioSpec)> {
  let file =
    File::open(path).with_context(|| format!("failed to open audio file: {}", path.display()))?;
  let mss = MediaSourceStream::new(Box::new(file), Default::default());

  let mut hint = Hint::new();
  if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
    hint.with_extension(ext);
  }

  let probed = symphonia::default::get_probe()
    .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
    .map_err(|e| anyhow!("failed to probe audio format: {e}"))?;

  let mut format = probed.format;
  let track = format.default_track().ok_or_else(|| anyhow!("no default audio track found"))?;

  let mut decoder = symphonia::default::get_codecs()
    .make(&track.codec_params, &DecoderOptions::default())
    .map_err(|e| anyhow!("failed to create decoder: {e}"))?;

  let mut out: Vec<f32> = Vec::new();
  let mut sample_buf: Option<SampleBuffer<f32>> = None;
  let mut spec_out: Option<AudioSpec> = None;

  loop {
    let packet = match format.next_packet() {
      Ok(p) => p,
      Err(SymphoniaError::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => break,
      Err(e) => return Err(anyhow!("failed to read audio packet: {e}")),
    };

    let decoded = match decoder.decode(&packet) {
      Ok(d) => d,
      Err(SymphoniaError::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => break,
      Err(SymphoniaError::DecodeError(_)) => {
        // Corrupt packet; skip.
        continue;
      }
      Err(e) => return Err(anyhow!("failed to decode audio packet: {e}")),
    };

    let spec = decoded.spec();
    let spec = AudioSpec { sample_rate: spec.rate, channels: spec.channels.count() as u16 };
    if spec.sample_rate == 0 || spec.channels == 0 {
      return Err(anyhow!("decoded audio has invalid spec (sr/ch=0)"));
    }
    spec_out.get_or_insert(spec);

    let buf = sample_buf
      .get_or_insert_with(|| SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec()));
    buf.copy_interleaved_ref(decoded);
    out.extend_from_slice(buf.samples());
  }

  let spec = spec_out.ok_or_else(|| anyhow!("no audio samples decoded"))?;
  Ok((out, spec))
}
