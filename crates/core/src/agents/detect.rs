//! Asks each CLI what it is: whether it is installed, its version, whether it is signed in, which
//! models it offers and, where it says so, how much of its allowance is used. It asks the CLI
//! itself, with no model call, and every answer that cannot be read degrades to "unknown" instead
//! of failing.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::Agent;
use crate::platform;
use crate::runner::{Captured, capture};

const ASK_LIMIT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// The reasoning efforts the CLI offers for it, when it says.
    pub efforts: Vec<String>,
}

impl Model {
    fn plain(id: &str, name: &str) -> Self {
        Self { id: id.to_owned(), name: name.to_owned(), efforts: Vec::new() }
    }
}

/// How much of an allowance is used and when it starts over, as the CLI words it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Window {
    pub percent: u8,
    pub resets: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Allowance {
    pub session: Option<Window>,
    pub week: Option<Window>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detected {
    pub agent: Agent,
    pub installed: bool,
    pub version: Option<String>,
    /// `None` when the CLI gives no way to tell.
    pub signed_in: Option<bool>,
    pub account: Option<String>,
    pub models: Vec<Model>,
    pub allowance: Option<Allowance>,
}

/// The first word that looks like a version (starts with a digit or `v`), else the last word.
fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit() || c == 'v'))
        .map(str::to_owned)
        .or_else(|| output.split_whitespace().last().map(str::to_owned))
}

/// The aliases `claude -p "/model"` says it accepts: "Available: sonnet, opus, ..., or a full model ID."
fn claude_models(output: &str) -> Vec<Model> {
    let Some((_, listed)) = output.split_once("Available:") else { return Vec::new() };
    let listed = listed.lines().next().unwrap_or_default();
    let listed = listed.split(", or ").next().unwrap_or(listed);
    listed
        .split(',')
        .map(|alias| alias.trim().trim_end_matches('.'))
        .filter(|alias| !alias.is_empty() && *alias != "default")
        .map(|alias| Model::plain(alias, alias))
        .collect()
}

/// What `claude -p "/usage"` prints: "Current session: 0% used · resets Oct 18, 8:50pm (...)".
fn claude_allowance(output: &str) -> Option<Allowance> {
    let window = |label: &str| {
        let line = output.lines().find(|line| line.trim_start().starts_with(label))?;
        let percent = line.split_once(':')?.1.split('%').next()?.trim().parse().ok()?;
        let resets = line.split_once("resets").map(|(_, rest)| rest.trim().to_owned()).unwrap_or_default();
        Some(Window { percent, resets })
    };
    let allowance = Allowance { session: window("Current session"), week: window("Current week") };
    (allowance.session.is_some() || allowance.week.is_some()).then_some(allowance)
}

/// `codex debug models`: a catalog in JSON, some entries hidden from the picker.
fn codex_models(output: &str) -> Vec<Model> {
    let Ok(catalog) = serde_json::from_str::<Value>(output) else { return Vec::new() };
    catalog["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| model["visibility"].as_str() == Some("list"))
        .filter_map(|model| {
            let id = model["slug"].as_str()?;
            Some(Model {
                id: id.to_owned(),
                name: model["display_name"].as_str().unwrap_or(id).to_owned(),
                efforts: model["supported_reasoning_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|level| level["effort"].as_str().map(str::to_owned))
                    .collect(),
            })
        })
        .collect()
}

/// `agy models`: an id and a name per line, separated by a tab, after a notice.
fn agy_models(output: &str) -> Vec<Model> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(id, name)| Model::plain(id.trim(), name.trim()))
        .collect()
}

/// `opencode models`: `provider/model` per line.
fn opencode_models(output: &str) -> Vec<Model> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| line.contains('/') && !line.contains(' '))
        .map(|id| Model::plain(id, id))
        .collect()
}

struct Ask {
    program: PathBuf,
}

impl Ask {
    fn run(&self, args: &[&str]) -> Option<Captured> {
        capture(&self.program, args, ASK_LIMIT)
    }

    fn ok(&self, args: &[&str]) -> Option<String> {
        self.run(args).filter(|answer| answer.success).map(|answer| answer.stdout)
    }
}

