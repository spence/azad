use crate::pipeline::EngineState;
use crate::render::RenderEvent;
use anyhow::{Context, Result, anyhow};
use crossbeam_channel::Receiver;
use crossterm::{
  event::{self, Event, KeyCode, KeyModifiers},
  event::{DisableMouseCapture, EnableMouseCapture, MouseEventKind},
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
  Terminal,
  backend::CrosstermBackend,
  layout::{Constraint, Direction, Layout},
  style::{Color, Modifier, Style},
  text::{Line, Text},
  widgets::{Block, Borders, Paragraph, Wrap},
};
use std::io::{self, Write as _};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct App {
  model: String,
  device: String,

  state: EngineState,
  status_detail: String,

  history: Vec<HistoryLine>,
  max_seen_turn_id: u64,
  transcript_area: ratatui::layout::Rect,
  transcript_scroll: u16,
  transcript_follow: bool,
  active_committed: String,
  active_live: String,

  audio_peak_db: f32,
  vad_speech: bool,
}

#[derive(Clone)]
struct HistoryLine {
  id: u64,
  text: String,
}

impl App {
  fn new(model: String, device: String) -> Self {
    Self {
      model,
      device,
      state: EngineState::Idle,
      status_detail: "starting".to_string(),
      history: Vec::new(),
      max_seen_turn_id: 0,
      transcript_area: ratatui::layout::Rect::default(),
      transcript_scroll: 0,
      transcript_follow: true,
      active_committed: String::new(),
      active_live: String::new(),

      audio_peak_db: -120.0,
      vad_speech: false,
    }
  }

  fn active_line(&self) -> String {
    let body = format!("{}{}", self.active_committed, self.active_live).trim().to_string();
    if body.is_empty() { String::new() } else { body }
  }

  fn push_history_line(&mut self, id: u64, text: String) {
    self.history.push(HistoryLine { id, text });
    if id > 0 {
      self.max_seen_turn_id = self.max_seen_turn_id.max(id);
    }
    if self.history.len() > 500 {
      let excess = self.history.len() - 500;
      self.history.drain(..excess);
    }
  }

  fn upsert_turn_line(&mut self, id: u64, text: String) {
    if id == 0 {
      self.push_history_line(0, text);
      return;
    }

    if let Some(line) = self.history.iter_mut().rev().find(|l| l.id == id) {
      line.text = text;
      self.max_seen_turn_id = self.max_seen_turn_id.max(id);
      return;
    }

    self.push_history_line(id, text);
  }

  fn apply_replace_line(&mut self, id: u64, text: String) {
    if id == 0 {
      return;
    }

    if let Some(line) = self.history.iter_mut().rev().find(|l| l.id == id) {
      line.text = text;
      self.max_seen_turn_id = self.max_seen_turn_id.max(id);
    }
  }

  fn transcript_copy_text(&self) -> Option<String> {
    let mut out = String::new();
    for line in &self.history {
      let text = line.text.trim();
      if text.is_empty() || text.starts_with("ERROR:") {
        continue;
      }
      if !out.is_empty() {
        out.push('\n');
      }
      out.push_str(text);
    }
    if out.is_empty() { None } else { Some(out) }
  }

  fn transcript_lines_len(&self) -> usize {
    // Each history entry is a line (we join with '\n').
    if self.history.is_empty() { 1 } else { self.history.len() }
  }

  fn transcript_inner_height(&self) -> u16 {
    // Transcript paragraph has a bordered block: inner height is height - 2.
    self.transcript_area.height.saturating_sub(2)
  }

  fn transcript_max_scroll(&self) -> u16 {
    let lines = self.transcript_lines_len() as i32;
    let inner_h = self.transcript_inner_height() as i32;
    (lines - inner_h).max(0) as u16
  }

  fn scroll_transcript_up(&mut self, lines: u16) {
    if self.transcript_lines_len() <= 1 {
      return;
    }
    self.transcript_follow = false;
    self.transcript_scroll = self.transcript_scroll.saturating_sub(lines.max(1));
  }

  fn scroll_transcript_down(&mut self, lines: u16) {
    if self.transcript_lines_len() <= 1 {
      return;
    }
    let max = self.transcript_max_scroll();
    self.transcript_scroll = (self.transcript_scroll + lines.max(1)).min(max);
    if self.transcript_scroll >= max {
      self.transcript_follow = true;
    }
  }
}

