//! Builds the command line that runs one agent CLI for one request. Pure: it turns a [`Request`]
//! into a [`Prepared`] without starting anything, so every flag can be tested. The flags were
//! checked against each CLI's `--help` and a real run.

use std::fmt;

use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

use super::Agent;
use crate::id;
use crate::platform::{self, sandbox};
use crate::runner::{NESTING_GUARD, Spawn, secret_names};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Reads the repository and reports; never changes it.
    Read,
    /// Edits the folder it runs in.
    Write,
}

/// How much of the agent's own power a request keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// No sub-agents and no extra tool servers: cheap and predictable.
    Focused,
    /// The agent as it behaves in a terminal, with its tool servers, skills and sub-agents.
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    New,
    /// Continue a conversation the agent CLI already keeps.
    Resume(String),
}

#[derive(Debug, Clone)]
pub struct Request {
    pub agent: Agent,
    /// Empty means the CLI's own default.
    pub model: String,
    pub effort: Option<String>,
    pub mode: Mode,
    pub profile: Profile,
    pub session: Session,
    pub prompt: String,
    pub cwd: PathBuf,
    /// Where the run's output files go.
    pub out_dir: PathBuf,
    /// File name stem of this run's output, such as `0001-claude`.
    pub label: String,
    /// The total time the run may take; `agy` is told so itself.
    pub total: Duration,
}

/// What the machine looks like to the agent: where home is, and which environment is inherited.
pub struct Environment {
    pub home: PathBuf,
    /// Directories put in front of `PATH` for the spawned process.
    pub path_prefix: Vec<PathBuf>,
    pub vars: Vec<(String, String)>,
}

impl Environment {
    pub fn current() -> Self {
        Self { home: platform::home_dir(), path_prefix: Vec::new(), vars: std::env::vars().collect() }
    }
}

/// A process to start, with what is known about the conversation before it runs.
#[derive(Debug)]
pub struct Prepared {
    pub spawn: Spawn,
    /// The session id when it is known before the run starts (Claude takes one up front).
    pub session_id: Option<String>,
    /// File the agent writes its last message to, for the CLIs that do that.
    pub last_message_file: Option<PathBuf>,
}

/// Why a request cannot be turned into a command on this machine.
#[derive(Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The agent has no sandbox of its own and this machine cannot provide one.
    NeedsSandbox(Agent),
}

impl fmt::Display for Unavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsSandbox(agent) => write!(
                formatter,
                "{} runs commands without asking, so it only runs inside a sandbox, and this machine has none",
                agent.name()
            ),
        }
    }
}

/// The flag that picks the model, or nothing when none was chosen and the CLI uses its own default.
fn model_args(flag: &str, model: &str) -> Vec<String> {
    if model.is_empty() { Vec::new() } else { vec![flag.to_owned(), model.to_owned()] }
}

fn claude(request: &Request, session_id: &mut Option<String>) -> Vec<String> {
    let permission = if request.mode == Mode::Write { "acceptEdits" } else { "plan" };
    let mut args = vec!["-p".into(), request.prompt.clone()];
    args.extend(model_args("--model", &request.model));
    args.extend(["--permission-mode".into(), permission.into()]);
    args.extend(["--output-format".into(), "stream-json".into(), "--verbose".into()]);
    match &request.session {
        Session::New => {
            let id = id::uuid();
            args.extend(["--session-id".into(), id.clone()]);
            *session_id = Some(id);
        }
        Session::Resume(id) => {
            args.extend(["--resume".into(), id.clone()]);
            *session_id = Some(id.clone());
        }
    }
    if request.profile == Profile::Focused {
        // Without these the agent can still reach sub-agents and every connected tool server, and
        // it carries the listing of every installed skill in each request: measured at about 6k
        // tokens, a third of the base cost of a trivial run.
        args.extend([
            "--strict-mcp-config".into(),
            "--disable-slash-commands".into(),
            "--disallowed-tools".into(),
            "Agent".into(),
            "Task".into(),
        ]);
    }
    args
}

