//! A conversation that moves between agent CLIs. Leme Pier keeps its own record of what was said
//! and done, so the agent that takes over reads a short briefing instead of rediscovering the
//! repository, and goes back to its own native session when it speaks again.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

mod briefing;
pub(crate) use briefing::{NOTE_PREFIX, take_notes};
use briefing::{briefing, listed, repository_state};

use crate::agents::{Agent, Environment, Event, Mode, Profile, Request, Session, Transcript, Turn, take_turn};
use crate::memory::Memory;
use crate::runner::Failure;
use crate::store::Layout;
use crate::usage::{self, Log};

const META: &str = "meta.json";
const LOG: &str = "log.jsonl";
/// Longest a single turn may run, and longest it may stay silent.
const TURN_LIMIT: Duration = Duration::from_mins(30);
const STALL_LIMIT: Duration = Duration::from_mins(5);
/// How much of the conversation the briefing carries; the newest entries win.
const LISTED_COMMANDS: usize = 8;
const LISTED_FILES: usize = 20;
const NOTES_PER_TURN: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub repository: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// The id each agent CLI uses for its own copy of this conversation.
    #[serde(default)]
    pub native: BTreeMap<String, String>,
    /// How many entries of the log each agent had seen when it last answered.
    #[serde(default)]
    pub seen: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    User {
        text: String,
        at_ms: u64,
    },
    Agent {
        agent: String,
        model: String,
        text: String,
        edits: Vec<String>,
        commands: Vec<String>,
        tokens: u64,
        duration_ms: u64,
        at_ms: u64,
    },
    Failed {
        agent: String,
        model: String,
        detail: String,
        at_ms: u64,
    },
}

/// Which agent and model answer the next message.
#[derive(Debug, Clone)]
pub struct Choice {
    pub agent: String,
    pub model: String,
}

pub struct Store {
    root: PathBuf,
    /// Where the per-repository memory lives.
    memory: PathBuf,
    usage_file: PathBuf,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['-', '.'])
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

pub(crate) fn cut(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        format!("{}…", flat.chars().take(limit - 1).collect::<String>().trim_end())
    }
}

impl Store {
    pub fn new(layout: &Layout) -> Self {
        Self { root: layout.conversations(), memory: layout.memory(), usage_file: layout.usage_file() }
    }

    fn dir(&self, id: &str) -> Result<PathBuf, String> {
        if plain(id) { Ok(self.root.join(id)) } else { Err("invalid conversation id".into()) }
    }

    pub fn out_dir(&self, id: &str) -> Result<PathBuf, String> {
        Ok(self.dir(id)?.join("out"))
    }

    pub fn create(&self, id: &str, repository: &str) -> Result<Meta, String> {
        let dir = self.dir(id)?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let meta = Meta {
            id: id.to_owned(),
            repository: repository.to_owned(),
            title: "New conversation".into(),
            created_ms: now_ms(),
            updated_ms: now_ms(),
            native: BTreeMap::new(),
            seen: BTreeMap::new(),
        };
        self.save(&meta)?;
        Ok(meta)
    }

    fn save(&self, meta: &Meta) -> Result<(), String> {
        let body = serde_json::to_vec(meta).map_err(|e| e.to_string())?;
        fs::write(self.dir(&meta.id)?.join(META), body).map_err(|e| e.to_string())
    }

