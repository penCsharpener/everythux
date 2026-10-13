//! Progress reporting for `reindex`: a single updating line on a terminal, plain lines otherwise.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use crossbeam_channel::Receiver;
use everythux_index::{IndexEvent, JobId, JobStats};

use crate::render;

/// Width used to cut the progress line (no terminal-size query; 80 columns is safe everywhere).
const LINE_WIDTH: usize = 100;
const TTY_INTERVAL: Duration = Duration::from_millis(100);
const PLAIN_INTERVAL: Duration = Duration::from_secs(2);

/// Writes indexer events to `out` (normally stderr). Write errors are ignored: losing a progress
/// line must never abort an index run.
#[derive(Debug)]
pub(crate) struct Reporter<W: Write> {
    out: W,
    tty: bool,
    last_progress: Option<Instant>,
    line_open: bool,
}

impl<W: Write> Reporter<W> {
    pub(crate) const fn new(out: W, tty: bool) -> Self {
        Self {
            out,
            tty,
            last_progress: None,
            line_open: false,
        }
    }

    pub(crate) fn event(&mut self, event: &IndexEvent) {
        let _ = self.try_event(event);
    }

    fn try_event(&mut self, event: &IndexEvent) -> std::io::Result<()> {
        match event {
            IndexEvent::Started { description, .. } => {
                self.close_line()?;
                writeln!(self.out, "{description}")?;
            }
            IndexEvent::Progress { progress, .. } => {
                let interval = if self.tty { TTY_INTERVAL } else { PLAIN_INTERVAL };
                if self.last_progress.is_some_and(|t| t.elapsed() < interval) {
                    return Ok(());
                }
                self.last_progress = Some(Instant::now());
                let text = render::render_progress(progress, LINE_WIDTH);
                if self.tty {
                    write!(self.out, "\r\x1b[2K{text}")?;
                    self.line_open = true;
                } else {
                    writeln!(self.out, "{text}")?;
                }
                self.out.flush()?;
            }
            IndexEvent::Finished { .. } | IndexEvent::Cancelled { .. } | IndexEvent::Failed { .. } => {
                self.close_line()?;
            }
            IndexEvent::SnapshotReady(_) | IndexEvent::VolumesChanged(_) => {}
        }
        Ok(())
    }

    /// Erases the in-place progress line so following output starts on a clean line.
    fn close_line(&mut self) -> std::io::Result<()> {
        if self.line_open {
            write!(self.out, "\r\x1b[2K")?;
            self.line_open = false;
        }
        Ok(())
    }
}

/// Reports events until `job` ends. Events of other jobs (an empty database also starts a full
/// scan at startup) are shown but don't end the wait.
pub(crate) fn wait_for_job<W: Write>(
    events: &Receiver<IndexEvent>,
    job: JobId,
    reporter: &mut Reporter<W>,
) -> Result<JobStats> {
    loop {
        let event = events
            .recv()
            .context("the indexer stopped before the job finished")?;
        reporter.event(&event);
        match event {
            IndexEvent::Finished { job: id, stats } if id == job => return Ok(stats),
            IndexEvent::Failed { job: id, error } if id == job => bail!("indexing failed: {error}"),
            IndexEvent::Cancelled { job: id } if id == job => bail!("indexing was cancelled"),
            other => tracing::debug!(?other, "event for another job or informational"),
        }
    }
}

#[cfg(test)]
mod tests {
    use everythux_index::Progress;

    use super::*;

    fn started(job: u64, description: &str) -> IndexEvent {
        IndexEvent::Started {
            job: JobId(job),
            description: description.to_owned(),
        }
    }

    fn progress(job: u64, entries: u64) -> IndexEvent {
        IndexEvent::Progress {
            job: JobId(job),
            progress: Progress {
                entries,
                ..Progress::default()
            },
        }
    }

    fn finished(job: u64, upserted: u64) -> IndexEvent {
        IndexEvent::Finished {
            job: JobId(job),
            stats: JobStats {
                upserted,
                ..JobStats::default()
            },
        }
    }

    #[test]
    fn waits_for_its_own_job_and_ignores_the_startup_job() {
        let (tx, rx) = crossbeam_channel::unbounded();
        for event in [
            started(1, "Initial scan"),
            progress(1, 10),
            finished(1, 111),
            started(2, "Rescanning"),
            finished(2, 222),
            started(3, "never read"),
        ] {
            tx.send(event).unwrap();
        }
        let mut out = Vec::new();
        let stats = wait_for_job(&rx, JobId(2), &mut Reporter::new(&mut out, false)).unwrap();
        assert_eq!(stats.upserted, 222);
        assert_eq!(rx.len(), 1, "events after the job ended stay unread");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("Initial scan") && text.contains("Rescanning"),
            "{text}"
        );
        assert!(
            !text.contains('\r'),
            "plain mode has no carriage returns: {text:?}"
        );
    }

    #[test]
    fn failure_and_cancellation_are_errors() {
        for (event, expected) in [
            (
                IndexEvent::Failed {
                    job: JobId(7),
                    error: "disk full".to_owned(),
                },
                "disk full",
            ),
            (IndexEvent::Cancelled { job: JobId(7) }, "cancelled"),
        ] {
            let (tx, rx) = crossbeam_channel::unbounded();
            tx.send(event).unwrap();
            let err = wait_for_job(&rx, JobId(7), &mut Reporter::new(Vec::new(), false)).unwrap_err();
            assert!(err.to_string().contains(expected), "{err}");
        }
    }

    #[test]
    fn a_closed_channel_is_an_error() {
        let (tx, rx) = crossbeam_channel::unbounded::<IndexEvent>();
        drop(tx);
        let err = wait_for_job(&rx, JobId(1), &mut Reporter::new(Vec::new(), false)).unwrap_err();
        assert!(err.to_string().contains("indexer stopped"), "{err}");
    }

    #[test]
    fn terminal_mode_rewrites_one_line_and_clears_it() {
        let mut out = Vec::new();
        let mut reporter = Reporter::new(&mut out, true);
        reporter.event(&progress(1, 5));
        reporter.event(&finished(1, 5));
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("\r\x1b[2K") && text.contains("0 folders, 5 entries"),
            "{text:?}"
        );
        assert!(
            text.ends_with("\r\x1b[2K"),
            "line is cleared at the end: {text:?}"
        );
        assert!(!text.contains('\n'), "{text:?}");
    }

    #[test]
    fn plain_mode_throttles_progress_lines() {
        let mut out = Vec::new();
        let mut reporter = Reporter::new(&mut out, false);
        for n in 0..50 {
            reporter.event(&progress(1, n));
        }
        let lines = String::from_utf8(out).unwrap().lines().count();
        assert_eq!(lines, 1);
    }
}
