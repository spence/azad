use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use reqwest::header::{ETAG, HeaderMap, IF_RANGE, LAST_MODIFIED, RANGE};
use tokio_util::sync::CancellationToken;

use crate::app::{AppEvent, send_event};
use crate::models::{ModelFileDef, ModelPackDef, pack_dir};

const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_PROGRESS_INTERVAL_BYTES: u64 = 8 * 1024 * 1024;
const DOWNLOAD_PROGRESS_INTERVAL_TIME: Duration = Duration::from_millis(200);
const DOWNLOAD_WRITE_SLICE_BYTES: usize = 64 * 1024;

pub struct DownloadHandle {
  control: Arc<DownloadControl>,
}

impl DownloadHandle {
  pub fn pause(&self) {
    self.control.pause();
  }

  pub fn resume(&self) {
    self.control.resume();
  }

  pub fn is_paused(&self) -> bool {
    self.control.is_paused()
  }
}

struct DownloadControl {
  paused: AtomicBool,
  pause_lock: Mutex<()>,
  pause_cvar: Condvar,
  cancel_token: Mutex<CancellationToken>,
}

impl DownloadControl {
  fn new() -> Self {
    Self {
      paused: AtomicBool::new(false),
      pause_lock: Mutex::new(()),
      pause_cvar: Condvar::new(),
      cancel_token: Mutex::new(CancellationToken::new()),
    }
  }

  fn pause(&self) {
    self.paused.store(true, Ordering::SeqCst);
    if let Ok(token) = self.cancel_token.lock() {
      token.cancel();
    }
    self.pause_cvar.notify_all();
  }

  fn resume(&self) {
    self.paused.store(false, Ordering::SeqCst);
    if let Ok(mut token) = self.cancel_token.lock() {
      *token = CancellationToken::new();
    }
    self.pause_cvar.notify_all();
  }

  fn is_paused(&self) -> bool {
    self.paused.load(Ordering::SeqCst)
  }

  fn token(&self) -> Result<CancellationToken, String> {
    self
      .cancel_token
      .lock()
      .map(|token| token.clone())
      .map_err(|_| "download cancellation lock poisoned".to_string())
  }

  fn wait_if_paused(&self) -> Result<(), String> {
    if !self.paused.load(Ordering::SeqCst) {
      return Ok(());
    }

    let mut guard = self.pause_lock.lock().map_err(|_| "download pause lock poisoned")?;
    while self.paused.load(Ordering::SeqCst) {
      guard = self.pause_cvar.wait(guard).map_err(|_| "download pause lock poisoned")?;
    }
    Ok(())
  }
}

pub fn start_pack_download(pack: &'static ModelPackDef) -> DownloadHandle {
  let control = Arc::new(DownloadControl::new());
  let worker_control = control.clone();
  let pack_id = pack.id;

  thread::spawn(move || {
    let result = run_download_pack(pack, worker_control.clone());
    match result {
      Ok(()) => {
        if let Err(msg) = worker_control.wait_if_paused() {
          send_event(AppEvent::ModelDownloadError { pack_id: pack_id.to_string(), message: msg });
          return;
        }
        send_event(AppEvent::ModelDownloadCompleted(pack_id.to_string()));
      }
      Err(msg) => {
        send_event(AppEvent::ModelDownloadError { pack_id: pack_id.to_string(), message: msg });
      }
    }
  });

  DownloadHandle { control }
}

fn run_download_pack(
  pack: &'static ModelPackDef,
  control: Arc<DownloadControl>,
) -> Result<(), String> {
  let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .map_err(|e| format!("download runtime: {e}"))?;
  runtime.block_on(download_pack(pack, &control))
}

