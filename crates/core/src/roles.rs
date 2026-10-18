//! A role answers: who does this job and how are they told to. Roles are markdown files: built into
//! the app, in the user's folder, or in the project's `.leme-pier/roles/`. A file with the same
//! name overrides the one before it (project over user over built-in).

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::agents::{Agent, Detected, Mode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fallback {
    pub agent: String,
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Builtin,
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Role {
    pub name: String,
    /// The name shown aboard; the interface may translate the built-in ones.
    pub title: String,
    pub mode: Mode,
    /// Empty until the user picks one: the role then goes to the first agent that is available.
    pub agent: String,
    /// Empty means the CLI's own default.
    pub model: String,
    pub fallback: Option<Fallback>,
    /// What the agent is told about its job.
    pub prompt: String,
    pub source: Source,
}

/// Who a role goes to on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub agent: Agent,
    pub model: String,
    pub used_fallback: bool,
}

fn usable(found: &Detected) -> bool {
    found.installed && found.signed_in != Some(false)
}

impl Role {
    /// The agent and model this role goes to: its own, else its fallback, else (for a role no one
    /// has assigned) the first agent that is available. `None` when nobody can do it.
    pub fn assign(&self, available: &[Detected]) -> Option<Assignment> {
        let ready = |name: &str| {
            Agent::from_name(name).filter(|agent| available.iter().any(|f| f.agent == *agent && usable(f)))
        };
        if self.agent.is_empty() {
            let first = available.iter().find(|found| usable(found))?;
            return Some(Assignment { agent: first.agent, model: self.model.clone(), used_fallback: false });
        }
        if let Some(agent) = ready(&self.agent) {
            return Some(Assignment { agent, model: self.model.clone(), used_fallback: false });
        }
        let fallback = self.fallback.as_ref()?;
        ready(&fallback.agent).map(|agent| Assignment { agent, model: fallback.model.clone(), used_fallback: true })
    }
}

fn unquote(value: &str) -> String {
    let value = value.split(" #").next().unwrap_or(value).trim();
    value.trim_matches(|c| c == '"' || c == '\'').to_owned()
}

/// Reads one role file: a front matter between `---` lines, then the prompt.
pub fn parse(text: &str, source: Source) -> Result<Role, String> {
    let rest =
        text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")).ok_or("the file must start with `---`")?;
    let (head, body) = rest.split_once("\n---").ok_or("the front matter is never closed with `---`")?;
    let body = body.split_once('\n').map_or("", |(_, rest)| rest).trim().to_owned();
    let (mut name, mut title, mut mode, mut agent, mut model) =
        (String::new(), String::new(), None, String::new(), String::new());
    let mut fallback: Option<Fallback> = None;
    let mut in_fallback = false;
    for line in head.lines().filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#')) {
        let nested = line.starts_with([' ', '\t']);
        let Some((key, value)) = line.trim().split_once(':') else {
            return Err(format!("`{}` is not `key: value`", line.trim()));
        };
        let value = unquote(value);
        if nested {
            if !in_fallback {
                return Err(format!("`{key}` is indented but belongs to nothing"));
            }
            let target = fallback.get_or_insert(Fallback { agent: String::new(), model: String::new() });
            match key {
                "agent" => target.agent = value,
                "model" => target.model = value,
                other => return Err(format!("`fallback` has no `{other}`")),
            }
            continue;
        }
        in_fallback = key == "fallback";
        match key {
            "name" => name = value,
            "title" => title = value,
            "mode" => {
                mode = Some(match value.as_str() {
                    "read" => Mode::Read,
                    "write" => Mode::Write,
                    other => return Err(format!("`mode` is `read` or `write`, not `{other}`")),
                });
            }
            "agent" => agent = value,
            "model" => model = value,
            "fallback" => {}
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("`name` is required and may only hold letters, digits, `-` and `_`".into());
    }
    if let Some(fallback) = &fallback
        && fallback.agent.is_empty()
    {
        return Err("`fallback` needs an `agent`".into());
    }
    if !agent.is_empty() && Agent::from_name(&agent).is_none() {
        return Err(format!("`{agent}` is not a known agent"));
    }
    if let Some(fallback) = &fallback
        && Agent::from_name(&fallback.agent).is_none()
    {
        return Err(format!("`{}` is not a known agent", fallback.agent));
    }
    Ok(Role {
        title: if title.is_empty() { name.clone() } else { title },
        name,
        mode: mode.ok_or("`mode` is required")?,
        agent,
        model,
        fallback,
        prompt: body,
        source,
    })
}

impl Role {
    /// The role as a file: what [`parse`] reads back.
    pub fn to_markdown(&self) -> String {
        let mode = if self.mode == Mode::Write { "write" } else { "read" };
        let mut text = format!(
            "---\nname: {}\ntitle: {}\nmode: {mode}\nagent: {}\nmodel: \"{}\"\n",
            self.name, self.title, self.agent, self.model
        );
        if let Some(fallback) = &self.fallback {
            let _ = writeln!(text, "fallback:\n  agent: {}\n  model: \"{}\"", fallback.agent, fallback.model);
        }
        let _ = writeln!(text, "---\n{}", self.prompt);
        text
    }
}

/// Writes a role into `folder` as `<name>.md`, replacing the file that was there.
pub fn save(folder: &Path, role: &Role) -> std::io::Result<()> {
    fs::create_dir_all(folder)?;
    fs::write(folder.join(format!("{}.md", role.name)), role.to_markdown())
}

fn builtin(name: &str, title: &str, mode: Mode, prompt: &str) -> Role {
    Role {
        name: name.into(),
        title: title.into(),
        mode,
        agent: String::new(),
        model: String::new(),
        fallback: None,
        prompt: prompt.into(),
        source: Source::Builtin,
    }
}

/// The roles that ship with the app.
pub fn builtins() -> Vec<Role> {
    vec![
        builtin(
            "captain",
            "Navigator",
            Mode::Read,
            "You plan the work. Read the repository as much as you need, then split the brief into pieces that \
             can be done by the other roles. Keep pieces independent where you can, so that they run at the same \
             time, and say which files each one may touch.",
        ),
        builtin(
            "builder",
            "Shipwright",
            Mode::Write,
            "You implement the piece you are given inside this worktree. Change only what the piece needs. When \
             done, reply with a short summary of what you changed.",
        ),
        builtin(
            "reviewer",
            "First Mate",
            Mode::Read,
            "You look over a change another agent made. Read the diff and the code around it. Reply with \
             `approved`, or with the list of changes you want, each one specific.",
        ),
        builtin(
            "tester",
            "Bosun",
            Mode::Read,
            "You run the tests and try to reproduce failures, then report what you found. You never edit files.",
        ),
    ]
}

/// Every role, with the built-in ones overridden by the user's and the project's files, and a line
/// for each file that could not be read.
#[derive(Debug)]
pub struct Roles {
    pub list: Vec<Role>,
    pub problems: Vec<String>,
}

impl Roles {
    pub fn get(&self, name: &str) -> Option<&Role> {
        self.list.iter().find(|role| role.name == name)
    }
}

fn read_folder(folder: &Path, source: Source, roles: &mut Roles) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    for path in files {
        let parsed = fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| parse(&text, source));
        match parsed {
            Ok(role) => match roles.list.iter_mut().find(|existing| existing.name == role.name) {
                Some(existing) => *existing = role,
                None => roles.list.push(role),
            },
            Err(problem) => roles.problems.push(format!("{}: {problem}", path.display())),
        }
    }
}