pub fn run_ui(
  rx: Receiver<RenderEvent>,
  shutdown: Arc<AtomicBool>,
  model: String,
  device: String,
) -> Result<()> {
  let mut stdout = io::stdout();

  enable_raw_mode().context("failed to enable raw mode")?;
  execute!(stdout, EnterAlternateScreen).context("failed to enter alt screen")?;
  execute!(stdout, EnableMouseCapture).ok();

  let backend = CrosstermBackend::new(stdout);
  let mut terminal = Terminal::new(backend).context("failed to init terminal")?;
  terminal.clear().ok();

  let res = run_loop(&mut terminal, rx, shutdown.clone(), model, device);

  // Always restore terminal.
  disable_raw_mode().ok();
  execute!(terminal.backend_mut(), DisableMouseCapture).ok();
  execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
  terminal.show_cursor().ok();

  res
}

fn run_loop(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  rx: Receiver<RenderEvent>,
  shutdown: Arc<AtomicBool>,
  model: String,
  device: String,
) -> Result<()> {
  let mut app = App::new(model, device);

  let tick = Duration::from_millis(50);
  let mut last_tick = Instant::now();

  loop {
    // Drain engine events.
    while let Ok(ev) = rx.try_recv() {
      match ev {
        RenderEvent::Status(v) => {
          app.state = v.state;
          app.status_detail = v.detail;
        }
        RenderEvent::SpeechStartedByVad => {
          app.status_detail = "speech (vad)".to_string();
        }
        RenderEvent::TurnStarted { .. } => {
          // No status change here — `SpeechStartedByVad` already labels VAD
          // turns "(vad)", and the unified `TurnStarted` event arrives for
          // both Vad and Manual paths. The TUI doesn't need a `(manual)`
          // label today; this arm exists for exhaustiveness.
        }
        RenderEvent::Meter(v) => {
          app.audio_peak_db = v.peak_db;
          app.vad_speech = v.vad_speech;
        }
        RenderEvent::CaptureHealth(v) => {
          let _ = v;
        }
        RenderEvent::Active { id: _, committed, live } => {
          app.active_committed = committed;
          app.active_live = live;
        }
        RenderEvent::Finalizing { id: _, text } => {
          app.status_detail = "finalizing".to_string();
          app.active_committed = text;
          app.active_live.clear();
        }
        RenderEvent::FinalizingCancelled { id: _ } => {
          app.status_detail = "speech".to_string();
        }
        RenderEvent::FinalLine { id, text } => {
          app.upsert_turn_line(id, text.trim().to_string());
        }
        RenderEvent::ReplaceLine { id, text } => {
          let new_text = text.trim().to_string();
          app.apply_replace_line(id, new_text);
        }
        RenderEvent::Error { message } => {
          app.push_history_line(0, format!("ERROR: {}", message));
          app.status_detail = "error".to_string();
        }
        RenderEvent::DebugStats(_) => {}
      }
    }

    terminal
      .draw(|f| {
        let area = f.size();
        let chunks = Layout::default()
          .direction(Direction::Vertical)
          .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length((area.height / 4).max(3)),
          ])
          .split(area);

        app.transcript_area = chunks[1];
        if app.transcript_follow {
          app.transcript_scroll = app.transcript_max_scroll();
        } else {
          app.transcript_scroll = app.transcript_scroll.min(app.transcript_max_scroll());
        }

        let status = format!(
          "{:?} | {} | model={} | device={} | q:quit c:copy d:clear",
          app.state, app.status_detail, app.model, app.device,
        );
        let status = Paragraph::new(status).style(Style::default().add_modifier(Modifier::BOLD));
        f.render_widget(status, chunks[0]);

        let history_text = if app.history.is_empty() {
          Text::from(Line::from("Say something to start..."))
        } else {
          Text::from(app.history.iter().map(|l| Line::from(l.text.clone())).collect::<Vec<_>>())
        };
        let history = Paragraph::new(history_text)
          .wrap(Wrap { trim: false })
          .scroll((app.transcript_scroll, 0))
          .block(Block::default().borders(Borders::ALL).title("Transcript"));
        f.render_widget(history, chunks[1]);

        let bottom = Layout::default()
          .direction(Direction::Horizontal)
          .constraints([Constraint::Min(1), Constraint::Length(1)])
          .split(chunks[2]);

        let active_line = app.active_line();
        let active = Paragraph::new(active_line)
          .wrap(Wrap { trim: false })
          .block(Block::default().borders(Borders::ALL).title("Active"));
        f.render_widget(active, bottom[0]);

        let meter = vertical_audio_meter(app.audio_peak_db, app.vad_speech, bottom[1]);
        f.render_widget(meter, bottom[1]);
      })
      .context("terminal draw failed")?;

    // Input handling.
    let timeout = tick
      .checked_sub(last_tick.elapsed())
      .unwrap_or_else(|| Duration::from_millis(0));
    if event::poll(timeout).context("event poll failed")? {
      match event::read().context("event read failed")? {
        Event::Key(k) => match (k.code, k.modifiers) {
          (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            shutdown.store(true, Ordering::Relaxed);
            break;
          }
          (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            shutdown.store(true, Ordering::Relaxed);
            break;
          }
          (KeyCode::Char('c'), _) => {
            let Some(copy_text) = app.transcript_copy_text() else {
              app.status_detail = "copy failed".to_string();
              app.push_history_line(0, "ERROR: copy failed: transcript is empty".to_string());
              continue;
            };

            match copy_to_system_clipboard(&copy_text) {
              Ok(()) => app.status_detail = "copied".to_string(),
              Err(e) => {
                app.status_detail = "copy failed".to_string();
                app.push_history_line(0, format!("ERROR: copy failed: {e}"));
              }
            }
          }
          (KeyCode::Char('d'), _) => {
            app.history.clear();
            app.transcript_scroll = 0;
            app.transcript_follow = true;
          }
          _ => {}
        },
        Event::Mouse(m) => match m.kind {
          MouseEventKind::ScrollUp if rect_contains(app.transcript_area, m.column, m.row) => {
            app.scroll_transcript_up(3);
          }
          MouseEventKind::ScrollDown if rect_contains(app.transcript_area, m.column, m.row) => {
            app.scroll_transcript_down(3);
          }
          _ => {}
        },
        _ => {}
      }
    }

    if last_tick.elapsed() >= tick {
      last_tick = Instant::now();
    }

    if shutdown.load(Ordering::Relaxed) {
      break;
    }
  }

  Ok(())
}