async fn download_pack(
  pack: &'static ModelPackDef,
  control: &DownloadControl,
) -> Result<(), String> {
  let dir = pack_dir(pack.id).ok_or_else(|| "HOME not set".to_string())?;
  let mut bytes_done: u64 = 0;
  let bytes_total = pack.total_size_bytes;

  let client = reqwest::Client::builder()
    .use_native_tls()
    .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
    .build()
    .map_err(|e| format!("http client: {e}"))?;

  for file_def in pack.files {
    control.wait_if_paused()?;

    let dest = dir.join(file_def.rel_path);
    if dest.exists() {
      match verify_download(&dest, file_def) {
        Ok(()) => {
          bytes_done += file_def.size_bytes;
          send_progress(pack.id, bytes_done, bytes_total);
          continue;
        }
        Err(_) => {
          let _ = fs::remove_file(&dest);
        }
      }
    }

    if let Some(parent) = dest.parent() {
      fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }

    let part_path = PathBuf::from(format!("{}.part", dest.display()));
    download_file(
      &client,
      DownloadFileRequest {
        url: file_def.url,
        dest: &part_path,
        expected_size: file_def.size_bytes,
        pack_id: pack.id,
        bytes_total,
      },
      control,
      &mut bytes_done,
    )
    .await?;
    control.wait_if_paused()?;
    verify_download(&part_path, file_def)?;
    control.wait_if_paused()?;

    fs::rename(&part_path, &dest)
      .map_err(|e| format!("rename {} -> {}: {e}", part_path.display(), dest.display()))?;
    let _ = fs::remove_file(resume_meta_path(&part_path));
    control.wait_if_paused()?;
  }

  control.wait_if_paused()?;
  Ok(())
}

struct DownloadFileRequest<'a> {
  url: &'a str,
  dest: &'a Path,
  expected_size: u64,
  pack_id: &'a str,
  bytes_total: u64,
}

async fn download_file(
  client: &reqwest::Client,
  request: DownloadFileRequest<'_>,
  control: &DownloadControl,
  bytes_done: &mut u64,
) -> Result<(), String> {
  let DownloadFileRequest { url, dest, expected_size, pack_id, bytes_total } = request;
  let bytes_before_file = *bytes_done;
  let mut offset = resumable_download_len(dest, url, expected_size)?;
  *bytes_done = bytes_before_file + offset;
  if offset > 0 {
    send_progress(pack_id, *bytes_done, bytes_total);
  }

  let mut last_progress_bytes = *bytes_done;
  let mut last_progress_at = Instant::now();

  loop {
    control.wait_if_paused()?;
    offset = resumable_download_len(dest, url, expected_size)?;
    *bytes_done = bytes_before_file + offset;

    if offset >= expected_size {
      break;
    }

    let token = control.token()?;
    match stream_download_once(
      client,
      url,
      dest,
      expected_size,
      control,
      token,
      &mut offset,
      bytes_before_file,
      bytes_done,
      bytes_total,
      pack_id,
      &mut last_progress_bytes,
      &mut last_progress_at,
    )
    .await?
    {
      StreamResult::Complete => break,
      StreamResult::Paused => {
        offset = partial_download_len(dest, expected_size)?;
        *bytes_done = bytes_before_file + offset;
        send_progress(pack_id, *bytes_done, bytes_total);
        control.wait_if_paused()?;
      }
    }
  }

  control.wait_if_paused()?;
  send_progress(pack_id, *bytes_done, bytes_total);
  Ok(())
}

enum StreamResult {
  Complete,
  Paused,
}

