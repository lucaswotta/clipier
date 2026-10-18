//! A check of the real window: with `LEME_PIER_SELFTEST=<file>` set, the window opens, a script drives
//! the interface against the real backend, the result goes to the file and the process exits. It
//! catches what a mocked page cannot: the webview, the permissions and the real commands.
//!
//! ```text
//! (cd app && npm run build) && LEME_PIER_SELFTEST=/tmp/report.json cargo run -p leme-pier-app --features custom-protocol
//! ```
//!
//! With `LEME_PIER_SELFTEST_SAIL` also set it asks a real agent one short question in the chat and
//! sails a one-boat race, which spends a little of that agent's quota; with `LEME_PIER_SELFTEST_CREW`
//! as well it also has a captain draw a plan and a crew carry it out.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tauri::{AppHandle, Webview};

const SAIL_REQUEST: &str =
    "Create a file named hello.txt in the root of the repository whose only content is the word leme.";
const CREW_REQUEST: &str =
    "Create a file named crew.txt in the root of the repository whose only content is the word crew.";
const IDLE_REQUEST: &str = "Self test request: nothing will run.";

static SAMPLE: OnceLock<PathBuf> = OnceLock::new();

pub fn requested() -> Option<PathBuf> {
    std::env::var_os("LEME_PIER_SELFTEST").map(PathBuf::from)
}

fn sailing() -> bool {
    std::env::var_os("LEME_PIER_SELFTEST_SAIL").is_some()
}

fn crewing() -> bool {
    sailing() && std::env::var_os("LEME_PIER_SELFTEST_CREW").is_some()
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// A repository with one commit, for the race to work in.
fn sample_repository() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("leme-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("README.md"), "# Sample\n");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "Self test"],
        &["config", "user.email", "t@example.com"],
        &["add", "-A"],
        &["commit", "-q", "-m", "chore: start"],
    ] {
        git(&dir, args);
    }
    dir
}

/// The script that drives the interface, with the repository it may name in the forms.
pub fn script(repository: &str, sail: bool) -> String {
    let quote = |text: &str| serde_json::Value::String(text.to_owned());
    format!(
        "window.__SELFTEST_REPOSITORY__ = {};\nwindow.__SELFTEST_REQUEST__ = {};\nwindow.__SELFTEST_CREW_REQUEST__ = {};\nwindow.__SELFTEST_SAIL__ = {sail};\nwindow.__SELFTEST_CREW__ = {};\n{}",
        quote(repository),
        quote(if sail { SAIL_REQUEST } else { IDLE_REQUEST }),
        quote(CREW_REQUEST),
        crewing(),
        include_str!("../selftest.js")
    )
}

pub fn start(webview: &Webview) {
    let repository = if sailing() { sample_repository() } else { std::env::temp_dir() };
    SAMPLE.set(repository.clone()).ok();
    let _ = webview.eval(script(&repository.to_string_lossy(), sailing()));
}

/// What the repository holds after the race: the file the agent was asked for, on a boat's branch,
/// with the checkout untouched.
fn race_left(repository: &Path) -> Result<(), String> {
    let branch = git(repository, &["branch", "--list", "leme/*", "--format=%(refname:short)"])
        .and_then(|branches| branches.lines().next().map(str::to_owned))
        .ok_or("no boat branch was left in the repository")?;
    let content = git(repository, &["show", &format!("{branch}:hello.txt")]).ok_or("hello.txt is not on the branch")?;
    if content != "leme" {
        return Err(format!("hello.txt holds {content:?}"));
    }
    if crewing() {
        let integration = git(repository, &["branch", "--list", "leme/*/integration", "--format=%(refname:short)"])
            .and_then(|branches| branches.lines().next().map(str::to_owned))
            .ok_or("no integration branch was left by the crew")?;
        let file = git(repository, &["show", &format!("{integration}:crew.txt")])
            .ok_or("crew.txt is not on the integration branch")?;
        if file != "crew" {
            return Err(format!("crew.txt holds {file:?}"));
        }
    }
    if git(repository, &["status", "--porcelain"]).is_some_and(|status| !status.is_empty()) {
        return Err("the checkout was touched".into());
    }
    Ok(())
}

/// Receives the script's verdict, writes it where the caller asked and ends the process.
#[tauri::command]
pub fn selftest_report(app: AppHandle, report: String) -> Result<(), String> {
    let Some(file) = requested() else { return Err("the self test is not enabled".into()) };
    std::fs::write(&file, &report).map_err(|e| e.to_string())?;
    let mut verdict = serde_json::from_str::<serde_json::Value>(&report).unwrap_or_default();
    if sailing()
        && let Some(repository) = SAMPLE.get()
        && let Err(why) = race_left(repository)
        && let Some(failed) = verdict["failed"].as_array_mut()
    {
        failed.push(serde_json::json!({ "name": "the repository holds the race's work", "detail": why }));
    }
    let failed = verdict["failed"].as_array().is_none_or(|f| !f.is_empty());
    let _ = std::fs::write(&file, verdict.to_string());
    app.exit(i32::from(failed));
    Ok(())
}
