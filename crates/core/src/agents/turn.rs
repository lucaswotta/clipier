//! One turn with an agent: the request goes in, the reply and what it cost come out. The command
//! is built, run under its limits, and the output is read as it arrives.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Environment, Event, Request, Transcript, build};
use crate::runner::{Failure, Limits, classify, run};

#[derive(Debug)]
pub struct Turn {
    pub answer: Result<String, (Failure, String)>,
    /// The id the CLI uses to resume this conversation.
    pub session_id: Option<String>,
    pub events: Vec<Event>,
    /// Input read fresh, at full price.
    pub tokens_in: u64,
    /// Input served from the provider's cache, at a fraction of the price.
    pub tokens_cached: u64,
    pub tokens_out: u64,
    pub duration: Duration,
}

impl Turn {
    fn failed(failure: Failure, detail: String) -> Self {
        Self {
            answer: Err((failure, detail)),
            session_id: None,
            events: Vec::new(),
            tokens_in: 0,
            tokens_cached: 0,
            tokens_out: 0,
            duration: Duration::ZERO,
        }
    }

    /// What the turn cost, counting a cached input token as a tenth of a fresh one.
    pub fn effective_tokens(&self) -> u64 {
        self.tokens_in.saturating_add(self.tokens_out).saturating_add(self.tokens_cached / 10)
    }
}

/// Where the output of `request` is written: `<out_dir>/<label>.out`.
pub fn output_file(request: &Request) -> PathBuf {
    request.out_dir.join(format!("{}.out", request.label))
}

/// Runs one turn and waits for it. `cancel` stops it from outside; the process tree is ended before
/// this returns.
pub fn take_turn(request: &Request, environment: &Environment, stall: Duration, cancel: &AtomicBool) -> Turn {
    let prepared = match build(request, environment) {
        Ok(prepared) => prepared,
        Err(unavailable) => return Turn::failed(Failure::Unusable, unavailable.to_string()),
    };
    if let Err(error) = fs::create_dir_all(&request.out_dir) {
        return Turn::failed(Failure::Transient, error.to_string());
    }
    let transcript = Arc::new(Mutex::new(Transcript::new(request.agent)));
    let limits = Limits { total: request.total, stall: stall.min(request.total) };
    let feed = Arc::clone(&transcript);
    let ran = match run(&prepared.spawn, &output_file(request), limits, cancel, move |bytes| {
        feed.lock().expect("the transcript lock is never poisoned").feed(bytes);
    }) {
        Ok(ran) => ran,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Turn::failed(Failure::Unusable, format!("{} is not installed: {error}", request.agent.name()));
        }
        Err(error) => return Turn::failed(Failure::Transient, error.to_string()),
    };
    let transcript = transcript.lock().expect("the transcript lock is never poisoned");
    let mut turn = Turn {
        answer: Ok(String::new()),
        session_id: transcript.session_id().map(str::to_owned).or(prepared.session_id),
        events: transcript.events().to_vec(),
        tokens_in: transcript.tokens_in(),
        tokens_cached: transcript.tokens_cached(),
        tokens_out: transcript.tokens_out(),
        duration: ran.duration,
    };
    if ran.cancelled {
        turn.answer = Err((Failure::Cancelled, "cancelled".into()));
        return turn;
    }
    if ran.timed_out {
        turn.answer = Err((Failure::Transient, format!("no answer within {}s", request.total.as_secs())));
        return turn;
    }
    if ran.stalled {
        turn.answer =
            Err((Failure::Transient, format!("no output for {}s; the process was stopped", limits.stall.as_secs())));
        return turn;
    }
    let from_file = prepared.last_message_file.as_ref().and_then(|path| fs::read_to_string(path).ok());
    let text = transcript.final_text().map(str::to_owned).or(from_file).unwrap_or_default();
    if ran.exit_code != Some(0) || text.trim().is_empty() {
        let reported: Vec<&str> = turn
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Error { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let detail = format!("{}\n{}", ran.stderr_tail.trim(), reported.join("\n")).trim().to_owned();
        let detail = if detail.is_empty() { format!("exit code {:?} and no answer", ran.exit_code) } else { detail };
        turn.answer = Err((classify(&detail), detail));
        return turn;
    }
    turn.answer = Ok(text);
    turn
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agents::{Agent, Mode, Profile, Session};

    /// A temporary folder with a fake `claude` that prints what the test says.
    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        fn new(name: &str, script: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("clipier-turn-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("bin")).unwrap();
            let path = dir.join("bin/claude");
            fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn environment(&self) -> Environment {
            Environment {
                home: self.dir.clone(),
                path_prefix: vec![self.dir.join("bin")],
                vars: vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
            }
        }

        fn request(&self) -> Request {
            Request {
                agent: Agent::Claude,
                model: "m".into(),
                effort: None,
                mode: Mode::Write,
                profile: Profile::Focused,
                session: Session::New,
                prompt: "do it".into(),
                cwd: self.dir.clone(),
                out_dir: self.dir.join("out"),
                label: "0001-claude".into(),
                total: Duration::from_secs(20),
            }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn turn(fake: &Fake) -> Turn {
        take_turn(&fake.request(), &fake.environment(), Duration::from_secs(10), &AtomicBool::new(false))
    }

    #[test]
    fn a_reply_comes_back_with_its_session_and_what_it_cost() {
        let fake = Fake::new(
            "ok",
            r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-1"}'
printf '%s\n' '{"type":"result","session_id":"sess-1","is_error":false,"result":"done","usage":{"input_tokens":3,"cache_read_input_tokens":20,"output_tokens":4}}'"#,
        );
        let turn = turn(&fake);
        assert_eq!(turn.answer.as_ref().unwrap(), "done");
        assert_eq!(turn.session_id.as_deref(), Some("sess-1"));
        assert_eq!(turn.effective_tokens(), 3 + 4 + 2);
        assert!(output_file(&fake.request()).exists(), "the output is on disk");
    }

    #[test]
    fn a_failure_is_classified_from_what_the_process_said() {
        let fake = Fake::new("quota", "echo 'Individual quota reached. Resets in 134h7m2s.' >&2; exit 1");
        let (failure, detail) = turn(&fake).answer.unwrap_err();
        assert_eq!(failure, Failure::Unusable, "a quota that lasts for hours is not worth waiting for: {detail}");

        let fake = Fake::new("limited", "echo '429 Too Many Requests' >&2; exit 1");
        assert_eq!(turn(&fake).answer.unwrap_err().0, Failure::Limited);
    }

    #[test]
    fn a_cli_that_is_not_installed_is_unusable_and_one_that_prints_nothing_is_a_failure() {
        let fake = Fake::new("missing", "true");
        let mut request = fake.request();
        request.agent = Agent::Codex;
        let result = take_turn(&request, &fake.environment(), Duration::from_secs(5), &AtomicBool::new(false));
        assert_eq!(result.answer.unwrap_err().0, Failure::Unusable);
        assert!(turn(&fake).answer.is_err(), "no answer is not an answer");
    }
}
