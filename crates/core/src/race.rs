//! A Race: one brief, several boats. Each boat is an agent and model that works on its own copy of
//! the repository at the same time as the others; the user reads what each did and keeps one.
//! Nothing reaches the user's checkout.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::agents::{Agent, Environment, Event, Mode, Profile, Request, Session, Transcript, take_turn};
use crate::check::{self, Check};
use crate::git::{Repo, Snapshot, Worktree};
use crate::memory::Memory;
use crate::platform;
use crate::session::{NOTE_PREFIX, cut, take_notes};
use crate::store::Layout;
use crate::usage::{self, Log};

const STATE: &str = "race.json";
const VERSION: u32 = 1;
const BOAT_LIMIT: Duration = Duration::from_mins(30);
const BOAT_STALL: Duration = Duration::from_mins(5);
const CHECK_LIMIT: Duration = Duration::from_mins(10);
/// Most boats in one race: each is a full copy of the repository and a paid conversation.
pub const MAX_BOATS: usize = 4;
const DIFF_LIMIT: usize = 120_000;
const SUMMARY_CUT: usize = 4_000;
const NOTES_PER_BOAT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Waiting,
    Working,
    Checking,
    Done,
    /// The agent could not finish, or finished without changing anything.
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoatCheck {
    pub passed: bool,
    pub headline: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boat {
    pub id: String,
    pub agent: String,
    pub model: String,
    pub phase: Phase,
    /// What the agent said it did, or why it failed.
    pub summary: String,
    pub files: Vec<String>,
    pub tokens: u64,
    pub duration_ms: u64,
    pub check: Option<BoatCheck>,
    /// The branch that holds the boat's work, once it has any.
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Race {
    pub version: u32,
    pub id: String,
    pub repository: String,
    pub brief: String,
    pub check: String,
    pub started_ms: u64,
    pub updated_ms: u64,
    /// The commit every boat started from.
    pub base: String,
    /// The branch the checkout was on, which a pull request goes into.
    #[serde(default)]
    pub base_branch: String,
    pub boats: Vec<Boat>,
    pub finished: bool,
    pub error: Option<String>,
    /// Whether the user's checkout was exactly as it was when the race ended; `None` until then.
    pub checkout_untouched: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoatSpec {
    pub agent: String,
    pub model: String,
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

fn folder(layout: &Layout, id: &str) -> Result<PathBuf, String> {
    if plain(id) { Ok(layout.runs().join(id)) } else { Err("invalid race id".into()) }
}

impl Race {
    /// A race that has not started: every boat waits.
    pub fn new(id: &str, repository: &Path, brief: &str, check: &str, specs: &[BoatSpec]) -> Result<Self, String> {
        if specs.is_empty() || specs.len() > MAX_BOATS {
            return Err(format!("choose between 1 and {MAX_BOATS} boats"));
        }
        if let Some(unknown) = specs.iter().find(|spec| Agent::from_name(&spec.agent).is_none()) {
            return Err(format!("unknown agent `{}`", unknown.agent));
        }
        let boats = specs
            .iter()
            .enumerate()
            .map(|(index, spec)| Boat {
                id: format!("{}-{}", spec.agent, index + 1),
                agent: spec.agent.clone(),
                model: spec.model.clone(),
                phase: Phase::Waiting,
                summary: String::new(),
                files: Vec::new(),
                tokens: 0,
                duration_ms: 0,
                check: None,
                branch: None,
            })
            .collect();
        Ok(Self {
            version: VERSION,
            id: id.to_owned(),
            repository: repository.to_string_lossy().into_owned(),
            brief: brief.trim().to_owned(),
            check: check.trim().to_owned(),
            started_ms: now_ms(),
            updated_ms: now_ms(),
            base: String::new(),
            base_branch: String::new(),
            boats,
            finished: false,
            error: None,
            checkout_untouched: None,
        })
    }
}

/// Writes the state through a temporary file so that a reader never sees half of it.
pub fn save(layout: &Layout, race: &Race) -> Result<(), String> {
    let dir = folder(layout, &race.id)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_vec(race).map_err(|e| e.to_string())?;
    let temporary = dir.join(format!("{STATE}.tmp"));
    fs::write(&temporary, body).map_err(|e| e.to_string())?;
    fs::rename(&temporary, dir.join(STATE)).map_err(|e| e.to_string())
}

pub fn load(layout: &Layout, id: &str) -> Result<Race, String> {
    let bytes = fs::read(folder(layout, id)?.join(STATE)).map_err(|e| format!("no such race: {e}"))?;
    let race: Race = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if race.version > VERSION {
        return Err(format!("this race was written by a newer Leme Pier (format {})", race.version));
    }
    Ok(race)
}

/// Every race, the most recent first.
pub fn list(layout: &Layout) -> Vec<Race> {
    let Ok(entries) = fs::read_dir(layout.runs()) else { return Vec::new() };
    let mut found: Vec<Race> =
        entries.flatten().filter_map(|entry| load(layout, &entry.file_name().to_string_lossy()).ok()).collect();
    found.sort_by_key(|race| std::cmp::Reverse(race.started_ms));
    found
}

/// What the boat has printed so far, read from its output file.
pub fn progress(layout: &Layout, race: &str, boat: &Boat) -> Vec<Event> {
    let Some(agent) = Agent::from_name(&boat.agent) else { return Vec::new() };
    let Ok(dir) = folder(layout, race) else { return Vec::new() };
    let Ok(bytes) = fs::read(dir.join("out").join(format!("{}.out", boat.id))) else { return Vec::new() };
    let mut transcript = Transcript::new(agent);
    transcript.feed(&bytes);
    transcript.events().iter().map(|event| event.truncated(2_000)).collect()
}

/// What one boat changed against where the race started, cut to a size the screen can show.
pub fn diff(race: &Race, boat: &str) -> Result<String, String> {
    let branch = race
        .boats
        .iter()
        .find(|b| b.id == boat)
        .and_then(|b| b.branch.clone())
        .ok_or_else(|| "that boat has no work to show".to_owned())?;
    let repo = Repo::open(Path::new(&race.repository)).map_err(|e| e.to_string())?;
    repo.diff(&race.base, &branch, DIFF_LIMIT).map_err(|e| e.to_string())
}

/// What a boat is told: the job, that others work on it in parallel elsewhere, and the project's notes.
fn instructions(brief: &str, notes: &str) -> String {
    format!(
        "You are one of several agents given the same task at the same time. Each works in its own copy of \
         the repository; yours is the current folder. Work only here. Do the task, then reply with a short \
         summary of what you changed and anything the reader must know.\n\n## Task\n\n{brief}\n{notes}\n\n\
         If you learn something about this project that a later agent could not see in the code, end your reply \
         with a line `{NOTE_PREFIX} <short title> | <one or two sentences>`. Never put secrets in a note."
    )
}

/// Rewrites a brief so that an agent who knows nothing else can act on it. One read-only turn.
pub fn improve_brief(
    repository: &Path,
    out_dir: &Path,
    brief: &str,
    spec: &BoatSpec,
    environment: &Environment,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let agent = Agent::from_name(&spec.agent).ok_or_else(|| format!("unknown agent `{}`", spec.agent))?;
    let prompt = format!(
        "Rewrite the task brief below so that a coding agent who knows nothing else can do it well: say what \
         must change, where it probably lives (look at the repository if you need to), what \"done\" looks like \
         and what must not change. Keep the user's intent and their language. Do not add requirements they did \
         not imply. Do not change any file. Reply with ONLY the improved brief, no preface.\n\n## Brief\n\n{brief}"
    );
    let request = Request {
        agent,
        model: spec.model.clone(),
        effort: None,
        mode: Mode::Read,
        profile: Profile::Focused,
        session: Session::New,
        prompt,
        cwd: repository.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
        label: "improve".into(),
        total: Duration::from_mins(5),
    };
    take_turn(&request, environment, Duration::from_mins(2), cancel)
        .answer
        .map(|text| text.trim().to_owned())
        .map_err(|(_, detail)| detail)
}

pub struct Sail<'a> {
    pub layout: &'a Layout,
    pub environment: &'a Environment,
    pub cancel: &'a AtomicBool,
    /// Whether the check runs inside a sandbox with no network.
    pub sandbox_check: bool,
}

/// The state, updated under a lock and saved each time, so that the screen can follow it from disk.
fn update(layout: &Layout, shared: &Mutex<Race>, change: impl FnOnce(&mut Race)) {
    let mut race = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    change(&mut race);
    race.updated_ms = now_ms();
    let _ = save(layout, &race);
}

fn set_boat(layout: &Layout, shared: &Mutex<Race>, id: &str, change: impl FnOnce(&mut Boat)) {
    update(layout, shared, |race| {
        if let Some(boat) = race.boats.iter_mut().find(|b| b.id == id) {
            change(boat);
        }
    });
}

/// Everything a boat's thread needs from the race it belongs to.
struct Context<'a> {
    sail: &'a Sail<'a>,
    shared: &'a Mutex<Race>,
    base: &'a str,
    check: &'a Option<Check>,
    prompt: &'a str,
    out_dir: &'a Path,
    id: &'a str,
    brief: &'a str,
    memory: &'a Memory,
}

fn verdict(check: &Check, tree: &Worktree, sail: &Sail<'_>) -> BoatCheck {
    let sandbox = if sail.sandbox_check {
        check::Sandbox::Bubblewrap { home: platform::home_dir() }
    } else {
        check::Sandbox::None
    };
    match check::run(check, &tree.dir, &sandbox, CHECK_LIMIT, sail.cancel) {
        Ok(result) => BoatCheck { passed: result.passed(), headline: result.headline(), output: result.output },
        Err(error) => {
            BoatCheck { passed: false, headline: "the check could not run".into(), output: error.to_string() }
        }
    }
}

/// One boat from the start of its work to its verdict.
fn run_boat(context: &Context<'_>, boat: &str, agent: Agent, model: &str, tree: &Worktree) {
    let Context { sail, shared, base, check, prompt, out_dir, id, brief, memory } = *context;
    set_boat(sail.layout, shared, boat, |b| b.phase = Phase::Working);
    let request = Request {
        agent,
        model: model.to_owned(),
        effort: None,
        mode: Mode::Write,
        profile: Profile::Full,
        session: Session::New,
        prompt: prompt.to_owned(),
        cwd: tree.dir.clone(),
        out_dir: out_dir.to_path_buf(),
        label: boat.to_owned(),
        total: BOAT_LIMIT,
    };
    let turn = take_turn(&request, sail.environment, BOAT_STALL, sail.cancel);
    let tokens = turn.effective_tokens();
    let duration_ms = u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX);
    let outcome = match &turn.answer {
        Ok(_) => usage::Outcome::Delivered,
        Err((crate::runner::Failure::Cancelled, _)) => usage::Outcome::Stopped,
        Err(_) => usage::Outcome::Failed,
    };
    let _ = Log::new(&sail.layout.usage_file()).record(&usage::Use {
        at_ms: now_ms(),
        agent: agent.name().to_owned(),
        model: model.to_owned(),
        purpose: usage::Purpose::Race,
        outcome,
        tokens_in: turn.tokens_in,
        tokens_cached: turn.tokens_cached,
        tokens_out: turn.tokens_out,
        duration_ms,
    });
    let reply = match turn.answer {
        Ok(reply) => reply,
        Err((failure, detail)) => {
            set_boat(sail.layout, shared, boat, |b| {
                b.phase = if failure == crate::runner::Failure::Cancelled { Phase::Stopped } else { Phase::Failed };
                b.summary = cut(&detail, SUMMARY_CUT);
                b.tokens = tokens;
                b.duration_ms = duration_ms;
            });
            return;
        }
    };
    let (reply, notes) = take_notes(&reply);
    memory.propose_up_to(&notes, id, NOTES_PER_BOAT);
    let files = tree.changed_files(base).unwrap_or_default();
    let (phase, summary) = match tree.commit(&format!("feat({boat}): {}", cut(brief, 50))) {
        Ok(Some(_)) => (Phase::Checking, reply),
        Ok(None) => (Phase::Failed, format!("It changed nothing. It said: {}", cut(&reply, SUMMARY_CUT))),
        Err(error) => (Phase::Failed, error.to_string()),
    };
    set_boat(sail.layout, shared, boat, |b| {
        b.phase = phase;
        b.summary = summary;
        b.files = files;
        b.tokens = tokens;
        b.duration_ms = duration_ms;
        b.branch = (phase != Phase::Failed).then(|| tree.branch.clone());
    });
    if phase == Phase::Checking {
        let result = check.as_ref().map(|check| verdict(check, tree, sail));
        set_boat(sail.layout, shared, boat, |b| {
            b.phase = Phase::Done;
            b.check = result;
        });
    }
}

fn fail_all(layout: &Layout, shared: &Mutex<Race>, message: String) {
    update(layout, shared, |race| {
        race.error = Some(message);
        race.finished = true;
        for boat in &mut race.boats {
            boat.phase = Phase::Failed;
        }
    });
}

/// What a race was asked to do.
struct Asked {
    id: String,
    repository: PathBuf,
    brief: String,
    check: String,
    /// Each boat's id, agent and model.
    boats: Vec<(String, String, String)>,
}

fn asked(shared: &Mutex<Race>) -> Asked {
    let race = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Asked {
        id: race.id.clone(),
        repository: PathBuf::from(&race.repository),
        brief: race.brief.clone(),
        check: race.check.clone(),
        boats: race.boats.iter().map(|b| (b.id.clone(), b.agent.clone(), b.model.clone())).collect(),
    }
}

/// Sets every boat off at once and waits for all of them. The state is saved as it moves.
pub fn sail(sail: &Sail<'_>, shared: &Arc<Mutex<Race>>) {
    let Asked { id, repository, brief, check: check_text, boats } = asked(shared);
    let check = if check_text.is_empty() {
        None
    } else {
        match Check::parse(&check_text) {
            Ok(check) => Some(check),
            Err(error) => {
                return fail_all(sail.layout, shared, format!("the check is not a command Leme can run: {error}"));
            }
        }
    };
    let (repo, before): (Repo, Snapshot) =
        match Repo::open(&repository).and_then(|repo| repo.snapshot().map(|s| (repo, s))) {
            Ok(opened) => opened,
            Err(error) => return fail_all(sail.layout, shared, error.to_string()),
        };
    update(sail.layout, shared, |race| {
        race.base.clone_from(&before.head);
        race.base_branch.clone_from(&before.branch);
    });

    // Worktrees are made one after another: git locks the repository while it adds one.
    let mut trees: Vec<(String, Agent, String, Worktree)> = Vec::new();
    for (boat, agent, model) in boats {
        let dir = sail.layout.worktrees().join(&id).join(&boat);
        match repo.add_worktree(&dir, &format!("leme/{id}/{boat}"), &before.head) {
            Ok(tree) => {
                trees.push((boat, Agent::from_name(&agent).expect("checked when the race was made"), model, tree));
            }
            Err(error) => set_boat(sail.layout, shared, &boat, |b| {
                b.phase = Phase::Failed;
                b.summary = error.to_string();
            }),
        }
    }
    let out_dir = sail.layout.runs().join(&id).join("out");
    let _ = fs::create_dir_all(&out_dir);
    let memory = Memory::open(&sail.layout.memory(), &repository);
    let prompt = instructions(&brief, &memory.briefing());
    let context = Context {
        sail,
        shared,
        base: &before.head,
        check: &check,
        prompt: &prompt,
        out_dir: &out_dir,
        id: &id,
        brief: &brief,
        memory: &memory,
    };
    thread::scope(|scope| {
        for (boat, agent, model, tree) in &trees {
            let context = &context;
            scope.spawn(move || run_boat(context, boat, *agent, model, tree));
        }
    });

    for (_, _, _, tree) in &trees {
        let _ = repo.remove_worktree(tree);
    }
    let untouched = repo.check_untouched(&before).is_ok();
    let cancelled = sail.cancel.load(Ordering::Relaxed);
    update(sail.layout, shared, |race| {
        race.finished = true;
        race.checkout_untouched = Some(untouched);
        if cancelled {
            for boat in &mut race.boats {
                if matches!(boat.phase, Phase::Waiting | Phase::Working | Phase::Checking) {
                    boat.phase = Phase::Stopped;
                }
            }
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    fn folder_of(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("leme-race-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn repository(dir: &Path) -> PathBuf {
        let repository = dir.join("repo");
        fs::create_dir_all(&repository).unwrap();
        git(&repository, &["init", "-q", "-b", "main"]);
        fs::write(repository.join("a.txt"), "hello\n").unwrap();
        git(&repository, &["add", "-A"]);
        git(&repository, &["commit", "-q", "-m", "init"]);
        repository
    }

    /// A fake `claude` that runs `script` in the folder it is started in.
    fn fake_environment(dir: &Path, script: &str) -> Environment {
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

    const WRITES_A_FILE: &str = r#"echo "boat $$" > "greeting-$$.txt"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"s"}'
printf '%s\n' '{"type":"result","session_id":"s","is_error":false,"result":"Wrote a greeting.","usage":{"input_tokens":3,"output_tokens":4}}'"#;

    fn race_of(repository: &Path, id: &str, check: &str) -> Arc<Mutex<Race>> {
        let spec = BoatSpec { agent: "claude".into(), model: "m".into() };
        Arc::new(Mutex::new(Race::new(id, repository, "Add a greeting file", check, &[spec.clone(), spec]).unwrap()))
    }

    fn sail_it(layout: &Layout, shared: &Arc<Mutex<Race>>, environment: &Environment) {
        let cancel = AtomicBool::new(false);
        sail(&Sail { layout, environment, cancel: &cancel, sandbox_check: false }, shared);
    }

    #[test]
    fn every_boat_works_in_its_own_copy_and_the_checkout_is_left_alone() {
        let dir = folder_of("sail");
        let repository = repository(&dir);
        let layout = Layout::new(&dir.join("data"));
        let shared = race_of(&repository, "fair-kestrel-0001", "true");
        sail_it(&layout, &shared, &fake_environment(&dir, WRITES_A_FILE));

        let saved = load(&layout, "fair-kestrel-0001").unwrap();
        assert!(saved.finished && saved.error.is_none(), "{saved:?}");
        assert_eq!(saved.checkout_untouched, Some(true));
        assert_eq!(saved.base_branch, "main", "a pull request goes into the branch the checkout was on");
        for boat in &saved.boats {
            assert_eq!(
                (boat.phase, boat.summary.as_str(), boat.tokens),
                (Phase::Done, "Wrote a greeting.", 7),
                "{boat:?}"
            );
            assert!(boat.check.as_ref().is_some_and(|check| check.passed), "{boat:?}");
            assert_eq!(boat.files.len(), 1, "{:?}", boat.files);
            let branch = boat.branch.clone().expect("the work is kept on a branch");
            assert!(git(&repository, &["ls-tree", "--name-only", &branch]).contains("greeting-"));
        }
        assert_ne!(saved.boats[0].files, saved.boats[1].files, "each boat wrote in its own copy");
        assert_eq!(git(&repository, &["status", "--porcelain"]), "", "the checkout was not touched");
        assert_eq!(git(&repository, &["rev-parse", "HEAD"]), saved.base);
        assert!(diff(&saved, "claude-1").unwrap().contains("+boat "));
        assert!(
            !layout.worktrees().join("fair-kestrel-0001/claude-1").exists(),
            "the folders are removed, the branches stay"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_boat_that_changes_nothing_has_failed_and_a_failing_check_is_reported() {
        let dir = folder_of("verdicts");
        let repository = repository(&dir);
        let layout = Layout::new(&dir.join("data"));
        let silent = fake_environment(
            &dir,
            r#"printf '%s\n' '{"type":"result","session_id":"s","is_error":false,"result":"All done.","usage":{}}'"#,
        );
        let shared = race_of(&repository, "fair-kestrel-0002", "false");
        sail_it(&layout, &shared, &silent);
        let saved = load(&layout, "fair-kestrel-0002").unwrap();
        assert!(saved.boats.iter().all(|b| b.phase == Phase::Failed && b.branch.is_none()), "{saved:?}");
        assert!(saved.boats[0].summary.starts_with("It changed nothing"));

        let shared = race_of(&repository, "fair-kestrel-0003", "false");
        sail_it(&layout, &shared, &fake_environment(&dir, WRITES_A_FILE));
        let saved = load(&layout, "fair-kestrel-0003").unwrap();
        assert!(saved.boats.iter().all(|b| b.phase == Phase::Done && b.check.as_ref().is_some_and(|c| !c.passed)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_check_that_is_not_a_command_stops_the_race_before_it_starts() {
        let dir = folder_of("badcheck");
        let repository = repository(&dir);
        let layout = Layout::new(&dir.join("data"));
        let shared = race_of(&repository, "fair-kestrel-0004", "'unterminated");
        sail_it(&layout, &shared, &fake_environment(&dir, WRITES_A_FILE));
        let saved = load(&layout, "fair-kestrel-0004").unwrap();
        assert!(saved.finished && saved.error.is_some());
        assert_eq!(git(&repository, &["branch", "--list", "leme/*"]), "", "no branch was made");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_race_needs_one_to_four_known_boats_and_a_safe_id() {
        let spec = |agent: &str| BoatSpec { agent: agent.into(), model: String::new() };
        let repository = Path::new("/r");
        assert!(Race::new("x", repository, "b", "", &[]).is_err());
        assert!(Race::new("x", repository, "b", "", &vec![spec("claude"); 5]).is_err());
        assert!(Race::new("x", repository, "b", "", &[spec("gemini")]).is_err());
        assert!(Race::new("x", repository, "b", "", &[spec("claude"), spec("codex")]).is_ok());
        let layout = Layout::new(Path::new("/data"));
        for bad in ["../x", "a/b", "", "-x", ".hidden"] {
            assert!(folder(&layout, bad).is_err(), "{bad}");
        }
    }
}
