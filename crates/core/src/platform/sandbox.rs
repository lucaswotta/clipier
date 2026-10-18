//! How to run a command inside bubblewrap: the system read-only, credentials hidden and the
//! working directory writable only when asked. Building the arguments is pure and works anywhere;
//! only Linux has bubblewrap, and [`available`] says whether this machine does.

use std::path::{Path, PathBuf};

use super::{find_program, path_folders};

/// Credentials and state of other tools: empty inside the sandbox.
const SECRET_PATHS: [&str; 17] = [
    ".ssh",
    ".gnupg",
    ".codex",
    ".claude",
    ".claude.json",
    ".config/gh",
    ".local/share/keyrings",
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".anydesk",
    ".gemini",
    ".local/share/opencode",
    ".config/opencode",
    ".aws",
    ".kube",
    ".netrc",
];

const BASE: [&str; 9] = ["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"];

/// What a sandboxed agent may touch.
pub struct Agent<'a> {
    pub home: &'a Path,
    pub cwd: &'a Path,
    /// Whether the agent may write in `cwd`.
    pub writable: bool,
    /// Its own state, relative to home, that has to stay writable.
    pub own_state: &'a [&'a str],
    /// Files of its own CLI that stay read-only even inside its state.
    pub protected: &'a [&'a str],
}

/// Whether this machine can sandbox: only Linux, and only with `bwrap` installed.
pub fn available() -> bool {
    cfg!(target_os = "linux") && find_program("bwrap", &path_folders()).is_some()
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

/// Hides the files that hold credentials, except the state the sandboxed program owns.
fn hide_secrets(args: &mut Vec<String>, home: &Path, own: &[PathBuf]) {
    for relative in SECRET_PATHS {
        let path = home.join(relative);
        let is_own = own.iter().any(|state| path == *state || state.starts_with(&path));
        if is_own || !path.exists() {
            continue;
        }
        if path.is_dir() {
            args.extend(["--tmpfs".into(), text(&path)]);
        } else {
            args.extend(["--ro-bind".into(), "/dev/null".into(), text(&path)]);
        }
    }
}

/// `bwrap` and its arguments to run `command` as an agent.
pub fn agent(command: Vec<String>, agent: &Agent<'_>) -> (String, Vec<String>) {
    let own: Vec<PathBuf> = agent.own_state.iter().map(|p| agent.home.join(p)).collect();
    let mut args: Vec<String> = BASE.map(String::from).to_vec();
    hide_secrets(&mut args, agent.home, &own);
    for path in &own {
        args.extend(["--bind".into(), text(path), text(path)]);
    }
    for relative in agent.protected {
        let path = agent.home.join(relative);
        if path.exists() {
            args.extend(["--ro-bind".into(), text(&path), text(&path)]);
        }
    }
    let bind = if agent.writable { "--bind" } else { "--ro-bind" };
    let cwd = text(agent.cwd);
    args.extend([
        bind.into(),
        cwd.clone(),
        cwd.clone(),
        "--chdir".into(),
        cwd,
        "--die-with-parent".into(),
        "--".into(),
    ]);
    args.extend(command);
    ("bwrap".into(), args)
}

/// `bwrap` and its arguments to run a project's own check on what an agent wrote: only the
/// worktree writable and no network.
pub fn check(command: &[String], cwd: &Path, home: &Path) -> (String, Vec<String>) {
    let mut flags: Vec<String> = BASE.map(String::from).to_vec();
    flags.push("--unshare-net".into());
    hide_secrets(&mut flags, home, &[]);
    let dir = text(cwd);
    flags.extend([
        "--bind".into(),
        dir.clone(),
        dir.clone(),
        "--chdir".into(),
        dir,
        "--die-with-parent".into(),
        "--".into(),
    ]);
    flags.extend(command.iter().cloned());
    ("bwrap".into(), flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("clipier-home-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for dir in [".ssh", ".gemini/config", ".gemini/antigravity-cli", ".config/gh"] {
            std::fs::create_dir_all(home.join(dir)).unwrap();
        }
        std::fs::write(home.join(".gemini/antigravity-cli/settings.json"), "{}").unwrap();
        std::fs::write(home.join(".netrc"), "machine x").unwrap();
        home
    }

    /// A path under home as the sandbox writes it, whatever the platform's separator.
    fn inside(home: &Path, relative: &str) -> String {
        home.join(relative).display().to_string()
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    fn has_bind(args: &[String], flag: &str, source: &str, target: &str) -> bool {
        args.windows(3).any(|w| w[0] == flag && w[1] == source && w[2] == target)
    }

    #[test]
    fn a_check_runs_without_network_with_secrets_hidden_and_only_the_worktree_writable() {
        let home = home("check");
        let command = vec!["python3".to_owned(), "-m".to_owned(), "unittest".to_owned()];
        let (program, args) = check(&command, Path::new("/work/tree"), &home);
        assert_eq!(program, "bwrap");
        assert!(args.contains(&"--unshare-net".to_owned()));
        assert!(has_pair(&args, "--tmpfs", &inside(&home, ".ssh")));
        assert!(has_bind(&args, "--bind", "/work/tree", "/work/tree"));
        assert_eq!(args.iter().filter(|a| a.as_str() == "--bind").count(), 1, "nothing else is writable");
        assert!(args.ends_with(&command));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_agent_keeps_its_own_state_writable_and_its_permissions_read_only_while_other_secrets_are_hidden() {
        let home = home("agent");
        let sandbox = Agent {
            home: &home,
            cwd: Path::new("/work/tree"),
            writable: false,
            own_state: &[".gemini"],
            protected: &[".gemini/antigravity-cli/settings.json"],
        };
        let (program, args) = agent(vec!["agy".into(), "-p".into()], &sandbox);
        assert_eq!(program, "bwrap");
        assert!(
            has_pair(&args, "--tmpfs", &inside(&home, ".ssh"))
                && has_pair(&args, "--tmpfs", &inside(&home, ".config/gh"))
        );
        assert!(has_bind(&args, "--ro-bind", "/dev/null", &inside(&home, ".netrc")));
        let own = inside(&home, ".gemini");
        assert!(has_bind(&args, "--bind", &own, &own), "its own state stays writable");
        assert!(!has_pair(&args, "--tmpfs", &own), "it must not hide its own state");
        let settings = inside(&home, ".gemini/antigravity-cli/settings.json");
        assert!(has_bind(&args, "--ro-bind", &settings, &settings), "its permissions stay read-only");
        assert!(has_bind(&args, "--ro-bind", "/work/tree", "/work/tree"), "a read-only agent cannot write the tree");
        let command_at = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&args[command_at + 1..], ["agy", "-p"]);

        let writing = Agent { writable: true, ..sandbox };
        let (_, args) = agent(vec!["agy".into()], &writing);
        assert!(has_bind(&args, "--bind", "/work/tree", "/work/tree"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
