use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct StabilityTracker {
  stable_k: usize,
  max_history: usize,
  max_overlap: usize,

  committed: Vec<String>,
  history: VecDeque<Vec<String>>, // uncommitted continuation tokens, newest at back
}

impl StabilityTracker {
  pub fn new(stable_k: usize, max_history: usize) -> Self {
    Self {
      stable_k: stable_k.max(1),
      max_history: max_history.max(1),
      max_overlap: 30,
      committed: Vec::new(),
      history: VecDeque::new(),
    }
  }

  pub fn reset(&mut self) {
    self.committed.clear();
    self.history.clear();
  }

  pub fn update(&mut self, hypothesis: &str) -> (String, String) {
    let hyp = normalize_ws(hypothesis);
    let hyp_tokens = tokenize(&hyp);

    // Align the new hypothesis against the end of what we've already committed to avoid
    // duplicate words when the ASR window slides forward.
    let overlap = find_overlap(&self.committed, &hyp_tokens, self.max_overlap);
    let cont = hyp_tokens.into_iter().skip(overlap).collect::<Vec<_>>();

    self.history.push_back(cont);
    while self.history.len() > self.max_history {
      self.history.pop_front();
    }

    self.commit_stable_prefix();

    (self.committed_text(), self.live_text())
  }

  pub fn committed_text(&self) -> String {
    self.committed.join(" ")
  }

  pub fn live_text(&self) -> String {
    let live = self.history.back().map(|v| v.join(" ")).unwrap_or_default();

    if live.is_empty() {
      return String::new();
    }
    if self.committed.is_empty() { live } else { format!(" {}", live) }
  }

  pub fn full_text(&self) -> String {
    format!("{}{}", self.committed_text(), self.live_text()).trim().to_string()
  }

  fn commit_stable_prefix(&mut self) {
    if self.history.len() < self.stable_k {
      return;
    }

    let tail: Vec<&Vec<String>> = self
      .history
      .iter()
      .rev()
      .take(self.stable_k)
      .collect::<Vec<_>>()
      .into_iter()
      .rev()
      .collect();

    let prefix_len = lcp_len(&tail);
    if prefix_len == 0 {
      return;
    }

    // Commit tokens.
    if let Some(latest) = self.history.back() {
      let to_commit = latest.iter().take(prefix_len).cloned().collect::<Vec<_>>();
      self.committed.extend(to_commit);
    }

    // Remove committed tokens from all history entries so future LCP comparisons are relative
    // to the uncommitted continuation.
    for entry in self.history.iter_mut() {
      let n = prefix_len.min(entry.len());
      entry.drain(..n);
    }
  }
}

fn normalize_ws(s: &str) -> String {
  s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tokenize(s: &str) -> Vec<String> {
  s.split_whitespace().map(|t| t.to_string()).collect()
}

fn lcp_len(vs: &[&Vec<String>]) -> usize {
  if vs.is_empty() {
    return 0;
  }
  let min_len = vs.iter().map(|v| v.len()).min().unwrap_or(0);
  for i in 0..min_len {
    let tok = &vs[0][i];
    if !vs.iter().all(|v| v[i] == *tok) {
      return i;
    }
  }
  min_len
}

fn find_overlap(committed: &[String], hyp: &[String], max_overlap: usize) -> usize {
  if committed.is_empty() || hyp.is_empty() {
    return 0;
  }

  // Common case for incremental / monotonic hypotheses:
  // the new hypothesis contains the entire committed prefix. In that case we should
  // skip all committed tokens, otherwise `live` will include the whole hypothesis
  // and the UI will show duplicated text (`committed` + `live`).
  if committed.len() <= hyp.len() && hyp[..committed.len()] == *committed {
    return committed.len();
  }

  let max = max_overlap.min(committed.len()).min(hyp.len());
  for o in (1..=max).rev() {
    if committed[committed.len() - o..] == hyp[..o] {
      return o;
    }
  }
  0
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn commits_when_last_k_agree() {
    let mut t = StabilityTracker::new(3, 5);
    t.update("hello world");
    t.update("hello world there");
    let (c, l) = t.update("hello world there friend");
    assert_eq!(c, "hello world");
    assert_eq!(l, " there friend");
  }

  #[test]
  fn overlap_prevents_duplication_when_window_shifts() {
    let mut t = StabilityTracker::new(2, 5);

    // Two consistent hypotheses -> commit.
    t.update("hello world this is");
    t.update("hello world this is a");
    assert_eq!(t.committed_text(), "hello world this is");

    // Window shifts forward and drops "hello" but overlaps on tail.
    t.update("world this is a test");
    t.update("world this is a test now");

    let full = t.full_text();
    assert!(full.starts_with("hello world this is"));
    assert!(full.contains("a test"));
  }

  #[test]
  fn monotonic_hypothesis_does_not_duplicate_committed() {
    let mut t = StabilityTracker::new(1, 5);
    let (c1, l1) = t.update("one two three four five six seven eight nine ten");
    assert_eq!(c1, "one two three four five six seven eight nine ten");
    assert_eq!(l1, "");

    // Hypothesis grows by appending tokens at the end.
    let (c2, l2) = t.update("one two three four five six seven eight nine ten eleven twelve");
    assert_eq!(c2, "one two three four five six seven eight nine ten eleven twelve");
    assert_eq!(l2, "");
  }
}