fn codex(request: &Request, last_message: &std::path::Path) -> Vec<String> {
    let sandbox = if request.mode == Mode::Write { "workspace-write" } else { "read-only" };
    let mut args = vec!["exec".to_owned()];
    match &request.session {
        Session::New => args.extend(["--skip-git-repo-check".into(), "-s".into(), sandbox.into()]),
        // `exec resume` has no `-s`: the sandbox goes through the config override instead.
        Session::Resume(id) => args.extend([
            "resume".into(),
            "--skip-git-repo-check".into(),
            "-c".into(),
            format!("sandbox_mode=\"{sandbox}\""),
            id.clone(),
        ]),
    }
    args.extend(model_args("-m", &request.model));
    args.extend(["--json".into(), "-o".into(), last_message.display().to_string()]);
    if request.profile == Profile::Focused {
        args.extend(["--disable".into(), "multi_agent".into()]);
    }
    if let Some(effort) = request.effort.as_deref().filter(|e| !e.is_empty()) {
        args.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
    }
    args.push(request.prompt.clone());
    args
}

fn agy(request: &Request) -> Vec<String> {
    let mode = if request.mode == Mode::Write { "accept-edits" } else { "plan" };
    let mut args = vec!["agy".to_owned(), "-p".into(), request.prompt.clone(), "--mode".into(), mode.into()];
    // The sandbox is the barrier, so the agent can run its commands without asking.
    args.push("--dangerously-skip-permissions".into());
    args.extend(model_args("--model", &request.model));
    args.extend(["--output-format".into(), "stream-json".into()]);
    args.extend(["--print-timeout".into(), format!("{}s", request.total.as_secs())]);
    if let Session::Resume(id) = &request.session {
        args.extend(["--conversation".into(), id.clone()]);
    }
    args
}

fn opencode(request: &Request) -> Vec<String> {
    let agent = if request.mode == Mode::Write { "build" } else { "plan" };
    let mut args = vec!["opencode".to_owned(), "run".into(), "--standalone".into(), "--agent".into(), agent.into()];
    args.extend(["--format".into(), "json".into()]);
    args.extend(model_args("-m", &request.model));
    if let Session::Resume(id) = &request.session {
        args.extend(["--session".into(), id.clone()]);
    }
    args.push(request.prompt.clone());
    args
}

/// Agent state that has to stay writable inside the sandbox, relative to home.
fn own_state(agent: Agent) -> &'static [&'static str] {
    match agent {
        Agent::Agy => &[".gemini"],
        Agent::Opencode => &[".local/share/opencode", ".local/state/opencode", ".cache/opencode"],
        Agent::Claude | Agent::Codex => &[],
    }
}

/// Rules and permissions of the agent's own CLI that stay read-only even inside its state folder.
fn protected_state(agent: Agent) -> &'static [&'static str] {
    match agent {
        Agent::Agy => &[".gemini/config", ".gemini/antigravity-cli/settings.json"],
        _ => &[],
    }
}

fn sandboxed(command: Vec<String>, request: &Request, home: &std::path::Path) -> (String, Vec<String>) {
    sandbox::agent(
        command,
        &sandbox::Agent {
            home,
            cwd: &request.cwd,
            writable: request.mode == Mode::Write,
            own_state: own_state(request.agent),
            protected: protected_state(request.agent),
        },
    )
}

/// The process to start for `request`, on a machine that can or cannot sandbox as it is.
pub fn build(request: &Request, environment: &Environment) -> Result<Prepared, Unavailable> {
    build_with(request, environment, sandbox::available())
}

