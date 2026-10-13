//! External helper programs (`pdftoppm`, `ffmpeg`, …): discovery via `PATH` and supervised execution.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, bounded};

use crate::cancel::Cancel;
use crate::error::PreviewError;

/// How often a running helper is checked for cancellation and timeout.
const POLL: Duration = Duration::from_millis(10);
/// A helper that has not been found is looked up again after this long (the user may install it).
const NEGATIVE_TTL: Duration = Duration::from_secs(30);
/// Upper bound on what is read from a helper's stdout.
const MAX_OUTPUT: u64 = 128 * 1024 * 1024;

#[derive(Debug)]
struct Lookup {
    found: Option<PathBuf>,
    at: Instant,
}

/// Cached `PATH` lookups for helper programs.
#[derive(Debug)]
pub(crate) struct Tools {
    search_path: OsString,
    cache: Mutex<HashMap<&'static str, Lookup>>,
}

impl Tools {
    /// Looks programs up in `$PATH`.
    pub(crate) fn from_env() -> Self {
        Self::with_path(std::env::var_os("PATH").unwrap_or_default())
    }

    /// Looks programs up in an explicit `PATH`-style list (tests).
    pub(crate) fn with_path(search_path: impl Into<OsString>) -> Self {
        Self {
            search_path: search_path.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Absolute path of `name`, searching `PATH` once and remembering the answer.
    pub(crate) fn locate(&self, name: &'static str) -> Option<PathBuf> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(hit) = cache.get(name)
            && (hit.found.is_some() || hit.at.elapsed() < NEGATIVE_TTL)
        {
            return hit.found.clone();
        }
        let found = find_in_path(name, &self.search_path);
        cache.insert(
            name,
            Lookup {
                found: found.clone(),
                at: Instant::now(),
            },
        );
        found
    }
}

/// First executable regular file called `name` in the `PATH`-style list.
pub(crate) fn find_in_path(name: &str, search_path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(search_path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Kills and reaps the child unless it already finished.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawns, retrying briefly on `ETXTBSY`: a helper that is being installed or upgraded (or, in
/// tests, a script some other thread has just written) can be momentarily busy.
fn spawn(command: &mut Command) -> std::io::Result<Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 20 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

/// Runs `program args…`, returning its stdout.
///
/// The helper is killed when `cancel` fires or `timeout` elapses, and a non-zero exit is an error.
/// Stdin and stderr are closed.
pub(crate) fn run_capture(
    tool: &'static str,
    program: &Path,
    args: &[OsString],
    timeout: Duration,
    cancel: &Cancel,
) -> Result<Vec<u8>, PreviewError> {
    cancel.check()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = spawn(&mut command)?;
    let stdout = child.stdout.take();
    let mut guard = ChildGuard(child);
    let deadline = Instant::now() + timeout;

    // Drain stdout on its own thread so a full pipe never stalls the helper. If we bail out, the
    // kill closes the pipe and the thread ends by itself.
    let (tx, rx) = bounded::<Vec<u8>>(1);
    if let Some(stdout) = stdout {
        thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.take(MAX_OUTPUT).read_to_end(&mut out);
            let _ = tx.send(out);
        });
    } else {
        drop(tx);
    }

    let supervise = || -> Result<(), PreviewError> {
        cancel.check()?;
        if Instant::now() >= deadline {
            return Err(PreviewError::Timeout { tool });
        }
        Ok(())
    };

    let output = loop {
        match rx.recv_timeout(POLL) {
            Ok(out) => break out,
            Err(RecvTimeoutError::Timeout) => supervise()?,
            Err(RecvTimeoutError::Disconnected) => break Vec::new(),
        }
    };
    loop {
        if let Some(status) = guard.0.try_wait()? {
            return if status.success() {
                Ok(output)
            } else {
                Err(PreviewError::ToolFailed { tool })
            };
        }
        supervise()?;
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    #[test]
    fn finds_executables_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        script(dir.path(), "tool-a", "true");
        fs::write(dir.path().join("tool-b"), "x").expect("write");
        let path = std::env::join_paths([Path::new("/nonexistent-dir"), dir.path()]).expect("join");
        assert_eq!(find_in_path("tool-a", &path), Some(dir.path().join("tool-a")));
        assert_eq!(find_in_path("tool-b", &path), None, "not executable");
        assert_eq!(find_in_path("tool-c", &path), None);
    }

    #[test]
    fn lookup_is_cached() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = script(dir.path(), "cached-tool", "true");
        let tools = Tools::with_path(dir.path());
        assert_eq!(tools.locate("cached-tool"), Some(tool.clone()));
        fs::remove_file(&tool).expect("remove");
        assert_eq!(
            tools.locate("cached-tool"),
            Some(tool),
            "second lookup must not hit the disk"
        );
    }

    #[test]
    fn captures_stdout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = script(dir.path(), "echoer", "printf 'hello %s' \"$1\"");
        let out = run_capture(
            "echoer",
            &tool,
            &[OsString::from("world")],
            Duration::from_secs(10),
            &Cancel::never(),
        )
        .expect("run");
        assert_eq!(out, b"hello world");
    }

    #[test]
    fn nonzero_exit_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = script(dir.path(), "failer", "echo partial; exit 3");
        let err = run_capture("failer", &tool, &[], Duration::from_secs(10), &Cancel::never())
            .expect_err("must fail");
        assert!(matches!(err, PreviewError::ToolFailed { .. }), "{err:?}");
    }

    #[test]
    fn slow_helper_times_out_and_is_killed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = script(dir.path(), "sleeper", "exec sleep 60");
        let started = Instant::now();
        let err = run_capture(
            "sleeper",
            &tool,
            &[],
            Duration::from_millis(100),
            &Cancel::never(),
        )
        .expect_err("must time out");
        assert!(matches!(err, PreviewError::Timeout { .. }), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "kill must not wait for sleep to end"
        );
    }

    #[test]
    fn cancel_kills_helper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let tool = script(
            dir.path(),
            "waiter",
            &format!("echo $$ > '{}'\nexec sleep 60", pid_file.display()),
        );
        let latest = Arc::new(AtomicU64::new(1));
        let cancel = Cancel::new(Arc::clone(&latest), 1);
        let canceller = {
            let pid_file = pid_file.clone();
            thread::spawn(move || {
                // Cancel only once the helper is demonstrably running.
                while fs::read_to_string(&pid_file).map_or(true, |s| s.trim().is_empty()) {
                    thread::yield_now();
                }
                latest.store(2, Ordering::Relaxed);
            })
        };
        let err = run_capture("waiter", &tool, &[], Duration::from_secs(60), &cancel).expect_err("cancelled");
        canceller.join().expect("canceller");
        assert!(err.is_cancelled(), "{err:?}");
        let pid = fs::read_to_string(&pid_file).expect("pid").trim().to_owned();
        assert!(
            !Path::new("/proc").join(pid).exists(),
            "helper process must be gone after cancellation (killed and reaped)"
        );
    }

    #[test]
    fn missing_program_is_an_io_error() {
        let err = run_capture(
            "nope",
            Path::new("/nonexistent/nope"),
            &[],
            Duration::from_secs(1),
            &Cancel::never(),
        )
        .expect_err("spawn must fail");
        assert!(matches!(err, PreviewError::Io(_)), "{err:?}");
    }
}
