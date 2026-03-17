use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crate::app::{AppEvent, send_event};
use crate::models::{ModelPackDef, pack_dir};

pub struct DownloadHandle {
  pub cancel: Arc<AtomicBool>,
}

impl DownloadHandle {
  pub fn cancel(&self) {
    self.cancel.store(true, Ordering::SeqCst);
  }
}

pub fn start_pack_download(pack: &'static ModelPackDef) -> DownloadHandle {
  let cancel = Arc::new(AtomicBool::new(false));
  let cancel_flag = cancel.clone();
  let pack_id = pack.id;

  thread::spawn(move || {
    let result = download_pack(pack, &cancel_flag);
    match result {
      Ok(()) => {
        send_event(AppEvent::ModelDownloadCompleted(pack_id.to_string()));
      }
      Err(msg) => {
        if cancel_flag.load(Ordering::SeqCst) {
          return;
        }
        send_event(AppEvent::ModelDownloadError { pack_id: pack_id.to_string(), message: msg });
      }
    }
  });

  DownloadHandle { cancel }
}

fn download_pack(pack: &'static ModelPackDef, cancel: &AtomicBool) -> Result<(), String> {
  let dir = pack_dir(pack.id);
  let mut bytes_done: u64 = 0;
  let bytes_total = pack.total_size_bytes;

  for file_def in pack.files {
    if cancel.load(Ordering::SeqCst) {
      return Err("cancelled".to_string());
    }

    let dest = dir.join(file_def.rel_path);
    if dest.exists() {
      bytes_done += file_def.size_bytes;
      send_progress(pack.id, bytes_done, bytes_total);
      continue;
    }

    if let Some(parent) = dest.parent() {
      fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }

    let part_path = PathBuf::from(format!("{}.part", dest.display()));
    download_file(file_def.url, &part_path, cancel, pack.id, &mut bytes_done, bytes_total)?;

    fs::rename(&part_path, &dest)
      .map_err(|e| format!("rename {} -> {}: {e}", part_path.display(), dest.display()))?;
  }

  Ok(())
}

fn download_file(
  url: &str,
  dest: &PathBuf,
  cancel: &AtomicBool,
  pack_id: &str,
  bytes_done: &mut u64,
  bytes_total: u64,
) -> Result<(), String> {
  let response = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;

  let status = response.status();
  if status != 200 {
    return Err(format!("GET {url}: HTTP {status}"));
  }

  let mut reader = response.into_body().into_reader();
  let mut file = fs::File::create(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;

  let mut buf = vec![0u8; 256 * 1024];
  let mut last_progress_bytes = *bytes_done;

  loop {
    if cancel.load(Ordering::SeqCst) {
      drop(file);
      let _ = fs::remove_file(dest);
      return Err("cancelled".to_string());
    }

    let n = reader.read(&mut buf).map_err(|e| format!("read {url}: {e}"))?;
    if n == 0 {
      break;
    }

    file
      .write_all(&buf[..n])
      .map_err(|e| format!("write {}: {e}", dest.display()))?;

    *bytes_done += n as u64;

    // Send progress roughly every 1 MB.
    if *bytes_done - last_progress_bytes >= 1_000_000 {
      send_progress(pack_id, *bytes_done, bytes_total);
      last_progress_bytes = *bytes_done;
    }
  }

  file.flush().map_err(|e| format!("flush {}: {e}", dest.display()))?;

  send_progress(pack_id, *bytes_done, bytes_total);
  Ok(())
}

fn send_progress(pack_id: &str, bytes_done: u64, bytes_total: u64) {
  send_event(AppEvent::ModelDownloadProgress {
    pack_id: pack_id.to_string(),
    bytes_done,
    bytes_total,
  });
}