fn vertical_audio_meter(
  peak_db: f32,
  vad_speech: bool,
  area: ratatui::layout::Rect,
) -> Paragraph<'static> {
  // Map dBFS range [-60, 0] to [0, 1] for the fill ratio.
  const MIN_DB: f32 = -60.0;
  let ratio = ((peak_db - MIN_DB) / (0.0 - MIN_DB)).clamp(0.0, 1.0);

  let h = area.height as usize;
  let w = area.width as usize;

  let filled = (ratio * h as f32).round() as usize;
  let fill_style =
    if vad_speech { Style::default().fg(Color::Green) } else { Style::default().fg(Color::Cyan) };

  let mut lines = Vec::with_capacity(h);
  for row in 0..h {
    let is_filled = row >= h.saturating_sub(filled);
    let c = if is_filled { '|' } else { ' ' };
    let s = std::iter::repeat_n(c, w).collect::<String>();
    let style = if is_filled { fill_style } else { Style::default().fg(Color::DarkGray) };
    lines.push(Line::styled(s, style));
  }

  Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false })
}

fn rect_contains(r: ratatui::layout::Rect, col: u16, row: u16) -> bool {
  col >= r.x
    && col < r.x.saturating_add(r.width)
    && row >= r.y
    && row < r.y.saturating_add(r.height)
}

fn copy_to_system_clipboard(text: &str) -> Result<()> {
  let text = text.trim();
  if text.is_empty() {
    return Err(anyhow!("transcript is empty"));
  }

  #[cfg(target_os = "macos")]
  {
    let mut child = Command::new("pbcopy")
      .stdin(Stdio::piped())
      .spawn()
      .context("failed to launch pbcopy")?;

    {
      let stdin = child.stdin.as_mut().ok_or_else(|| anyhow!("failed to open pbcopy stdin"))?;
      stdin
        .write_all(text.as_bytes())
        .context("failed to write transcript to pbcopy")?;
    }

    let status = child.wait().context("failed to wait for pbcopy")?;
    if !status.success() {
      return Err(anyhow!("pbcopy exited with status {status}"));
    }
    Ok(())
  }

  #[cfg(not(target_os = "macos"))]
  {
    Err(anyhow!("system clipboard copy is currently implemented only for macOS (pbcopy)"))
  }
}
