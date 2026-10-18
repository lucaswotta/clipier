//! The project's own checks (its tests), run by Clipier on what an agent wrote.
//!
//! A check costs no tokens and settles what a reviewer can only guess at: a read-only reviewer
//! cannot run the tests, and a builder's own report that they passed is a claim. Failing a check
//! sends the work back before a review is paid for.

use std::fmt::Write as _;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::platform::{self, sandbox};
use crate::runner::secret_names;

/// The longest tail of a check's output that is kept.
const OUTPUT_TAIL: usize = 4_000;
const POLL: Duration = Duration::from_millis(50);

/// A command to run in the root of a worktree: the program and its arguments, never a shell line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub argv: Vec<String>,
    /// Variables set for the command, written before it as `NAME=value`.
    pub env: Vec<(String, String)>,
}

impl Check {
    /// Reads a command line the user typed, honoring quotes. Pipes and redirections are not
    /// interpreted: write `sh -c "..."` for those.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut argv = Vec::new();
        let mut current = String::new();
        let mut started = false;
        let mut quote: Option<char> = None;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some('"'), '\\') => current.push(chars.next().ok_or("a backslash ends the command")?),
                (Some(_), c) => current.push(c),
                (None, '\'' | '"') => {
                    quote = Some(c);
                    started = true;
                }
                (None, '\\') => {
                    current.push(chars.next().ok_or("a backslash ends the command")?);
                    started = true;
                }
                (None, c) if c.is_whitespace() => {
                    if started {
                        argv.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                (None, c) => {
                    current.push(c);
                    started = true;
                }
            }
        }
        if quote.is_some() {
            return Err("a quote is never closed".into());
        }
        if started {
            argv.push(current);
        }
        let mut env = Vec::new();
        while let Some((name, value)) =
            argv.first().and_then(|w| w.split_once('=')).filter(|(name, _)| is_variable(name))
        {
            env.push((name.to_owned(), value.to_owned()));
            argv.remove(0);
        }
        if argv.is_empty() {
            return Err("the check is empty".into());
        }
        Ok(Self { argv, env })
    }

    pub fn display(&self) -> String {
        self.env
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .chain(
                self.argv
                    .iter()
                    .map(|word| if word.contains(char::is_whitespace) { format!("\"{word}\"") } else { word.clone() }),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn is_variable(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// The check a repository most likely has, by what it contains. `None` when nothing says so.
pub fn detect(repository: &Path) -> Option<Check> {
    let has = |name: &str| repository.join(name).exists();
    let check = |words: &[&str]| Some(Check { argv: words.iter().map(|w| (*w).to_owned()).collect(), env: Vec::new() });
    if has("Cargo.toml") {
        return check(&["cargo", "test"]);
    }
    if let Ok(manifest) = std::fs::read_to_string(repository.join("package.json"))
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&manifest)
        && let Some(script) = json["scripts"]["test"].as_str()
        && !script.contains("no test specified")
    {
        if has("pnpm-lock.yaml") {
            return check(&["pnpm", "test"]);
        }
        if has("yarn.lock") {
            return check(&["yarn", "test"]);
        }
        return check(&["npm", "test", "--silent"]);
    }
    if has("go.mod") {
        return check(&["go", "test", "./..."]);
    }
    if has("pytest.ini")
        || has("tox.ini")
        || std::fs::read_to_string(repository.join("pyproject.toml")).is_ok_and(|t| t.contains("[tool.pytest"))
    {
        return check(&["python3", "-m", "pytest", "-q"]);
    }
    let python_tests = std::fs::read_dir(repository).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.starts_with("test_") && Path::new(&name).extension().is_some_and(|e| e == "py"))
                || (name == "tests" && entry.path().is_dir())
        })
    });
    if python_tests {
        return check(&["python3", "-m", "unittest"]);
    }
    if !has("package.json")
        && !has("pyproject.toml")
        && std::fs::read_to_string(repository.join("Makefile"))
            .is_ok_and(|text| text.lines().any(|line| line.starts_with("test:")))
    {
        return check(&["make", "test"]);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub command: String,
    /// `None` when the process was stopped before it finished.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The last part of what it printed.
    pub output: String,
}

