//! Runs a short command to the end and returns what it printed, for asking a CLI a question.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::platform;

const POLL: Duration = Duration::from_millis(50);

pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

impl Captured {
    /// Both streams, for the CLIs that answer on the error stream.
    pub fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

fn read_all(mut stream: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut text = String::new();
        let _ = stream.read_to_string(&mut text);
        text
    })
}

/// Runs `program` with `args` and waits up to `limit`. `None` when it cannot start or does not
/// finish in time (it is stopped, with whatever it started).
pub fn capture(program: &Path, args: &[&str], limit: Duration) -> Option<Captured> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    platform::lead_group(&mut command);
    let mut child = platform::start(&mut command).ok()?;
    let stdout = read_all(child.stdout.take()?);
    let stderr = read_all(child.stderr.take()?);
    let deadline = Instant::now() + limit;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            _ => {
                platform::kill_tree(&mut child);
                return None;
            }
        }
    };
    Some(Captured { stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default(), success })
}