#[allow(clippy::too_many_arguments)]
async fn stream_download_once(
  client: &reqwest::Client,
  url: &str,
  dest: &Path,
  expected_size: u64,
  control: &DownloadControl,
  token: CancellationToken,
  offset: &mut u64,
  bytes_before_file: u64,
  bytes_done: &mut u64,
  bytes_total: u64,
  pack_id: &str,
  last_progress_bytes: &mut u64,
  last_progress_at: &mut Instant,
) -> Result<StreamResult, String> {
  let request_offset = *offset;
  let resume_meta = if request_offset > 0 { read_resume_meta(dest)? } else { None };
  let mut response = tokio::select! {
    _ = token.cancelled() => {
      return Ok(StreamResult::Paused);
    }
    response = send_request(client, url, request_offset, resume_meta.as_ref()) => response?,
  };
  let status = response.status();

  let restart_from_zero = request_offset > 0 && status == reqwest::StatusCode::OK;
  if request_offset > 0 && !restart_from_zero && status != reqwest::StatusCode::PARTIAL_CONTENT {
    return Err(format!(
      "GET {url}: HTTP {status}; expected byte-range response for {}",
      range_header(request_offset)
    ));
  }
  if request_offset == 0
    && status != reqwest::StatusCode::OK
    && status != reqwest::StatusCode::PARTIAL_CONTENT
  {
    return Err(format!("GET {url}: HTTP {status}"));
  }

  if restart_from_zero {
    *offset = 0;
    *bytes_done = bytes_before_file;
    *last_progress_bytes = *bytes_done;
    let _ = fs::remove_file(dest);
    let _ = fs::remove_file(resume_meta_path(dest));
    send_progress(pack_id, *bytes_done, bytes_total);
  }

  save_resume_meta(dest, &ResumeMeta::from_response(url, expected_size, response.headers()))?;

  let mut file = fs::OpenOptions::new()
    .create(true)
    .write(true)
    .append(*offset > 0)
    .truncate(*offset == 0)
    .open(dest)
    .map_err(|e| format!("open {}: {e}", dest.display()))?;

  loop {
    let chunk = tokio::select! {
      _ = token.cancelled() => {
        file.flush().map_err(|e| format!("flush {}: {e}", dest.display()))?;
        return Ok(StreamResult::Paused);
      }
      chunk = response.chunk() => chunk.map_err(|e| format!("read {url}: {e}"))?,
    };

    let Some(chunk) = chunk else {
      break;
    };

    if control.is_paused() {
      file.flush().map_err(|e| format!("flush {}: {e}", dest.display()))?;
      return Ok(StreamResult::Paused);
    }

    for slice in chunk.chunks(DOWNLOAD_WRITE_SLICE_BYTES) {
      if control.is_paused() {
        file.flush().map_err(|e| format!("flush {}: {e}", dest.display()))?;
        return Ok(StreamResult::Paused);
      }

      file.write_all(slice).map_err(|e| format!("write {}: {e}", dest.display()))?;

      *offset = offset.saturating_add(slice.len() as u64);
      *bytes_done = bytes_before_file + *offset;

      if *offset > expected_size {
        return Err(format!("downloaded {} bytes from {url}, expected {}", *offset, expected_size));
      }

      if *bytes_done - *last_progress_bytes >= DOWNLOAD_PROGRESS_INTERVAL_BYTES
        || last_progress_at.elapsed() >= DOWNLOAD_PROGRESS_INTERVAL_TIME
      {
        send_progress(pack_id, *bytes_done, bytes_total);
        *last_progress_bytes = *bytes_done;
        *last_progress_at = Instant::now();
      }
    }
  }

  file.flush().map_err(|e| format!("flush {}: {e}", dest.display()))?;
  send_progress(pack_id, *bytes_done, bytes_total);

  if *offset != expected_size {
    return Err(format!("downloaded {} bytes from {url}, expected {}", *offset, expected_size));
  }

  Ok(StreamResult::Complete)
}

async fn send_request(
  client: &reqwest::Client,
  url: &str,
  offset: u64,
  resume_meta: Option<&ResumeMeta>,
) -> Result<reqwest::Response, String> {
  let mut request = client.get(url);
  if offset > 0 {
    request = request.header(RANGE, range_header(offset));
    if let Some(if_range) = resume_meta.and_then(ResumeMeta::if_range_value) {
      request = request.header(IF_RANGE, if_range);
    }
  }

  request.send().await.map_err(|e| format!("GET {url}: {e}"))
}

fn range_header(offset: u64) -> String {
  format!("bytes={offset}-")
}

fn resumable_download_len(path: &Path, url: &str, expected_size: u64) -> Result<u64, String> {
  let len = partial_download_len(path, expected_size)?;
  if len == 0 {
    let _ = fs::remove_file(resume_meta_path(path));
    return Ok(0);
  }

  match read_resume_meta(path) {
    Ok(Some(meta)) if meta.url == url && meta.expected_size == expected_size => Ok(len),
    Ok(Some(_)) => {
      remove_partial(path)?;
      Ok(0)
    }
    Ok(None) => Ok(len),
    Err(_) => {
      remove_partial(path)?;
      Ok(0)
    }
  }
}

fn partial_download_len(path: &Path, expected_size: u64) -> Result<u64, String> {
  match fs::metadata(path) {
    Ok(meta) if meta.len() <= expected_size => Ok(meta.len()),
    Ok(_) => {
      remove_partial(path)?;
      Ok(0)
    }
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
    Err(err) => Err(format!("stat {}: {err}", path.display())),
  }
}