impl CheckResult {
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// One line for a report or an event.
    pub fn headline(&self) -> String {
        if self.timed_out {
            format!("`{}` did not finish in time", self.command)
        } else {
            match self.exit_code {
                Some(0) => format!("`{}` passed", self.command),
                Some(code) => format!("`{}` failed with exit code {code}", self.command),
                None => format!("`{}` was stopped", self.command),
            }
        }
    }
}

pub enum Sandbox {
    /// `bwrap`: no network, credentials hidden, only the worktree writable.
    Bubblewrap { home: std::path::PathBuf },
    /// Straight on the machine. Only for tests of this module.
    None,
}

fn keep_tail(buffer: &Mutex<Vec<u8>>, bytes: &[u8]) {
    if let Ok(mut buffer) = buffer.lock() {
        buffer.extend_from_slice(bytes);
        if buffer.len() > OUTPUT_TAIL * 2 {
            let cut = buffer.len() - OUTPUT_TAIL;
            buffer.drain(..cut);
        }
    }
}

fn pump(mut stream: impl Read + Send + 'static, buffer: Arc<Mutex<Vec<u8>>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0u8; 8 * 1024];
        while let Ok(n) = stream.read(&mut chunk) {
            if n == 0 {
                break;
            }
            keep_tail(&buffer, &chunk[..n]);
        }
    })
}