/// Built-in roles, then the user's folder, then the project's.
pub fn load(user_folder: &Path, project_folder: &Path) -> Roles {
    let mut roles = Roles { list: builtins(), problems: Vec::new() };
    read_folder(user_folder, Source::User, &mut roles);
    read_folder(project_folder, Source::Project, &mut roles);
    roles
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILDER: &str = "---\nname: builder\ntitle: Shipwright   # shown name\nmode: write\nagent: codex\nmodel: \"\"\nfallback:\n  agent: claude\n  model: sonnet\n---\nYou implement the piece.\n";

    fn detected(agent: Agent, installed: bool, signed_in: Option<bool>) -> Detected {
        Detected { agent, installed, version: None, signed_in, account: None, models: Vec::new(), allowance: None }
    }

    #[test]
    fn a_role_file_is_read_with_its_fallback_and_its_prompt() {
        let role = parse(BUILDER, Source::User).unwrap();
        assert_eq!((role.name.as_str(), role.title.as_str(), role.mode), ("builder", "Shipwright", Mode::Write));
        assert_eq!((role.agent.as_str(), role.model.as_str()), ("codex", ""));
        assert_eq!(role.fallback, Some(Fallback { agent: "claude".into(), model: "sonnet".into() }));
        assert_eq!(role.prompt, "You implement the piece.");
    }

    #[test]
    fn a_role_written_to_a_file_reads_back_the_same() {
        let role = parse(BUILDER, Source::User).unwrap();
        assert_eq!(parse(&role.to_markdown(), Source::User).unwrap(), role);
        let plain = Role { fallback: None, agent: String::new(), ..role };
        assert_eq!(parse(&plain.to_markdown(), Source::User).unwrap(), plain);
    }

    #[test]
    fn a_role_file_that_is_wrong_says_what_is_wrong() {
        for (text, wanted) in [
            ("name: x\n", "must start"),
            ("---\nname: x\nmode: read\n", "never closed"),
            ("---\nname: x\nmode: sometimes\n---\n", "`read` or `write`"),
            ("---\nname: x\nmode: read\nagent: gemini\n---\n", "not a known agent"),
            ("---\nmode: read\n---\n", "`name` is required"),
            ("---\nname: x\nmode: read\nflavor: sweet\n---\n", "unknown key"),
            ("---\nname: x\nmode: read\nfallback:\n  model: m\n---\n", "needs an `agent`"),
        ] {
            let error = parse(text, Source::User).unwrap_err();
            assert!(error.contains(wanted), "{text:?} gave {error:?}");
        }
    }

    #[test]
    fn a_role_goes_to_its_agent_then_to_its_fallback_and_an_unassigned_one_to_the_first_available() {
        let role = parse(BUILDER, Source::User).unwrap();
        let both = [detected(Agent::Claude, true, Some(true)), detected(Agent::Codex, true, Some(true))];
        assert_eq!(role.assign(&both).map(|a| (a.agent, a.used_fallback)), Some((Agent::Codex, false)));
        let signed_out = [detected(Agent::Claude, true, Some(true)), detected(Agent::Codex, true, Some(false))];
        let assigned = role.assign(&signed_out).unwrap();
        assert_eq!((assigned.agent, assigned.model.as_str(), assigned.used_fallback), (Agent::Claude, "sonnet", true));
        assert_eq!(role.assign(&[detected(Agent::Codex, false, None)]), None);

        let unassigned = &builtins()[1];
        assert_eq!(unassigned.assign(&signed_out).map(|a| a.agent), Some(Agent::Claude));
        assert_eq!(unassigned.assign(&[]), None);
    }

    #[test]
    fn the_projects_files_override_the_users_which_override_the_built_in_roles() {
        let root = std::env::temp_dir().join(format!("leme-roles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (user, project) = (root.join("user"), root.join("project"));
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(user.join("builder.md"), BUILDER).unwrap();
        fs::write(user.join("broken.md"), "no front matter").unwrap();
        fs::write(project.join("builder.md"), BUILDER.replace("codex", "opencode")).unwrap();
        fs::write(project.join("docs.md"), "---\nname: docs\nmode: write\n---\nWrite docs.\n").unwrap();

        let roles = load(&user, &project);
        let builder = roles.get("builder").unwrap();
        assert_eq!((builder.agent.as_str(), builder.source), ("opencode", Source::Project));
        assert_eq!(roles.get("captain").unwrap().source, Source::Builtin);
        assert!(roles.get("docs").is_some());
        assert_eq!(roles.problems.len(), 1, "{:?}", roles.problems);
        assert!(roles.problems[0].contains("broken.md"));
        let _ = fs::remove_dir_all(&root);
    }
}