fn remove_partial(path: &Path) -> Result<(), String> {
  let _ = fs::remove_file(path);
  let _ = fs::remove_file(resume_meta_path(path));
  Ok(())
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct ResumeMeta {
  url: String,
  expected_size: u64,
  etag: Option<String>,
  last_modified: Option<String>,
}

impl ResumeMeta {
  fn from_response(url: &str, expected_size: u64, headers: &HeaderMap) -> Self {
    Self {
      url: url.to_string(),
      expected_size,
      etag: header_value(headers, ETAG),
      last_modified: header_value(headers, LAST_MODIFIED),
    }
  }

  fn if_range_value(&self) -> Option<&str> {
    self.etag.as_deref().or(self.last_modified.as_deref())
  }
}

fn read_resume_meta(path: &Path) -> Result<Option<ResumeMeta>, String> {
  let meta_path = resume_meta_path(path);
  let content = match fs::read_to_string(&meta_path) {
    Ok(content) => content,
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(err) => return Err(format!("read {}: {err}", meta_path.display())),
  };

  serde_json::from_str(&content)
    .map(Some)
    .map_err(|e| format!("parse {}: {e}", meta_path.display()))
}

fn save_resume_meta(path: &Path, meta: &ResumeMeta) -> Result<(), String> {
  let meta_path = resume_meta_path(path);
  let content =
    serde_json::to_string(meta).map_err(|e| format!("serialize resume metadata: {e}"))?;
  fs::write(&meta_path, content).map_err(|e| format!("write {}: {e}", meta_path.display()))
}

fn resume_meta_path(path: &Path) -> PathBuf {
  PathBuf::from(format!("{}.meta", path.display()))
}

fn header_value(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
  headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
}

fn verify_download(path: &Path, file_def: &ModelFileDef) -> Result<(), String> {
  let actual_len = fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?.len();
  if actual_len != file_def.size_bytes {
    let _ = fs::remove_file(path);
    return Err(format!(
      "downloaded {} has {} bytes, expected {}",
      file_def.rel_path, actual_len, file_def.size_bytes
    ));
  }

  let actual_hash = sha256_file(path)?;
  if actual_hash != file_def.sha256 {
    let _ = fs::remove_file(path);
    return Err(format!(
      "downloaded {} has SHA-256 {}, expected {}",
      file_def.rel_path, actual_hash, file_def.sha256
    ));
  }

  Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
  use sha2::{Digest, Sha256};

  let mut file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
  let mut hasher = Sha256::new();
  let mut buf = vec![0u8; 2 * 1024 * 1024];
  loop {
    let n = file.read(&mut buf).map_err(|e| format!("read {}: {e}", path.display()))?;
    if n == 0 {
      break;
    }
    hasher.update(&buf[..n]);
  }
  Ok(format!("{:x}", hasher.finalize()))
}

fn send_progress(pack_id: &str, bytes_done: u64, bytes_total: u64) {
  send_event(AppEvent::ModelDownloadProgress {
    pack_id: pack_id.to_string(),
    bytes_done,
    bytes_total,
  });
}

#[cfg(test)]
mod tests {
  use super::{
    DOWNLOAD_CONNECT_TIMEOUT, DownloadControl, DownloadFileRequest, DownloadHandle, download_file,
    partial_download_len, range_header,
  };
  use std::fs;
  use std::io::{Read, Write};
  use std::net::{TcpListener, TcpStream};
  use std::path::{Path, PathBuf};
  use std::sync::Arc;
  use std::thread;
  use std::time::{Duration, Instant};

  #[test]
  fn download_handle_tracks_pause_and_resume() {
    let handle = DownloadHandle { control: Arc::new(DownloadControl::new()) };

    assert!(!handle.is_paused());
    handle.pause();
    assert!(handle.is_paused());
    handle.resume();
    assert!(!handle.is_paused());
  }

  #[test]
  fn range_header_starts_from_partial_offset() {
    assert_eq!(range_header(42), "bytes=42-");
  }

  #[test]
  fn partial_download_len_keeps_valid_partial_file() {
    let path =
      std::env::temp_dir().join(format!("azad-partial-{}-{}.part", std::process::id(), "valid"));
    fs::write(&path, b"abc").unwrap();
    assert_eq!(partial_download_len(&path, 10).unwrap(), 3);
    let _ = fs::remove_file(path);
  }

  #[test]
  fn partial_download_len_removes_oversized_partial_file() {
    let path = std::env::temp_dir().join(format!(
      "azad-partial-{}-{}.part",
      std::process::id(),
      "oversized"
    ));
    fs::write(&path, b"abc").unwrap();
    assert_eq!(partial_download_len(&path, 2).unwrap(), 0);
    assert!(!path.exists());
  }

  #[test]
  fn downloader_pause_stops_partial_file_growth_and_resume_finishes() {
    let data: Arc<Vec<u8>> = Arc::new((0..2_500_000).map(|i| (i % 251) as u8).collect());
    let url = spawn_range_server(data.clone());
    let path = std::env::temp_dir().join(format!(
      "azad-download-pause-{}-{}.part",
      std::process::id(),
      "range"
    ));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));

    let control = Arc::new(DownloadControl::new());
    let worker_control = control.clone();
    let worker_data = data.clone();
    let worker_path = path.clone();
    let worker = thread::spawn(move || {
      let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
      runtime.block_on(async move {
        let client = reqwest::Client::builder()
          .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
          .build()
          .unwrap();
        let mut bytes_done = 0;
        download_file(
          &client,
          DownloadFileRequest {
            url: &url,
            dest: &worker_path,
            expected_size: worker_data.len() as u64,
            pack_id: "test-pack",
            bytes_total: worker_data.len() as u64,
          },
          &worker_control,
          &mut bytes_done,
        )
        .await
        .unwrap();
        bytes_done
      })
    });

    wait_for_file_len_at_least(&path, 64 * 1024, Duration::from_secs(2));
    control.pause();
    let paused_len =
      wait_for_stable_file_len(&path, Duration::from_millis(200), Duration::from_secs(2));
    thread::sleep(Duration::from_millis(250));
    assert_eq!(fs::metadata(&path).unwrap().len(), paused_len);
    assert!(paused_len < data.len() as u64);

    control.resume();
    let bytes_done = worker.join().unwrap();
    assert_eq!(bytes_done, data.len() as u64);
    assert_eq!(fs::read(&path).unwrap(), data.as_slice());
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));
  }

  #[test]
  fn downloader_pause_interrupts_request_before_response_headers() {
    let data: Arc<Vec<u8>> = Arc::new((0..512_000).map(|i| (i % 251) as u8).collect());
    let url = spawn_range_server_with_header_delay(data.clone(), Duration::from_millis(600));
    let path = std::env::temp_dir().join(format!(
      "azad-download-pause-before-headers-{}-{}.part",
      std::process::id(),
      "range"
    ));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));

    let control = Arc::new(DownloadControl::new());
    let worker_control = control.clone();
    let worker_data = data.clone();
    let worker_path = path.clone();
    let worker = thread::spawn(move || {
      let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
      runtime.block_on(async move {
        let client = reqwest::Client::builder()
          .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
          .build()
          .unwrap();
        let mut bytes_done = 0;
        download_file(
          &client,
          DownloadFileRequest {
            url: &url,
            dest: &worker_path,
            expected_size: worker_data.len() as u64,
            pack_id: "test-pack",
            bytes_total: worker_data.len() as u64,
          },
          &worker_control,
          &mut bytes_done,
        )
        .await
        .unwrap();
        bytes_done
      })
    });

    thread::sleep(Duration::from_millis(100));
    control.pause();
    thread::sleep(Duration::from_millis(250));
    assert_eq!(fs::metadata(&path).map(|m| m.len()).unwrap_or(0), 0);

    control.resume();
    let bytes_done = worker.join().unwrap();
    assert_eq!(bytes_done, data.len() as u64);
    assert_eq!(fs::read(&path).unwrap(), data.as_slice());
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));
  }

  #[test]
  fn downloader_pause_settles_quickly_during_body_stream() {
    let data: Arc<Vec<u8>> = Arc::new((0..6_000_000).map(|i| (i % 251) as u8).collect());
    let url =
      spawn_range_server_with_delays(data.clone(), Duration::ZERO, Duration::from_millis(8));
    let path = temp_part_path("azad-download-pause-latency");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));

    let control = Arc::new(DownloadControl::new());
    let worker = spawn_download_worker(url, path.clone(), data.clone(), control.clone());

    wait_for_file_len_at_least(&path, 256 * 1024, Duration::from_secs(2));
    pause_and_assert_settles_quickly(&control, &path, data.len() as u64);

    control.resume();
    let bytes_done = worker.join().unwrap();
    assert_eq!(bytes_done, data.len() as u64);
    assert_eq!(fs::read(&path).unwrap(), data.as_slice());
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));
  }

  #[test]
  fn downloader_repeated_pause_resume_cycles_settle_quickly() {
    let data: Arc<Vec<u8>> = Arc::new((0..16_000_000).map(|i| (i % 251) as u8).collect());
    let url =
      spawn_range_server_with_delays(data.clone(), Duration::ZERO, Duration::from_millis(8));
    let path = temp_part_path("azad-download-pause-cycles");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));

    let control = Arc::new(DownloadControl::new());
    let worker = spawn_download_worker(url, path.clone(), data.clone(), control.clone());

    let mut previous_len = 0;
    for _ in 0..3 {
      wait_for_file_len_greater_than(&path, previous_len + 128 * 1024, Duration::from_secs(3));
      previous_len = pause_and_assert_settles_quickly(&control, &path, data.len() as u64);
      control.resume();
    }

    let bytes_done = worker.join().unwrap();
    assert_eq!(bytes_done, data.len() as u64);
    assert_eq!(fs::read(&path).unwrap(), data.as_slice());
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));
  }

  #[test]
  fn downloader_immediate_pause_after_resume_cancels_new_request() {
    let data: Arc<Vec<u8>> = Arc::new((0..512_000).map(|i| (i % 251) as u8).collect());
    let url = spawn_range_server_with_header_delay(data.clone(), Duration::from_millis(600));
    let path = temp_part_path("azad-download-immediate-pause-after-resume");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));

    let control = Arc::new(DownloadControl::new());
    let worker = spawn_download_worker(url, path.clone(), data.clone(), control.clone());

    thread::sleep(Duration::from_millis(100));
    control.pause();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(fs::metadata(&path).map(|m| m.len()).unwrap_or(0), 0);

    control.resume();
    thread::sleep(Duration::from_millis(20));
    control.pause();
    thread::sleep(Duration::from_millis(250));
    assert_eq!(fs::metadata(&path).map(|m| m.len()).unwrap_or(0), 0);

    control.resume();
    let bytes_done = worker.join().unwrap();
    assert_eq!(bytes_done, data.len() as u64);
    assert_eq!(fs::read(&path).unwrap(), data.as_slice());
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::resume_meta_path(&path));
  }

  fn spawn_download_worker(
    url: String,
    path: PathBuf,
    data: Arc<Vec<u8>>,
    control: Arc<DownloadControl>,
  ) -> thread::JoinHandle<u64> {
    thread::spawn(move || {
      let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
      runtime.block_on(async move {
        let client = reqwest::Client::builder()
          .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
          .build()
          .unwrap();
        let mut bytes_done = 0;
        download_file(
          &client,
          DownloadFileRequest {
            url: &url,
            dest: &path,
            expected_size: data.len() as u64,
            pack_id: "test-pack",
            bytes_total: data.len() as u64,
          },
          &control,
          &mut bytes_done,
        )
        .await
        .unwrap();
        bytes_done
      })
    })
  }

  fn wait_for_file_len_at_least(path: &Path, min_len: u64, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
      if fs::metadata(path).map(|m| m.len() >= min_len).unwrap_or(false) {
        return;
      }
      assert!(Instant::now() < deadline, "timed out waiting for partial download");
      thread::sleep(Duration::from_millis(10));
    }
  }

  fn wait_for_file_len_greater_than(path: &Path, min_len: u64, timeout: Duration) -> u64 {
    let deadline = Instant::now() + timeout;
    loop {
      if let Ok(meta) = fs::metadata(path)
        && meta.len() > min_len
      {
        return meta.len();
      }
      assert!(Instant::now() < deadline, "timed out waiting for partial download growth");
      thread::sleep(Duration::from_millis(10));
    }
  }

  fn wait_for_stable_file_len(path: &Path, stable_for: Duration, timeout: Duration) -> u64 {
    let deadline = Instant::now() + timeout;
    let mut last_len = fs::metadata(path).unwrap().len();
    let mut stable_since = Instant::now();
    loop {
      thread::sleep(Duration::from_millis(20));
      let len = fs::metadata(path).unwrap().len();
      if len != last_len {
        last_len = len;
        stable_since = Instant::now();
      }
      if stable_since.elapsed() >= stable_for {
        return last_len;
      }
      assert!(Instant::now() < deadline, "timed out waiting for paused download to settle");
    }
  }

  fn pause_and_assert_settles_quickly(
    control: &DownloadControl,
    path: &Path,
    total_len: u64,
  ) -> u64 {
    let before_pause = fs::metadata(path).unwrap().len();
    let started = Instant::now();

    control.pause();
    let paused_len =
      wait_for_stable_file_len(path, Duration::from_millis(80), Duration::from_millis(400));
    let elapsed = started.elapsed();

    assert!(
      elapsed <= Duration::from_millis(450),
      "pause took {elapsed:?} to settle at {paused_len} bytes"
    );
    assert!(
      paused_len <= before_pause + 512 * 1024,
      "download wrote {} bytes after pause; before={before_pause}, paused={paused_len}",
      paused_len.saturating_sub(before_pause)
    );
    assert!(paused_len < total_len, "download completed before pause could take effect");

    thread::sleep(Duration::from_millis(120));
    assert_eq!(fs::metadata(path).unwrap().len(), paused_len);
    paused_len
  }

  fn spawn_range_server(data: Arc<Vec<u8>>) -> String {
    spawn_range_server_with_header_delay(data, Duration::ZERO)
  }

  fn spawn_range_server_with_header_delay(data: Arc<Vec<u8>>, header_delay: Duration) -> String {
    spawn_range_server_with_delays(data, header_delay, Duration::from_millis(10))
  }

  fn spawn_range_server_with_delays(
    data: Arc<Vec<u8>>,
    header_delay: Duration,
    chunk_delay: Duration,
  ) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
      for stream in listener.incoming().take(32) {
        let mut stream = stream.unwrap();
        serve_range_request(&mut stream, &data, header_delay, chunk_delay);
      }
    });
    format!("http://{addr}/model.bin")
  }

  fn serve_range_request(
    stream: &mut TcpStream,
    data: &[u8],
    header_delay: Duration,
    chunk_delay: Duration,
  ) {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
      let n = stream.read(&mut buf).unwrap();
      if n == 0 {
        return;
      }
      request.extend_from_slice(&buf[..n]);
      if request.windows(4).any(|w| w == b"\r\n\r\n") {
        break;
      }
    }

    let request = String::from_utf8_lossy(&request);
    let (start, end, status) = match parse_range(&request) {
      Some((start, end)) => (start, end.min(data.len() - 1), "206 Partial Content"),
      None => (0, data.len() - 1, "200 OK"),
    };
    let body = &data[start..=end];
    thread::sleep(header_delay);
    if write!(
      stream,
      "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nETag: \"test-etag\"\r\nConnection: close\r\n\r\n",
      body.len(),
      start,
      end,
      data.len()
    )
    .is_err()
    {
      return;
    }
    for chunk in body.chunks(64 * 1024) {
      if stream.write_all(chunk).is_err() {
        return;
      }
      if stream.flush().is_err() {
        return;
      }
      thread::sleep(chunk_delay);
    }
  }

  fn parse_range(request: &str) -> Option<(usize, usize)> {
    let line = request.lines().find(|line| line.to_ascii_lowercase().starts_with("range:"))?;
    let value = line.split_once(':').unwrap().1.trim();
    let value = value.strip_prefix("bytes=").unwrap();
    let (start, end) = value.split_once('-').unwrap();
    let start = start.parse().unwrap();
    let end = if end.is_empty() { usize::MAX } else { end.parse().unwrap() };
    Some((start, end))
  }

  fn temp_part_path(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}-range.part", std::process::id()))
  }
}
