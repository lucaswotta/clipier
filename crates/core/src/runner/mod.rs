//! Starts a process, copies what it prints to a file as it arrives, and stops it when it takes too
//! long, goes silent or is cancelled. The file means a killed run still leaves what it printed.

mod capture;
mod failure;

pub use capture::{Captured, capture};
pub use failure::{Failure, classify};

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::platform;

/// Set in every process Leme starts, so that an agent cannot start runs of its own.
pub const NESTING_GUARD: &str = "LEME_PIER_RUN";
const POLL: Duration = Duration::from_millis(50);
const STDERR_TAIL: usize = 4096;

/// A process to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env_set: Vec<(String, String)>,
    /// Names of variables the process must not see.
    pub env_remove: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The whole run, from start to finish.
    pub total: Duration,
    /// Longest the process may stay alive without printing anything.
    pub stall: Duration,
}

#[derive(Debug)]
pub struct Ran {
    /// `None` when the process was killed.
    pub exit_code: Option<i32>,
    /// The last bytes the process wrote to its error stream.
    pub stderr_tail: String,
    pub stalled: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration: Duration,
}

/// Names of environment variables that look like secrets: tokens, passwords, API keys.
pub fn secret_names(vars: &[(String, String)]) -> Vec<String> {
    const MARKERS: [&str; 5] = ["TOKEN", "SECRET", "PASSW", "CREDENTIAL", "API_KEY"];
    vars.iter()
        .map(|(name, _)| name)
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            MARKERS.iter().any(|marker| upper.contains(marker))
                || upper.contains("APIKEY")
                || upper.starts_with("VAULT_")
        })
        .cloned()
        .collect()
}