/// Runs `check` in `dir` and waits for it, up to `timeout`.
pub fn run(
    check: &Check,
    dir: &Path,
    sandbox: &Sandbox,
    timeout: Duration,
    cancel: &AtomicBool,
) -> io::Result<CheckResult> {
    let (program, args) = match sandbox {
        Sandbox::Bubblewrap { home } => sandbox::check(&check.argv, dir, home),
        Sandbox::None => (check.argv[0].clone(), check.argv[1..].to_vec()),
    };
    let variables: Vec<(String, String)> = std::env::vars().collect();
    let mut command = Command::new(&program);
    command
        .args(&args)
        .envs(check.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if matches!(sandbox, Sandbox::None) {
        command.current_dir(dir);
    }
    platform::lead_group(&mut command);
    for name in secret_names(&variables) {
        command.env_remove(name);
    }
    let mut child = platform::start(&mut command).map_err(|e| io::Error::new(e.kind(), format!("{program}: {e}")))?;
    let output = Arc::new(Mutex::new(Vec::new()));
    let pumps = [
        pump(child.stdout.take().expect("stdout is piped"), Arc::clone(&output)),
        pump(child.stderr.take().expect("stderr is piped"), Arc::clone(&output)),
    ];

    let started = Instant::now();
    let (mut timed_out, mut stopped) = (false, false);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if cancel.load(Ordering::Acquire) {
            stopped = true;
        } else if started.elapsed() > timeout {
            timed_out = true;
        }
        if stopped || timed_out {
            platform::kill_tree(&mut child);
            break None;
        }
        thread::sleep(POLL);
    };
    for handle in pumps {
        let _ = handle.join();
    }
    let bytes = output.lock().map(|b| b.clone()).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let tail: String = text.chars().rev().take(OUTPUT_TAIL).collect::<Vec<_>>().into_iter().rev().collect();
    let mut shown = String::new();
    let _ = write!(shown, "{}", tail.trim());
    Ok(CheckResult { command: check.display(), exit_code: status.and_then(|s| s.code()), timed_out, output: shown })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clipier-checks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn argv(words: &[&str]) -> Check {
        Check { argv: words.iter().map(|w| (*w).to_owned()).collect(), env: Vec::new() }
    }

    #[cfg(unix)]
    fn quick(check: &Check, dir: &Path) -> CheckResult {
        run(check, dir, &Sandbox::None, Duration::from_secs(10), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn a_typed_command_is_split_on_spaces_and_honors_quotes() {
        assert_eq!(Check::parse("python3 -m unittest").unwrap(), argv(&["python3", "-m", "unittest"]));
        assert_eq!(Check::parse("  sh -c \"echo a b\"  ").unwrap(), argv(&["sh", "-c", "echo a b"]));
        assert_eq!(Check::parse("echo 'it works' \"x\\\"y\"").unwrap(), argv(&["echo", "it works", "x\"y"]));
        assert_eq!(Check::parse("echo ''").unwrap(), argv(&["echo", ""]));
        assert!(Check::parse("   ").is_err());
        assert!(Check::parse("echo \"open").is_err());
    }

    #[test]
    fn variables_written_before_the_command_are_set_for_it() {
        let check = Check::parse("CARGO_TARGET_DIR=/tmp/shared RUST_LOG=debug cargo test").unwrap();
        assert_eq!(check.argv, ["cargo", "test"]);
        assert_eq!(
            check.env,
            [("CARGO_TARGET_DIR".to_owned(), "/tmp/shared".to_owned()), ("RUST_LOG".to_owned(), "debug".to_owned())]
        );
        assert_eq!(check.display(), "CARGO_TARGET_DIR=/tmp/shared RUST_LOG=debug cargo test");
        assert!(Check::parse("A=1").is_err(), "variables alone are not a command");
        assert_eq!(Check::parse("echo A=1").unwrap().env, Vec::new(), "only leading words are variables");
        assert_eq!(Check::parse("a=1 echo").unwrap().argv, ["a=1", "echo"], "lowercase is a program name");
    }

    #[cfg(unix)]
    #[test]
    fn the_variables_reach_the_process() {
        let dir = temp("vars");
        let check = Check::parse("MY_CHECK_VALUE=found sh -c 'echo [$MY_CHECK_VALUE]'").unwrap();
        assert_eq!(quick(&check, &dir).output, "[found]");
    }

    #[test]
    fn a_check_reads_back_the_way_it_was_typed() {
        assert_eq!(argv(&["sh", "-c", "echo a b"]).display(), "sh -c \"echo a b\"");
    }

    #[test]
    fn the_check_is_detected_from_what_the_repository_contains() {
        let python = temp("python");
        fs::write(python.join("test_x.py"), "").unwrap();
        assert_eq!(detect(&python), Some(argv(&["python3", "-m", "unittest"])));

        let cargo = temp("cargo");
        fs::write(cargo.join("Cargo.toml"), "").unwrap();
        assert_eq!(detect(&cargo), Some(argv(&["cargo", "test"])));

        let node = temp("node");
        fs::write(node.join("package.json"), r#"{"scripts":{"test":"vitest run"}}"#).unwrap();
        assert_eq!(detect(&node), Some(argv(&["npm", "test", "--silent"])));

        let placeholder = temp("placeholder");
        fs::write(
            placeholder.join("package.json"),
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        assert_eq!(detect(&placeholder), None, "npm's placeholder is not a test suite");

        assert_eq!(detect(&temp("empty")), None);
    }

    #[test]
    fn node_checks_use_the_lockfiles_package_manager() {
        let node = temp("node-lockfiles");
        fs::write(node.join("package.json"), r#"{"scripts":{"test":"vitest run"}}"#).unwrap();
        fs::write(node.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect(&node), Some(argv(&["pnpm", "test"])));
        fs::remove_file(node.join("pnpm-lock.yaml")).unwrap();
        fs::write(node.join("yarn.lock"), "").unwrap();
        assert_eq!(detect(&node), Some(argv(&["yarn", "test"])));
        fs::write(node.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect(&node), Some(argv(&["pnpm", "test"])));

        for lockfile in ["pnpm-lock.yaml", "yarn.lock"] {
            fs::write(node.join("package.json"), r#"{"scripts":{"test":"echo no test specified"}}"#).unwrap();
            assert_eq!(detect(&node), None);
            fs::write(node.join("package.json"), r#"{"scripts":{}}"#).unwrap();
            assert_eq!(detect(&node), None);
            fs::remove_file(node.join(lockfile)).unwrap();
        }
    }

    #[test]
    fn make_test_is_a_fallback_for_repositories_without_a_recognised_project_file() {
        let make = temp("make");
        for text in ["all:\n\techo test:\n", "# test:\n", "testing:\n", ".PHONY: test\n"] {
            fs::write(make.join("Makefile"), text).unwrap();
            assert_eq!(detect(&make), None);
        }
        fs::write(make.join("Makefile"), ".PHONY: test\ntest:\n\techo ok\n").unwrap();
        assert_eq!(detect(&make), Some(argv(&["make", "test"])));
        for (manifest, expected) in [
            ("Cargo.toml", Some(argv(&["cargo", "test"]))),
            ("package.json", None),
            ("go.mod", Some(argv(&["go", "test", "./..."]))),
            ("pyproject.toml", None),
            ("pytest.ini", Some(argv(&["python3", "-m", "pytest", "-q"]))),
            ("tox.ini", Some(argv(&["python3", "-m", "pytest", "-q"]))),
        ] {
            fs::write(make.join(manifest), "").unwrap();
            assert_eq!(detect(&make), expected, "{manifest} takes precedence over Makefile");
            fs::remove_file(make.join(manifest)).unwrap();
        }
        fs::write(make.join("test_x.py"), "").unwrap();
        assert_eq!(detect(&make), Some(argv(&["python3", "-m", "unittest"])));
    }

    #[cfg(unix)]
    #[test]
    fn a_passing_check_reports_its_output() {
        let result = quick(&argv(&["sh", "-c", "echo fine"]), &temp("pass"));
        assert!(result.passed() && result.output == "fine", "{result:?}");
        assert_eq!(result.headline(), "`sh -c \"echo fine\"` passed");
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_check_reports_the_exit_code_and_the_end_of_its_output() {
        let result = quick(&argv(&["sh", "-c", "echo broken >&2; exit 3"]), &temp("fail"));
        assert_eq!((result.exit_code, result.output.as_str()), (Some(3), "broken"));
        assert!(result.headline().contains("exit code 3"));
    }

    #[cfg(unix)]
    #[test]
    fn only_the_tail_of_a_long_output_is_kept() {
        let result = quick(&argv(&["sh", "-c", "seq 1 20000"]), &temp("long"));
        assert!(result.output.len() <= OUTPUT_TAIL && result.output.ends_with("20000"), "{}", result.output.len());
    }

    #[cfg(unix)]
    #[test]
    fn a_check_that_never_ends_is_stopped_at_the_limit() {
        let started = Instant::now();
        let result = run(
            &argv(&["sleep", "30"]),
            &temp("slow"),
            &Sandbox::None,
            Duration::from_millis(300),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(result.timed_out && !result.passed() && started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_stops_a_running_check() {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Release);
        });
        let result =
            run(&argv(&["sleep", "30"]), &temp("cancel"), &Sandbox::None, Duration::from_secs(30), &cancel).unwrap();
        stopper.join().unwrap();
        assert_eq!(result.exit_code, None);
        assert!(result.headline().contains("stopped"));
    }

    #[cfg(unix)]
    #[test]
    fn credentials_in_the_environment_never_reach_a_check() {
        // SAFETY: no other test of this module reads or writes this variable.
        unsafe { std::env::set_var("CLIPIER_TEST_API_KEY", "secret-value") };
        let result = quick(&argv(&["sh", "-c", "echo [$CLIPIER_TEST_API_KEY]"]), &temp("env"));
        assert_eq!(result.output, "[]");
    }

    #[cfg(unix)]
    #[test]
    fn inside_the_sandbox_the_worktree_is_writable_the_rest_is_not_and_there_is_no_network() {
        // Some machines (CI runners among them) forbid the user namespaces `bwrap` needs.
        if !Command::new("bwrap").args(["--ro-bind", "/", "/", "true"]).status().is_ok_and(|s| s.success()) {
            return;
        }
        let dir = temp("sandbox");
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let sandbox = Sandbox::Bubblewrap { home };
        let script = format!(
            "touch {d}/inside && echo inside-ok; touch /usr/outside 2>/dev/null && echo outside-bad; python3 -c \"import socket,sys; s=socket.socket(); s.settimeout(2); s.connect(('1.1.1.1',53)); print('net-bad')\" 2>/dev/null; echo end",
            d = dir.display()
        );
        let result =
            run(&argv(&["sh", "-c", &script]), &dir, &sandbox, Duration::from_secs(20), &AtomicBool::new(false))
                .unwrap();
        assert!(result.output.contains("inside-ok") && result.output.contains("end"), "{result:?}");
        assert!(!result.output.contains("outside-bad") && !result.output.contains("net-bad"), "{result:?}");
        assert!(dir.join("inside").exists());
    }
}