    pub fn meta(&self, id: &str) -> Result<Meta, String> {
        let bytes = fs::read(self.dir(id)?.join(META)).map_err(|e| format!("no such conversation: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    /// Every conversation, the most recently used first.
    pub fn list(&self) -> Vec<Meta> {
        let Ok(entries) = fs::read_dir(&self.root) else { return Vec::new() };
        let mut found: Vec<Meta> =
            entries.flatten().filter_map(|entry| self.meta(&entry.file_name().to_string_lossy()).ok()).collect();
        found.sort_by_key(|meta| std::cmp::Reverse(meta.updated_ms));
        found
    }

    /// The whole record, in order. A line cut short by a crash is skipped.
    pub fn log(&self, id: &str) -> Result<Vec<Entry>, String> {
        let text = fs::read_to_string(self.dir(id)?.join(LOG)).unwrap_or_default();
        Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    }

    fn append(&self, id: &str, entry: &Entry) -> Result<(), String> {
        let mut line = serde_json::to_vec(entry).map_err(|e| e.to_string())?;
        line.push(b'\n');
        let mut file =
            OpenOptions::new().create(true).append(true).open(self.dir(id)?.join(LOG)).map_err(|e| e.to_string())?;
        file.write_all(&line).map_err(|e| e.to_string())
    }

    /// Records that a turn could not even be started or written down.
    pub fn fail(&self, id: &str, choice: &Choice, detail: &str) {
        let entry = Entry::Failed {
            agent: choice.agent.clone(),
            model: choice.model.clone(),
            detail: cut(detail, 800),
            at_ms: now_ms(),
        };
        let _ = self.append(id, &entry);
    }

    /// The events the running turn has printed so far, read from its output file.
    pub fn progress(&self, id: &str, label: &str, agent: &str) -> Vec<Event> {
        let Some(kind) = Agent::from_name(agent) else { return Vec::new() };
        let Ok(dir) = self.out_dir(id) else { return Vec::new() };
        let Ok(bytes) = fs::read(dir.join(format!("{label}.out"))) else { return Vec::new() };
        let mut transcript = Transcript::new(kind);
        transcript.feed(&bytes);
        transcript.events().iter().map(|event| event.truncated(2_000)).collect()
    }

    /// Sends `text` to the chosen agent and records the exchange. Returns the label of the turn's
    /// output file through `started` as soon as it is known, so the screen can follow it.
    pub fn take_turn(
        &self,
        id: &str,
        text: &str,
        choice: &Choice,
        environment: &Environment,
        cancel: &AtomicBool,
        started: &dyn Fn(&str),
    ) -> Result<Entry, String> {
        let mut meta = self.meta(id)?;
        let history = self.log(id)?;
        let repository = PathBuf::from(&meta.repository);
        let native = meta.native.get(&choice.agent).cloned();
        let seen =
            if native.is_some() { meta.seen.get(&choice.agent).copied().unwrap_or(0).min(history.len()) } else { 0 };
        let missed = &history[seen..];

        let user = Entry::User { text: text.to_owned(), at_ms: now_ms() };
        self.append(id, &user)?;
        if history.is_empty() {
            meta.title = cut(text, 60);
        }

        let memory = Memory::open(&self.memory, &repository);
        let prompt = if native.is_some() && missed.is_empty() {
            text.to_owned()
        } else {
            format!("{}\n\n{text}", briefing(&choice.agent, missed, &repository_state(&repository), &memory.briefing()))
        };

        let label = format!("{:04}-{}", history.len(), choice.agent);
        started(&label);
        let out_dir = self.out_dir(id)?;
        fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
        let Some(agent) = Agent::from_name(&choice.agent) else {
            return Err(format!("unknown agent `{}`", choice.agent));
        };
        let request = Request {
            agent,
            model: choice.model.clone(),
            effort: None,
            mode: Mode::Write,
            profile: Profile::Full,
            session: native.map_or(Session::New, Session::Resume),
            prompt,
            cwd: repository.clone(),
            out_dir,
            label,
            total: TURN_LIMIT,
        };
        let turn = take_turn(&request, environment, STALL_LIMIT, cancel);

        self.finish(id, meta, choice, history.len(), &memory, turn)
    }

    /// Records what came back: the reply or the failure, the usage, and what the conversation now knows.
    fn finish(
        &self,
        id: &str,
        mut meta: Meta,
        choice: &Choice,
        seen_before: usize,
        memory: &Memory,
        turn: Turn,
    ) -> Result<Entry, String> {
        let at_ms = now_ms();
        let tokens = turn.effective_tokens();
        let outcome = match &turn.answer {
            Ok(_) => usage::Outcome::Delivered,
            Err((Failure::Cancelled, _)) => usage::Outcome::Stopped,
            Err(_) => usage::Outcome::Failed,
        };
        let _ = Log::new(&self.usage_file).record(&usage::Use {
            at_ms,
            agent: choice.agent.clone(),
            model: choice.model.clone(),
            purpose: usage::Purpose::Chat,
            outcome,
            tokens_in: turn.tokens_in,
            tokens_cached: turn.tokens_cached,
            tokens_out: turn.tokens_out,
            duration_ms: u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX),
        });
        let entry = match turn.answer {
            Ok(reply) => {
                let (reply, notes) = take_notes(&reply);
                memory.propose_up_to(&notes, id, NOTES_PER_TURN);
                if let Some(session) = turn.session_id {
                    meta.native.insert(choice.agent.clone(), session);
                }
                meta.seen.insert(choice.agent.clone(), seen_before + 2);
                Entry::Agent {
                    agent: choice.agent.clone(),
                    model: choice.model.clone(),
                    text: reply,
                    edits: listed(
                        &turn.events,
                        |event| match event {
                            Event::Edit { path } => Some(path.clone()),
                            _ => None,
                        },
                        LISTED_FILES,
                    ),
                    commands: listed(
                        &turn.events,
                        |event| match event {
                            Event::Command { line, .. } => Some(line.clone()),
                            _ => None,
                        },
                        LISTED_COMMANDS,
                    ),
                    tokens,
                    duration_ms: u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX),
                    at_ms,
                }
            }
            Err((_, detail)) => Entry::Failed {
                agent: choice.agent.clone(),
                model: choice.model.clone(),
                detail: cut(&detail, 800),
                at_ms,
            },
        };
        self.append(id, &entry)?;
        meta.updated_ms = at_ms;
        self.save(&meta)?;
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("leme-pier-session-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn user(text: &str) -> Entry {
        Entry::User { text: text.into(), at_ms: 1 }
    }

    fn agent(name: &str, text: &str) -> Entry {
        Entry::Agent {
            agent: name.into(),
            model: "m".into(),
            text: text.into(),
            edits: vec!["src/a.rs".into()],
            commands: vec!["cargo test".into()],
            tokens: 10,
            duration_ms: 5,
            at_ms: 2,
        }
    }

    #[test]
    fn a_conversation_is_kept_on_disk_and_read_back_in_order() {
        let store = Store::new(&Layout::new(&temp("disk")));
        store.create("calm-heron-1a2b", "/repo").unwrap();
        store.append("calm-heron-1a2b", &user("one")).unwrap();
        store.append("calm-heron-1a2b", &agent("claude", "two")).unwrap();
        let log = store.log("calm-heron-1a2b").unwrap();
        assert_eq!(log.len(), 2);
        assert!(matches!(&log[1], Entry::Agent { text, .. } if text == "two"));
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn a_conversation_name_cannot_point_outside_the_folder() {
        let store = Store::new(&Layout::new(&temp("names")));
        for bad in ["../x", "a/b", "", "-x", ".hidden"] {
            assert!(store.create(bad, "/repo").is_err(), "{bad}");
            assert!(store.meta(bad).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    /// A fake `claude` that answers from a script, so a turn runs without a real model.
    fn fake_environment(dir: &std::path::Path, script: &str) -> Environment {
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let path = bin.join("claude");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Environment {
            home: dir.to_owned(),
            path_prefix: vec![bin],
            vars: vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_turn_is_recorded_and_the_same_agent_resumes_without_a_briefing() {
        let dir = temp("turn");
        let repository = dir.join("repo");
        fs::create_dir_all(&repository).unwrap();
        // Prints the prompt it received inside its answer, so the test can see what was sent.
        let environment = fake_environment(
            &dir,
            r#"prompt=""; while [ $# -gt 0 ]; do [ "$1" = "-p" ] && prompt="$2"; shift; done
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-1"}'
case "$prompt" in *"Context from Leme Pier"*) seen=briefed;; *) seen=bare;; esac
printf '%s\n' "{\"type\":\"result\",\"session_id\":\"sess-1\",\"is_error\":false,\"result\":\"$seen\\nNOTE: Where | The code is in src.\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}""#,
        );
        let store = Store::new(&Layout::new(&dir));
        let id = "fair-kestrel-0001";
        store.create(id, &repository.to_string_lossy()).unwrap();
        let choice = Choice { agent: "claude".into(), model: "m".into() };
        let cancel = AtomicBool::new(false);

        let first = store.take_turn(id, "hello", &choice, &environment, &cancel, &|_| {}).unwrap();
        assert!(matches!(&first, Entry::Agent { text, tokens: 7, .. } if text == "briefed"), "{first:?}");
        let second = store.take_turn(id, "and again", &choice, &environment, &cancel, &|_| {}).unwrap();
        assert!(matches!(&second, Entry::Agent { text, .. } if text == "bare"), "{second:?}");

        let log = store.log(id).unwrap();
        assert_eq!(log.len(), 4);
        let meta = store.meta(id).unwrap();
        assert_eq!(meta.native.get("claude").map(String::as_str), Some("sess-1"));
        assert_eq!(meta.title, "hello");
        let notes = Memory::open(&Layout::new(&dir).memory(), &repository).notes();
        assert_eq!(notes.len(), 1, "the note the agent left went to the project's memory");
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_that_joins_late_is_briefed_on_what_the_others_did() {
        let dir = temp("switch");
        let repository = dir.join("repo");
        fs::create_dir_all(&repository).unwrap();
        let environment = fake_environment(
            &dir,
            r#"prompt=""; while [ $# -gt 0 ]; do [ "$1" = "-p" ] && prompt="$2"; shift; done
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-2"}'
case "$prompt" in *"The user asked: first request"*) seen=knows-the-history;; *) seen=blank;; esac
printf '%s\n' "{\"type\":\"result\",\"session_id\":\"sess-2\",\"is_error\":false,\"result\":\"$seen\",\"usage\":{}}""#,
        );
        let store = Store::new(&Layout::new(&dir));
        let id = "fair-kestrel-0002";
        store.create(id, &repository.to_string_lossy()).unwrap();
        store.append(id, &user("first request")).unwrap();
        store.append(id, &agent("codex", "did it")).unwrap();
        let choice = Choice { agent: "claude".into(), model: "m".into() };
        let entry = store.take_turn(id, "now you", &choice, &environment, &AtomicBool::new(false), &|_| {}).unwrap();
        assert!(matches!(&entry, Entry::Agent { text, .. } if text == "knows-the-history"), "{entry:?}");
    }
}