/// Copies a stream into `sink` while noting when it last produced anything.
fn pump(mut stream: impl Read, mut sink: impl FnMut(&[u8]), last_output_ms: &AtomicU64, started: Instant) {
    let mut buffer = [0u8; 16 * 1024];
    while let Ok(n) = stream.read(&mut buffer) {
        if n == 0 {
            break;
        }
        last_output_ms.store(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
        sink(&buffer[..n]);
    }
}

fn keep_tail(text: &mut String, bytes: &[u8]) {
    text.push_str(&String::from_utf8_lossy(bytes));
    if text.len() > STDERR_TAIL {
        let cut = text.len() - STDERR_TAIL;
        let boundary = (cut..text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(text.len());
        text.drain(..boundary);
    }
}

/// Runs `spawn` to the end, or until a limit or `cancel` stops it. Everything it prints goes to
/// `out_path` and to `on_output`, as it arrives. The whole process tree is ended when it is stopped.
pub fn run(
    spawn: &Spawn,
    out_path: &Path,
    limits: Limits,
    cancel: &AtomicBool,
    on_output: impl FnMut(&[u8]) + Send,
) -> io::Result<Ran> {
    let mut command = Command::new(&spawn.program);
    command
        .args(&spawn.args)
        .current_dir(&spawn.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    platform::lead_group(&mut command);
    for name in &spawn.env_remove {
        command.env_remove(name);
    }
    command.envs(spawn.env_set.iter().map(|(name, value)| (name, value)));

    let started = Instant::now();
    let mut child =
        platform::start(&mut command).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", spawn.program)))?;
    let out_file = Mutex::new(File::create(out_path)?);
    let stderr_text = Mutex::new(String::new());
    let last_output_ms = AtomicU64::new(0);
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let (mut stalled, mut timed_out, mut cancelled) = (false, false, false);
    let (out_file, stderr_text, last_output_ms) = (&out_file, &stderr_text, &last_output_ms);
    let exit_code = thread::scope(|scope| -> io::Result<Option<i32>> {
        scope.spawn(move || {
            let mut on_output = on_output;
            pump(
                stdout,
                |bytes| {
                    let _ = out_file.lock().expect("the output lock is never poisoned").write_all(bytes);
                    on_output(bytes);
                },
                last_output_ms,
                started,
            );
        });
        scope.spawn(move || {
            pump(
                stderr,
                |bytes| keep_tail(&mut stderr_text.lock().expect("the stderr lock is never poisoned"), bytes),
                last_output_ms,
                started,
            );
        });
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status.code());
            }
            let elapsed = started.elapsed();
            let silent_for = elapsed.saturating_sub(Duration::from_millis(last_output_ms.load(Ordering::Relaxed)));
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
            } else if elapsed >= limits.total {
                timed_out = true;
            } else if silent_for >= limits.stall {
                stalled = true;
            } else {
                thread::sleep(POLL);
                continue;
            }
            platform::kill_tree(&mut child);
            return Ok(None);
        }
    })?;

    Ok(Ran {
        exit_code,
        stderr_tail: stderr_text.lock().expect("the stderr lock is never poisoned").clone(),
        stalled,
        timed_out,
        cancelled,
        duration: started.elapsed(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("leme-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Runs `script` through `sh`: executing a file that was just written can fail with "Text file
    /// busy" when another thread forks at the same moment.
    fn spawn_script(dir: &Path, script: &str) -> Spawn {
        let path = dir.join("agent.sh");
        std::fs::write(&path, format!("{script}\n")).unwrap();
        Spawn {
            program: "/bin/sh".into(),
            args: vec![path.display().to_string()],
            cwd: dir.to_owned(),
            env_set: Vec::new(),
            env_remove: Vec::new(),
        }
    }

    fn limits(total_ms: u64, stall_ms: u64) -> Limits {
        Limits { total: Duration::from_millis(total_ms), stall: Duration::from_millis(stall_ms) }
    }

    fn go(name: &str, script: &str, limits: Limits) -> (Ran, String, String) {
        let dir = folder(name);
        let seen = Mutex::new(String::new());
        let ran = run(&spawn_script(&dir, script), &dir.join("out.txt"), limits, &AtomicBool::new(false), |bytes| {
            seen.lock().unwrap().push_str(&String::from_utf8_lossy(bytes));
        })
        .unwrap();
        let written = std::fs::read_to_string(dir.join("out.txt")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (ran, written, seen.into_inner().unwrap())
    }

    #[test]
    fn what_a_run_prints_reaches_the_file_and_the_listener_and_its_exit_code_is_kept() {
        let (ran, written, seen) = go("ok", "echo hello; echo 'quota exceeded' >&2; exit 3", limits(10_000, 10_000));
        assert_eq!(ran.exit_code, Some(3));
        assert!(!ran.stalled && !ran.timed_out && !ran.cancelled);
        assert_eq!((written.trim(), seen.trim()), ("hello", "hello"));
        assert!(ran.stderr_tail.contains("quota exceeded"));
    }

    #[test]
    fn a_process_that_goes_silent_is_killed_and_what_it_printed_is_kept() {
        let (ran, written, _) = go("stall", "echo hello\nsleep 30", limits(20_000, 1_500));
        assert!(ran.stalled && !ran.timed_out);
        assert_eq!(ran.exit_code, None);
        assert!(ran.duration < Duration::from_secs(10));
        assert_eq!(written.trim(), "hello");
    }

    #[test]
    fn a_run_over_its_total_time_is_killed_even_while_it_keeps_printing() {
        let (ran, _, _) = go("timeout", "while true; do echo tick; sleep 0.1; done", limits(800, 10_000));
        assert!(ran.timed_out && !ran.stalled);
        assert!(ran.duration < Duration::from_secs(10));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_whole_process_tree_dies_with_a_cancelled_run() {
        let dir = folder("tree");
        let spawn = spawn_script(&dir, "sleep 60 &\necho $! > child.pid\nwait");
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let pid_file = dir.join("child.pid");
        let stopper = thread::spawn(move || {
            // Cancel only once the script has started its background child.
            for _ in 0..200 {
                if std::fs::read_to_string(&pid_file).is_ok_and(|text| text.trim().parse::<i32>().is_ok()) {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            flag.store(true, Ordering::Relaxed);
        });
        let ran = run(&spawn, &dir.join("out.txt"), limits(20_000, 20_000), &cancel, |_| {}).unwrap();
        stopper.join().unwrap();
        assert!(ran.cancelled);
        let child: i32 = std::fs::read_to_string(dir.join("child.pid")).unwrap().trim().parse().unwrap();
        thread::sleep(Duration::from_millis(200));
        assert!(!Path::new(&format!("/proc/{child}")).exists(), "the grandchild must not outlive the cancelled run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_that_does_not_exist_is_an_error_naming_it() {
        let dir = folder("missing");
        let spawn = Spawn { program: "leme-no-such-agent".into(), ..spawn_script(&dir, "") };
        let error = run(&spawn, &dir.join("out.txt"), limits(1_000, 1_000), &AtomicBool::new(false), |_| {})
            .expect_err("it cannot start");
        assert!(error.to_string().contains("leme-no-such-agent"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secrets_are_removed_and_the_guard_is_set_for_the_process() {
        let dir = folder("env");
        let mut spawn = spawn_script(&dir, r#"echo "[$MY_API_KEY][$LEME_PIER_RUN]""#);
        // SAFETY: the test sets a variable of its own name and nothing else reads it.
        unsafe { std::env::set_var("MY_API_KEY", "hunter2") };
        spawn.env_remove = secret_names(&[("MY_API_KEY".into(), "hunter2".into()), ("PATH".into(), "/bin".into())]);
        spawn.env_set = vec![(NESTING_GUARD.into(), "1".into())];
        let seen = Mutex::new(String::new());
        run(&spawn, &dir.join("out.txt"), limits(5_000, 5_000), &AtomicBool::new(false), |bytes| {
            seen.lock().unwrap().push_str(&String::from_utf8_lossy(bytes));
        })
        .unwrap();
        assert_eq!(seen.into_inner().unwrap().trim(), "[][1]");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
