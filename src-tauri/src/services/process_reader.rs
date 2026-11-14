//! Line splitting for DepotDownloader-style child processes.
//!
//! Both the downloader and the account verifier read a child process's
//! stdout/stderr, split it into lines, and hand each line to a channel. They
//! differ only in the prefix they add ("OUT:"/"ERR:" vs nothing) and in their
//! rule for flushing a partial line early.
//!
//! That early flush exists because DepotDownloader prints some Steam Guard
//! prompts with `Console.Write` (no trailing newline) and then blocks on
//! `Console.ReadLine()`. On a redirected pipe a .NET process does not flush
//! such a partial line, so without a mid-line rule the prompt would never
//! reach us until the process was killed.

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

/// Incrementally splits a byte stream into lines.
///
/// `should_flush_partial` is consulted after every character appended to the
/// current partial line; returning `true` emits that partial line immediately
/// (and starts a fresh one). It never sees the trailing newline case, which is
/// always emitted.
pub struct LineSplitter<F> {
    current: String,
    should_flush_partial: F,
}

impl<F: Fn(&str) -> bool> LineSplitter<F> {
    pub fn new(should_flush_partial: F) -> Self {
        Self {
            current: String::new(),
            should_flush_partial,
        }
    }

    /// Feed one chunk of text, returning every line it completes.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        let mut out = Vec::new();
        for c in chunk.chars() {
            if c == '\n' {
                let line = std::mem::take(&mut self.current);
                let trimmed = line.trim_end_matches('\r');
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            } else {
                self.current.push(c);
                if (self.should_flush_partial)(&self.current) {
                    out.push(std::mem::take(&mut self.current));
                }
            }
        }
        out
    }

    /// Flush any trailing partial line at EOF.
    pub fn finish(self) -> Option<String> {
        if self.current.is_empty() {
            None
        } else {
            Some(self.current)
        }
    }
}

/// Read `reader` to EOF, splitting it into lines and sending each one to
/// `tx` prefixed with `prefix`.
///
/// Runs to completion on its own task; the caller drops its sender clone so
/// the channel closes when every reader has finished.
pub async fn spawn_line_reader<R, F>(
    mut reader: R,
    tx: mpsc::Sender<String>,
    prefix: &'static str,
    should_flush_partial: F,
) where
    R: AsyncReadExt + Unpin,
    F: Fn(&str) -> bool + Send + 'static,
{
    let mut splitter = LineSplitter::new(should_flush_partial);
    let mut buf = [0u8; 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                for line in splitter.push(&chunk) {
                    if tx.send(format!("{prefix}{line}")).await.is_err() {
                        return;
                    }
                }
            }
            Err(_) => break,
        }
    }
    if let Some(rest) = splitter.finish() {
        let _ = tx.send(format!("{prefix}{rest}")).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never(_: &str) -> bool {
        false
    }

    #[test]
    fn splits_on_newlines_and_trims_cr() {
        let mut s = LineSplitter::new(never);
        let mut lines = s.push("one\r\ntwo\n");
        lines.extend(s.finish());
        assert_eq!(lines, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn keeps_partial_line_across_chunks() {
        let mut s = LineSplitter::new(never);
        assert!(s.push("par").is_empty());
        assert!(s.push("tial").is_empty());
        assert_eq!(s.push("\nnext"), vec!["partial".to_string()]);
        assert_eq!(s.finish(), Some("next".to_string()));
    }

    #[test]
    fn drops_empty_lines() {
        let mut s = LineSplitter::new(never);
        assert!(s.push("\n\n").is_empty());
    }

    #[test]
    fn flushes_a_partial_prompt_when_the_rule_matches() {
        // Mirror the real rule: a Steam Guard prompt blocks on input without a
        // newline, and it always ends with a colon. Flushing on the trailing
        // colon (not on an inner keyword) avoids cutting the prompt in half.
        let rule = |line: &str| {
            let lower = line.to_lowercase();
            lower.trim_end().ends_with(':') && lower.contains("auth code")
        };
        let mut s = LineSplitter::new(rule);
        // Nothing flushes while the keyword appears mid-line without the colon.
        assert!(s.push("Please enter your auth code").is_empty());
        // The colon completes the prompt and triggers exactly one flush.
        let lines = s.push(":");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with(':'));
        // The buffer is reset, so the next line starts clean.
        assert_eq!(s.push("ok\n"), vec!["ok".to_string()]);
    }

    #[test]
    fn partial_flush_fires_at_most_once_per_line() {
        let rule = |line: &str| line.contains(":");
        let mut s = LineSplitter::new(rule);
        let lines = s.push("code:");
        assert_eq!(lines.len(), 1);
        // Subsequent characters accumulate into a fresh line.
        assert!(s.push("more").is_empty());
        assert_eq!(s.finish(), Some("more".to_string()));
    }
}