fn build_with(request: &Request, environment: &Environment, can_sandbox: bool) -> Result<Prepared, Unavailable> {
    let last_message = request.out_dir.join(format!("{}.last.txt", request.label));
    let mut session_id = match &request.session {
        Session::Resume(id) => Some(id.clone()),
        Session::New => None,
    };
    let (program, args, last_message_file) = match request.agent {
        Agent::Claude => ("claude".to_owned(), claude(request, &mut session_id), None),
        Agent::Codex => ("codex".to_owned(), codex(request, &last_message), Some(last_message)),
        Agent::Agy | Agent::Opencode => {
            if !can_sandbox {
                return Err(Unavailable::NeedsSandbox(request.agent));
            }
            let command = if request.agent == Agent::Agy { agy(request) } else { opencode(request) };
            let (program, args) = sandboxed(command, request, &environment.home);
            (program, args, None)
        }
    };
    let mut env_set = vec![(NESTING_GUARD.to_owned(), "1".to_owned())];
    if !environment.path_prefix.is_empty() {
        let inherited = environment.vars.iter().find(|(name, _)| name == "PATH").map(|(_, value)| value.clone());
        let mut entries = environment.path_prefix.clone();
        entries.extend(inherited.iter().flat_map(|paths| std::env::split_paths(paths)));
        if let Ok(joined) = std::env::join_paths(entries) {
            env_set.push(("PATH".into(), joined.to_string_lossy().into_owned()));
        }
    }
    Ok(Prepared {
        spawn: Spawn { program, args, cwd: request.cwd.clone(), env_set, env_remove: secret_names(&environment.vars) },
        session_id,
        last_message_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn request(agent: Agent, mode: Mode, profile: Profile, session: Session) -> Request {
        Request {
            agent,
            model: "the-model".into(),
            effort: (agent == Agent::Codex).then(|| "high".to_owned()),
            mode,
            profile,
            session,
            prompt: "THE PROMPT".into(),
            cwd: PathBuf::from("/work/tree"),
            out_dir: PathBuf::from("/out"),
            label: "01-agent".into(),
            total: Duration::from_secs(300),
        }
    }

    fn environment() -> Environment {
        Environment { home: PathBuf::from("/nonexistent-home"), path_prefix: Vec::new(), vars: Vec::new() }
    }

    fn built(agent: Agent, mode: Mode, profile: Profile, session: Session) -> Prepared {
        build_with(&request(agent, mode, profile, session), &environment(), true).unwrap()
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn claude_names_its_session_up_front_and_keeps_workers_focused() {
        let prepared = built(Agent::Claude, Mode::Read, Profile::Focused, Session::New);
        let args = &prepared.spawn.args;
        assert_eq!(prepared.spawn.program, "claude");
        assert!(has_pair(args, "-p", "THE PROMPT") && has_pair(args, "--model", "the-model"));
        assert!(has_pair(args, "--permission-mode", "plan") && has_pair(args, "--output-format", "stream-json"));
        assert!(args.contains(&"--strict-mcp-config".into()) && args.contains(&"--disallowed-tools".into()));
        let id = prepared.session_id.expect("the id is known before the run");
        assert!(has_pair(args, "--session-id", &id));
        assert!(!args.contains(&"--no-session-persistence".into()), "the session must stay resumable");
    }

    #[test]
    fn claude_resumes_by_id_and_a_full_profile_keeps_its_tools() {
        let prepared = built(Agent::Claude, Mode::Write, Profile::Full, Session::Resume("abc".into()));
        let args = &prepared.spawn.args;
        assert!(has_pair(args, "--resume", "abc") && !args.contains(&"--session-id".into()));
        assert!(has_pair(args, "--permission-mode", "acceptEdits"));
        assert!(!args.contains(&"--strict-mcp-config".into()) && !args.contains(&"--disallowed-tools".into()));
        assert_eq!(prepared.session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn codex_gets_its_sandbox_effort_and_last_message_file() {
        let prepared = built(Agent::Codex, Mode::Read, Profile::Focused, Session::New);
        let args = &prepared.spawn.args;
        assert_eq!(&args[..2], ["exec", "--skip-git-repo-check"]);
        assert!(has_pair(args, "-s", "read-only") && has_pair(args, "-m", "the-model"));
        assert!(has_pair(args, "-c", "model_reasoning_effort=\"high\"") && has_pair(args, "--disable", "multi_agent"));
        let last_message = Path::new("/out").join("01-agent.last.txt");
        assert!(has_pair(args, "-o", &last_message.display().to_string()) && args.contains(&"--json".into()));
        assert_eq!(args.last().map(String::as_str), Some("THE PROMPT"));
        assert_eq!(prepared.last_message_file.as_deref(), Some(last_message.as_path()));
        assert!(has_pair(
            &built(Agent::Codex, Mode::Write, Profile::Focused, Session::New).spawn.args,
            "-s",
            "workspace-write"
        ));
    }

    #[test]
    fn a_resumed_codex_session_sets_the_sandbox_through_the_config_because_resume_has_no_flag_for_it() {
        let prepared = built(Agent::Codex, Mode::Read, Profile::Full, Session::Resume("thread-1".into()));
        let args = &prepared.spawn.args;
        assert_eq!(&args[..3], ["exec", "resume", "--skip-git-repo-check"]);
        assert!(has_pair(args, "-c", "sandbox_mode=\"read-only\""));
        assert!(!args.contains(&"-s".into()) && !args.contains(&"--disable".into()));
        let position = |needle: &str| args.iter().position(|a| a == needle).unwrap();
        assert!(position("thread-1") < position("THE PROMPT"), "the id comes before the prompt");
    }

    #[test]
    fn without_a_chosen_model_no_flag_is_sent_and_the_cli_uses_its_own() {
        for agent in [Agent::Claude, Agent::Codex, Agent::Agy, Agent::Opencode] {
            let mut asked = request(agent, Mode::Write, Profile::Full, Session::New);
            asked.model = String::new();
            let prepared = build_with(&asked, &environment(), true).unwrap();
            assert!(
                !prepared.spawn.args.iter().any(|a| matches!(a.as_str(), "--model" | "-m")),
                "{agent:?}: {:?}",
                prepared.spawn.args
            );
        }
    }

    #[test]
    fn agy_and_opencode_run_in_the_sandbox_and_resume_by_their_own_ids() {
        let agy = built(Agent::Agy, Mode::Read, Profile::Focused, Session::Resume("conv-3".into()));
        assert_eq!(agy.spawn.program, "bwrap");
        assert!(has_pair(&agy.spawn.args, "--conversation", "conv-3") && has_pair(&agy.spawn.args, "--mode", "plan"));
        assert!(has_pair(&agy.spawn.args, "--print-timeout", "300s"));

        let writing = built(Agent::Opencode, Mode::Write, Profile::Focused, Session::Resume("ses_9".into()));
        assert!(
            has_pair(&writing.spawn.args, "--agent", "build") && has_pair(&writing.spawn.args, "--session", "ses_9")
        );
        assert!(writing.spawn.args.windows(3).any(|w| w[0] == "--bind" && w[1] == "/work/tree"));
    }

    #[test]
    fn an_agent_that_needs_a_sandbox_is_unavailable_where_there_is_none() {
        for agent in [Agent::Agy, Agent::Opencode] {
            let result =
                build_with(&request(agent, Mode::Write, Profile::Focused, Session::New), &environment(), false);
            assert_eq!(result.unwrap_err(), Unavailable::NeedsSandbox(agent));
        }
        assert!(
            build_with(&request(Agent::Claude, Mode::Write, Profile::Focused, Session::New), &environment(), false)
                .is_ok()
        );
    }

    #[test]
    fn secrets_are_taken_out_of_the_environment_and_the_guard_and_path_are_set() {
        let vars: Vec<(String, String)> =
            [("PATH", "/usr/bin"), ("GITHUB_TOKEN", "x"), ("OPENAI_API_KEY", "x"), ("LANG", "C")]
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .to_vec();
        let environment = Environment { home: PathBuf::from("/h"), path_prefix: vec![PathBuf::from("/fake")], vars };
        let prepared =
            build_with(&request(Agent::Claude, Mode::Read, Profile::Focused, Session::New), &environment, true)
                .unwrap();
        let spawn = prepared.spawn;
        assert!(spawn.env_set.contains(&(NESTING_GUARD.into(), "1".into())));
        let path = spawn.env_set.iter().find(|(name, _)| name == "PATH").unwrap();
        assert!(path.1.starts_with("/fake") && path.1.contains("/usr/bin"));
        assert!(
            spawn.env_remove.contains(&"GITHUB_TOKEN".into()) && spawn.env_remove.contains(&"OPENAI_API_KEY".into())
        );
        assert!(!spawn.env_remove.contains(&"LANG".into()));
    }
}