/// Runs two questions at the same time: each is a process start of a second or two.
fn together<A: Send, B: Send>(first: impl FnOnce() -> A + Send, second: impl FnOnce() -> B + Send) -> (A, B) {
    thread::scope(|scope| {
        let first = scope.spawn(first);
        let second = second();
        (first.join().expect("a question to a CLI does not panic"), second)
    })
}

fn claude(ask: &Ask, mut detected: Detected) -> Detected {
    let (status, (models, usage)) = together(
        || ask.run(&["auth", "status"]).map(|answer| answer.stdout),
        || together(|| ask.ok(&["-p", "/model"]), || ask.ok(&["-p", "/usage"])),
    );
    let status = status.and_then(|text| serde_json::from_str::<Value>(&text).ok());
    detected.signed_in = status.as_ref().and_then(|s| s["loggedIn"].as_bool());
    detected.account = status.as_ref().and_then(|s| s["authMethod"].as_str().map(str::to_owned));
    detected.models = models.as_deref().map(claude_models).unwrap_or_default();
    if detected.models.is_empty() {
        // The CLI changed how it answers: fall back to the aliases it has always accepted.
        detected.models = ["haiku", "sonnet", "opus"].map(|alias| Model::plain(alias, alias)).to_vec();
    }
    detected.allowance = usage.as_deref().and_then(claude_allowance);
    detected
}

fn codex(ask: &Ask, mut detected: Detected) -> Detected {
    let (status, models) = together(|| ask.run(&["login", "status"]), || ask.ok(&["debug", "models"]));
    let status = status.map(|answer| (answer.success, answer.text()));
    detected.signed_in = status.as_ref().map(|(success, text)| *success && text.contains("Logged in"));
    detected.account = status.map(|(_, text)| text.trim().to_owned()).filter(|text| !text.is_empty());
    detected.models = models.as_deref().map(codex_models).unwrap_or_default();
    detected
}

fn agy(ask: &Ask, mut detected: Detected) -> Detected {
    detected.models = ask.ok(&["models"]).as_deref().map(agy_models).unwrap_or_default();
    // It has no login command: a CLI that lists models is signed in.
    detected.signed_in = Some(!detected.models.is_empty());
    detected
}

fn opencode(ask: &Ask, mut detected: Detected) -> Detected {
    let (providers, models) = together(|| ask.ok(&["auth", "list"]), || ask.ok(&["models"]));
    let providers: Vec<String> = providers
        .as_deref()
        .map(|text| text.lines().filter_map(|line| line.split_whitespace().next()).map(str::to_owned).collect())
        .unwrap_or_default();
    detected.models = models.as_deref().map(opencode_models).unwrap_or_default();
    detected.signed_in = Some(!providers.is_empty());
    detected.account = (!providers.is_empty()).then(|| providers.join(", "));
    detected
}

/// What `agent` is on this machine, looking for it in `folders`.
pub fn detect_in(agent: Agent, folders: &[PathBuf]) -> Detected {
    let mut detected = Detected {
        agent,
        installed: false,
        version: None,
        signed_in: None,
        account: None,
        models: Vec::new(),
        allowance: None,
    };
    let Some(program) = platform::find_program(agent.name(), folders) else { return detected };
    let ask = Ask { program };
    let Some(version) = ask.run(&["--version"]).map(|answer| answer.text()).and_then(|text| parse_version(&text))
    else {
        return detected;
    };
    detected.installed = true;
    detected.version = Some(version);
    match agent {
        Agent::Claude => claude(&ask, detected),
        Agent::Codex => codex(&ask, detected),
        Agent::Agy => agy(&ask, detected),
        Agent::Opencode => opencode(&ask, detected),
    }
}

