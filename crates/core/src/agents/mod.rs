//! Reads the line-delimited JSON an agent CLI prints while it works and turns it into events.
//!
//! Each agent speaks its own dialect (`claude.rs`, `codex.rs`, `agy.rs`, `opencode.rs`). A
//! dialect only translates one JSON line into calls on the shared [`Assembler`]; everything
//! the interface shows (commands paired with their output, merged reply fragments, token totals)
//! is built there, so adding an agent never touches the other dialects.

mod agy;
mod claude;
mod codex;
mod command;
mod detect;
mod opencode;

pub use command::{Environment, Mode, Prepared, Profile, Request, Session, Unavailable, build};
pub use detect::{Allowance, Detected, Model, Window, detect_all, detect_in};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Agy,
    Opencode,
}

impl Agent {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agy" => Some(Self::Agy),
            "opencode" => Some(Self::Opencode),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
            Self::Opencode => "opencode",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Message { text: String },
    Command { line: String, output: String },
    Tool { name: String, summary: String },
    Edit { path: String },
    Error { text: String },
}

impl Event {
    /// Cuts a command's output to `max` characters before it goes to the interface.
    #[must_use]
    pub fn truncated(&self, max: usize) -> Self {
        match self {
            Self::Command { line, output } => {
                let end = output.char_indices().nth(max).map_or(output.len(), |(i, _)| i);
                Self::Command { line: line.clone(), output: output[..end].to_owned() }
            }
            other => other.clone(),
        }
    }
}

/// Collects events as dialects report them. Commands and reply fragments arrive in several
/// pieces, so a key ties the pieces of one command or one reply together.
#[derive(Default)]
struct Assembler {
    events: Vec<Event>,
    index_by_key: HashMap<String, usize>,
    /// Input the model read fresh, which is what costs full price.
    tokens_in: u64,
    /// Input served from the provider's cache, which costs a fraction.
    tokens_cached: u64,
    tokens_out: u64,
    session_id: Option<String>,
    /// The agent's own final answer, when its output carries one apart from the message events.
    final_text: Option<String>,
}

impl Assembler {
    fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    fn message(&mut self, text: String) {
        self.push(Event::Message { text });
    }

    fn error(&mut self, text: String) {
        self.push(Event::Error { text });
    }

    fn edit(&mut self, path: String) {
        self.push(Event::Edit { path });
    }

    /// Remembers the first session id the agent reports.
    fn session(&mut self, id: &str) {
        if !id.is_empty() && self.session_id.is_none() {
            self.session_id = Some(id.to_owned());
        }
    }

    fn final_answer(&mut self, text: String) {
        self.final_text = Some(text);
    }

    fn tokens(&mut self, fresh: u64, cached: u64, output: u64) {
        self.tokens_in = self.tokens_in.saturating_add(fresh);
        self.tokens_cached = self.tokens_cached.saturating_add(cached);
        self.tokens_out = self.tokens_out.saturating_add(output);
    }

    fn has(&self, key: &str) -> bool {
        self.index_by_key.contains_key(key)
    }

    /// Adds an event that later pieces will refer to by `key`.
    fn keyed(&mut self, key: String, event: Event) {
        self.index_by_key.insert(key, self.events.len());
        self.events.push(event);
    }

    /// A command that arrives already finished, with its output.
    fn command(&mut self, line: String, output: String) {
        self.push(Event::Command { line, output });
    }

    fn command_started(&mut self, key: String, line: String) {
        self.keyed(key, Event::Command { line, output: String::new() });
    }

