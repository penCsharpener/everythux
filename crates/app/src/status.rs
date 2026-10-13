//! Indexer progress and transient status-bar messages.

use std::fmt::Write as _;

use everythux_index::{IndexEvent, JobId, Progress};

use crate::format::{format_count, format_duration};

/// What the indexer is doing, derived from [`IndexEvent`]s.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum IndexState {
    /// Nothing happened yet.
    #[default]
    Idle,
    /// A job is running.
    Running {
        job: JobId,
        description: String,
        progress: Option<Progress>,
    },
    /// The last job finished (or was cancelled / failed); the message stays until the next job.
    Done { message: String, failed: bool },
}

/// Indexer state plus the bits the status bar needs.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct IndexStatus {
    pub(crate) state: IndexState,
    /// Whether any job has ever completed in this session (to tell "empty index" from "loading").
    pub(crate) any_finished: bool,
}

impl IndexStatus {
    /// Whether a job is running right now.
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.state, IndexState::Running { .. })
    }

    /// Folds an indexer event into the state. Snapshot and volume events are handled elsewhere.
    pub(crate) fn on_event(&mut self, event: &IndexEvent) {
        match event {
            IndexEvent::Started { job, description } => {
                self.state = IndexState::Running {
                    job: *job,
                    description: description.clone(),
                    progress: None,
                };
            }
            IndexEvent::Progress { job, progress } => {
                if let IndexState::Running {
                    job: running,
                    progress: slot,
                    ..
                } = &mut self.state
                    && running == job
                {
                    *slot = Some(progress.clone());
                }
            }
            IndexEvent::Finished { stats, .. } => {
                self.any_finished = true;
                let mut message = format!("Indexed in {}", format_duration(stats.elapsed));
                if stats.errors > 0 {
                    let _ = write!(message, " · {} folders unreadable", format_count(stats.errors));
                }
                self.state = IndexState::Done {
                    message,
                    failed: false,
                };
            }
            IndexEvent::Cancelled { .. } => {
                self.state = IndexState::Done {
                    message: "Indexing cancelled".to_owned(),
                    failed: false,
                };
            }
            IndexEvent::Failed { error, .. } => {
                self.any_finished = true;
                self.state = IndexState::Done {
                    message: format!("Indexing failed: {error}"),
                    failed: true,
                };
            }
            IndexEvent::SnapshotReady(_) | IndexEvent::VolumesChanged(_) => {}
        }
    }

    /// The status-bar text for the indexer.
    pub(crate) fn text(&self, index_is_empty: bool) -> String {
        match &self.state {
            IndexState::Running {
                description,
                progress,
                ..
            } => {
                let mut text = description.clone();
                if let Some(p) = progress {
                    let _ = write!(text, " — {} entries", format_count(p.entries));
                    if let Some(f) = p.fraction {
                        let _ = write!(text, " · {:.0}%", (f * 100.0).clamp(0.0, 100.0));
                    }
                }
                text
            }
            IndexState::Done { message, .. } => message.clone(),
            IndexState::Idle if index_is_empty && !self.any_finished => "Indexing…".to_owned(),
            IndexState::Idle => String::new(),
        }
    }
}

/// How a toast is styled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Info,
    Error,
}

/// What clicking "Undo" on a toast does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToastUndo {
    /// Remove this exclusion rule again; `applied` says whether entries were already removed from
    /// the index, in which case the affected folders are re-indexed.
    Exclusion {
        rule: everythux_core::exclusion::ExclusionRule,
        applied: bool,
    },
}

/// A transient message in the status bar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Toast {
    pub(crate) text: String,
    pub(crate) kind: ToastKind,
    /// `egui` time (seconds) after which the toast disappears.
    pub(crate) expires: f64,
    pub(crate) undo: Option<ToastUndo>,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use everythux_index::JobStats;

    use super::*;

    fn started(job: u64) -> IndexEvent {
        IndexEvent::Started {
            job: JobId(job),
            description: "Indexing /home".to_owned(),
        }
    }

    #[test]
    fn progress_text_shows_entries_and_percentage() {
        let mut status = IndexStatus::default();
        status.on_event(&started(1));
        assert_eq!(status.text(false), "Indexing /home");
        status.on_event(&IndexEvent::Progress {
            job: JobId(1),
            progress: Progress {
                entries: 123_456,
                fraction: Some(0.43),
                ..Progress::default()
            },
        });
        assert_eq!(status.text(false), "Indexing /home — 123,456 entries · 43%");
        assert!(status.is_running());
    }

    #[test]
    fn progress_of_other_jobs_is_ignored() {
        let mut status = IndexStatus::default();
        status.on_event(&started(1));
        status.on_event(&IndexEvent::Progress {
            job: JobId(2),
            progress: Progress {
                entries: 5,
                ..Progress::default()
            },
        });
        assert_eq!(status.text(false), "Indexing /home");
    }

    #[test]
    fn finish_cancel_and_failure_messages() {
        let mut status = IndexStatus::default();
        status.on_event(&started(1));
        status.on_event(&IndexEvent::Finished {
            job: JobId(1),
            stats: JobStats {
                elapsed: Duration::from_millis(12_300),
                ..JobStats::default()
            },
        });
        assert_eq!(status.text(false), "Indexed in 12.3 s");
        assert!(!status.is_running());
        status.on_event(&IndexEvent::Cancelled { job: JobId(2) });
        assert_eq!(status.text(false), "Indexing cancelled");
        status.on_event(&IndexEvent::Failed {
            job: JobId(3),
            error: "disk full".to_owned(),
        });
        assert_eq!(status.text(false), "Indexing failed: disk full");
    }

    #[test]
    fn empty_index_before_any_job_says_indexing() {
        let status = IndexStatus::default();
        assert_eq!(status.text(true), "Indexing…");
        assert_eq!(status.text(false), "");
    }
}