/// All four, asked at the same time, on the `PATH` of this process.
pub fn detect_all() -> Vec<Detected> {
    let folders = platform::path_folders();
    let folders = &folders;
    thread::scope(|scope| {
        let running = [Agent::Claude, Agent::Codex, Agent::Agy, Agent::Opencode]
            .map(|agent| scope.spawn(move || detect_in(agent, folders)));
        running.into_iter().map(|handle| handle.join().expect("detecting an agent does not panic")).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_models_are_the_aliases_its_own_model_command_lists() {
        let output = "Current model: `Sonnet 5.5`\nUsage: /model <name>. Available: sonnet, opus, haiku, fable, best, sonnet[1m], opusplan, default, or a full model ID.\n";
        let ids: Vec<String> = claude_models(output).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["sonnet", "opus", "haiku", "fable", "best", "sonnet[1m]", "opusplan"]);
        assert_eq!(claude_models("something else entirely"), []);
    }

    #[test]
    fn claude_reports_how_much_of_the_session_and_the_week_is_used() {
        let output = "You are currently using your subscription\n\nCurrent session: 0% used · resets Oct 18, 8:50pm (America/Sao_Paulo)\nCurrent week (all models): 66% used · resets Oct 21, 9pm (America/Sao_Paulo)\n\nWhat's contributing\n";
        let allowance = claude_allowance(output).expect("both windows are there");
        assert_eq!(allowance.session, Some(Window { percent: 0, resets: "Oct 18, 8:50pm (America/Sao_Paulo)".into() }));
        assert_eq!(allowance.week.map(|w| w.percent), Some(66));
        assert_eq!(claude_allowance("nothing about usage"), None);
    }

    #[test]
    fn codex_models_come_from_its_catalog_without_the_hidden_ones_and_with_their_efforts() {
        let catalog = r#"{"models":[
            {"slug":"gpt-6.1-sol","display_name":"GPT-6.1-Sol","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]},
            {"slug":"gpt-reserve","display_name":"GPT-Reserve","visibility":"hide","supported_reasoning_levels":[]}]}"#;
        let models = codex_models(catalog);
        assert_eq!(
            models,
            [Model {
                id: "gpt-6.1-sol".into(),
                name: "GPT-6.1-Sol".into(),
                efforts: vec!["low".into(), "high".into()]
            }]
        );
        assert_eq!(codex_models("not json"), []);
    }

    #[test]
    fn agy_and_opencode_list_one_model_per_line_and_skip_notices() {
        let agy = "Fetching available models...\ngemini-4-argon-high\tGemini 4 Argon (High)\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\n";
        let models = agy_models(agy);
        assert_eq!((models.len(), models[1].name.as_str()), (2, "Claude Opus 5.5 (Low)"));
        let opencode = "Loading...\ngoogle/gemini-2.5-computer-use-preview-10-2025\nnvidia/llama-4\n";
        assert_eq!(opencode_models(opencode).len(), 2);
    }

    #[test]
    fn the_version_is_the_first_word_that_looks_like_one() {
        assert_eq!(parse_version("2.1.302 (Claude Code)\n").as_deref(), Some("2.1.302"));
        assert_eq!(parse_version("codex-cli 0.163.0").as_deref(), Some("0.163.0"));
        assert_eq!(parse_version("opencode v2.0.31").as_deref(), Some("v2.0.31"));
        assert_eq!(parse_version("  \n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_is_asked_what_it_is_and_one_that_is_missing_is_reported_as_not_installed() {
        let dir = std::env::temp_dir().join(format!("clipier-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = r#"case "$1" in
  --version) echo "codex-cli 9.9.9";;
  login) echo "Logged in using ChatGPT" >&2;;
  debug) echo '{"models":[{"slug":"m1","display_name":"M1","visibility":"list","supported_reasoning_levels":[]}]}';;
esac"#;
        let path = dir.join("codex");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        let found = detect_in(Agent::Codex, std::slice::from_ref(&dir));
        assert!(found.installed && found.version.as_deref() == Some("9.9.9"));
        assert_eq!(found.signed_in, Some(true));
        assert_eq!(found.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["m1"]);
        let missing = detect_in(Agent::Claude, std::slice::from_ref(&dir));
        assert!(!missing.installed && missing.models.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Asks the real CLIs of this machine; run by hand with `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore = "asks the agent CLIs installed on this machine"]
    fn detects_the_agents_of_this_machine() {
        for found in detect_all() {
            println!(
                "{:?}: installed={} version={:?} signed_in={:?} account={:?} models={} allowance={:?}",
                found.agent,
                found.installed,
                found.version,
                found.signed_in,
                found.account,
                found.models.len(),
                found.allowance
            );
        }
    }
}