    /// Fills in the output of the command opened under `key`. Returns whether that key was known;
    /// a key that points at something other than a command counts as known and is left alone.
    fn command_finished(&mut self, key: &str, new_output: String) -> bool {
        match self.index_by_key.get(key).map(|&i| &mut self.events[i]) {
            Some(Event::Command { output, .. }) => {
                *output = new_output;
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Appends a fragment to the reply opened under `key`, opening it on the first fragment.
    fn reply_fragment(&mut self, key: String, fragment: String) {
        match self.index_by_key.get(&key).map(|&i| &mut self.events[i]) {
            Some(Event::Message { text }) => text.push_str(&fragment),
            _ => self.keyed(key, Event::Message { text: fragment }),
        }
    }
}

pub struct Transcript {
    agent: Agent,
    pending: Vec<u8>,
    assembler: Assembler,
    unrecognized: usize,
}

impl Transcript {
    pub fn new(agent: Agent) -> Self {
        Self { agent, pending: Vec::new(), assembler: Assembler::default(), unrecognized: 0 }
    }

    /// Takes new output bytes; a line is only processed once it arrives whole.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            self.process(String::from_utf8_lossy(&line).trim());
        }
    }

    fn process(&mut self, line: &str) {
        if !line.starts_with('{') {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            self.unrecognized += 1;
            return;
        };
        let recognized = match self.agent {
            Agent::Claude => claude::handle(&value, &mut self.assembler),
            Agent::Codex => codex::handle(&value, &mut self.assembler),
            Agent::Agy => agy::handle(&value, &mut self.assembler),
            Agent::Opencode => opencode::handle(&value, &mut self.assembler),
        };
        if !recognized {
            self.unrecognized += 1;
        }
    }

    /// The id the agent CLI uses to resume this conversation, once it has reported one.
    pub fn session_id(&self) -> Option<&str> {
        self.assembler.session_id.as_deref()
    }

    /// The agent's final answer: the one its output marks as final, else its last message.
    pub fn final_text(&self) -> Option<&str> {
        self.assembler.final_text.as_deref().or_else(|| {
            self.assembler.events.iter().rev().find_map(|event| match event {
                Event::Message { text } => Some(text.as_str()),
                _ => None,
            })
        })
    }

    pub fn events(&self) -> &[Event] {
        &self.assembler.events
    }

    pub fn tokens_in(&self) -> u64 {
        self.assembler.tokens_in
    }

    pub fn tokens_cached(&self) -> u64 {
        self.assembler.tokens_cached
    }

    pub fn tokens_out(&self) -> u64 {
        self.assembler.tokens_out
    }

    /// Lines that matched no known format: a sign the CLI changed its output.
    #[cfg(test)]
    pub fn unrecognized(&self) -> usize {
        self.unrecognized
    }
}

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().unwrap_or_default()
}

fn text(value: &Value, pointer: &str) -> String {
    value.pointer(pointer).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn number(value: &Value, pointer: &str) -> u64 {
    value.pointer(pointer).and_then(Value::as_u64).unwrap_or_default()
}

/// A one-line description of a tool call's input, for tools the interface has no special view for.
fn summarize(input: &Value) -> String {
    for key in ["file_path", "filePath", "AbsolutePath", "path", "pattern", "url", "query", "command"] {
        if let Some(value) = input[key].as_str() {
            return value.to_owned();
        }
    }
    input.to_string().chars().take(120).collect()
}

#[cfg(test)]
fn transcribe(agent: Agent, lines: &[&str]) -> Transcript {
    let mut transcript = Transcript::new(agent);
    for line in lines {
        transcript.feed(format!("{line}\n").as_bytes());
    }
    transcript
}

#[cfg(test)]
fn command(line: &str, output: &str) -> Event {
    Event::Command { line: line.into(), output: output.into() }
}

#[cfg(test)]
fn message(text: &str) -> Event {
    Event::Message { text: text.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_split_across_reads_is_processed_once_it_is_whole() {
        let mut t = Transcript::new(Agent::Codex);
        t.feed(br#"{"type":"item.completed","item":{"id":"i","type":"agent_mes"#);
        assert_eq!(t.events(), []);
        t.feed(b"sage\",\"text\":\"ok\"}}\n{\"type\":\"new.format\"}\n");
        assert_eq!(t.events(), [message("ok")]);
        assert_eq!(t.unrecognized(), 1);
    }

    #[test]
    fn text_that_is_not_json_is_ignored_and_broken_json_is_counted() {
        let mut t = Transcript::new(Agent::Claude);
        t.feed(b"plain log line\n{broken json\n");
        assert_eq!(t.events(), []);
        assert_eq!(t.unrecognized(), 1);
    }

    #[test]
    fn a_command_finished_under_an_unknown_key_is_reported_as_unknown() {
        let mut assembler = Assembler::default();
        assembler.command_started("a".into(), "ls".into());
        assert!(assembler.command_finished("a", "out".into()));
        assert!(!assembler.command_finished("missing", "out".into()));
        assert_eq!(assembler.events, [command("ls", "out")]);
    }

    #[test]
    fn reply_fragments_with_the_same_key_become_one_message() {
        let mut assembler = Assembler::default();
        assembler.reply_fragment("s1".into(), "List".into());
        assembler.reply_fragment("s1".into(), "ing.".into());
        assembler.reply_fragment("s2".into(), "Next.".into());
        assert_eq!(assembler.events, [message("Listing."), message("Next.")]);
    }

    #[test]
    fn truncation_cuts_only_command_output_on_a_char_boundary() {
        let long = command("echo", "ééééé");
        assert_eq!(long.truncated(2), command("echo", "éé"));
        assert_eq!(message("ééééé").truncated(2), message("ééééé"));
    }
}
