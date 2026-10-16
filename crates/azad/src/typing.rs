use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypingProgress {
  Waiting(Duration),
  Complete,
  Failed,
}

pub struct TypingCursor {
  text: String,
  offset: usize,
  interval: Duration,
  due: Instant,
}

impl TypingCursor {
  pub fn new(text: String, rate: u32, now: Instant) -> Self {
    let interval =
      if rate == 0 { Duration::ZERO } else { Duration::from_secs_f64(1.0 / f64::from(rate)) };
    Self { text, offset: 0, interval, due: now }
  }

  pub fn text(&self) -> &str {
    &self.text
  }

  pub fn advance(
    &mut self,
    now: Instant,
    emit: impl FnOnce(char) -> Result<Instant, ()>,
  ) -> TypingProgress {
    let Some(ch) = self.text[self.offset..].chars().next() else {
      return TypingProgress::Complete;
    };
    if now < self.due {
      return TypingProgress::Waiting(self.due.duration_since(now));
    }
    let Ok(emitted_at) = emit(ch) else {
      return TypingProgress::Failed;
    };
    self.offset += ch.len_utf8();
    if self.offset == self.text.len() {
      TypingProgress::Complete
    } else {
      self.due = emitted_at + self.interval;
      TypingProgress::Waiting(self.interval)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn scalar_pairs_preserve_unicode_and_wait_only_between_pairs() {
    let fixture: serde_json::Value =
      serde_json::from_str(include_str!("../docs/typing-key-rate.fixture.json")).unwrap();
    let sample = fixture["unicodeSample"].as_str().unwrap();
    for text in ["", "🧠", sample] {
      let mut now = Instant::now();
      let mut cursor = TypingCursor::new(text.to_string(), 80, now);
      let mut emitted = String::new();
      let mut waits = 0;
      loop {
        match cursor.advance(now, |ch| {
          emitted.push(ch);
          Ok(now)
        }) {
          TypingProgress::Waiting(wait) => {
            assert_eq!(wait, Duration::from_millis(12) + Duration::from_micros(500));
            waits += 1;
            now += wait;
          }
          TypingProgress::Complete => break,
          TypingProgress::Failed => panic!("recording emission failed"),
        }
      }
      assert_eq!(emitted, text);
      assert_eq!(waits, text.chars().count().saturating_sub(1));
    }
  }

  #[test]
  fn early_and_late_callbacks_never_burst_or_skip_a_scalar() {
    let start = Instant::now();
    let mut cursor = TypingCursor::new("abc".into(), 20, start);
    assert_eq!(
      cursor.advance(start, |_| Ok(start + Duration::from_millis(3))),
      TypingProgress::Waiting(Duration::from_millis(50))
    );
    assert_eq!(
      cursor.advance(start + Duration::from_millis(52), |_| panic!("too early")),
      TypingProgress::Waiting(Duration::from_millis(1))
    );
    let late = start + Duration::from_secs(2);
    assert_eq!(
      cursor.advance(late, |ch| {
        assert_eq!(ch, 'b');
        Ok(late)
      }),
      TypingProgress::Waiting(Duration::from_millis(50))
    );
    assert_eq!(
      cursor.advance(late, |_| panic!("no catch-up burst")),
      TypingProgress::Waiting(Duration::from_millis(50))
    );
    assert_eq!(
      cursor.advance(late + Duration::from_millis(50), |ch| {
        assert_eq!(ch, 'c');
        Ok(late + Duration::from_millis(50))
      }),
      TypingProgress::Complete
    );
  }

  #[test]
  fn max_has_no_added_wait_and_failure_does_not_advance() {
    let now = Instant::now();
    let mut cursor = TypingCursor::new("ab".into(), 0, now);
    assert_eq!(cursor.advance(now, |_| Err(())), TypingProgress::Failed);
    assert_eq!(
      cursor.advance(now, |ch| {
        assert_eq!(ch, 'a');
        Ok(now)
      }),
      TypingProgress::Waiting(Duration::ZERO)
    );
    assert_eq!(
      cursor.advance(now, |ch| {
        assert_eq!(ch, 'b');
        Ok(now)
      }),
      TypingProgress::Complete
    );
  }
}
