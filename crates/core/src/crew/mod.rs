//! A Crew carries out a plan. A captain writes the plan, the user reads it, and then the pieces run:
//! those that do not wait for each other at the same time, each on its own worktree. A finished
//! piece is checked, may be looked over by a reviewer, and is merged into one integration branch.
//! There is no scheduler and no retry policy; when something goes wrong the user decides.

mod piece;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::agents::{Agent, Detected, Environment, Event, Mode, Profile, Request, Session, Transcript, take_turn};
use crate::check::Check;
use crate::git::{Merge, Repo, Snapshot, Worktree};
use crate::memory::Memory;
use crate::plan::{self, Piece, Plan};
use crate::race::BoatCheck;
use crate::roles::Roles;
use crate::store::Layout;
use crate::usage::{self, Log};

const STATE: &str = "crew.json";
const VERSION: u32 = 1;
const TICK: Duration = Duration::from_millis(100);
/// Most pieces that may run at the same time when the project does not say.
pub const DEFAULT_PARALLEL: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Waiting,
    Working,
    Checking,
    /// Done and checked; a reviewer is looking at it, or about to.
    Reviewing,
    Merged,
    /// The agent failed, or the check failed after a second try. The user decides.
    Failed,
    /// It does not merge cleanly. The user decides.
    Conflict,
    /// A reviewer turned it back twice. The user decides.
    Rejected,
    Skipped,
    Stopped,
}

impl Phase {
    /// Whether the piece will not change by itself any more.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Merged | Self::Skipped | Self::Stopped)
    }

    /// Whether the piece waits for the user.
    pub fn asks(self) -> bool {
        matches!(self, Self::Failed | Self::Conflict | Self::Rejected)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PieceState {
    pub piece: Piece,
    pub agent: String,
    pub model: String,
    pub phase: Phase,
    /// What the agent said it did, or why the piece could not go on.
    pub summary: String,
    pub files: Vec<String>,
    pub tokens: u64,
    pub duration_ms: u64,
    pub check: Option<BoatCheck>,
    pub attempts: u32,
    /// What a reviewer asked for, when it turned the piece back.
    pub feedback: String,
    /// How many times a reviewer has turned it back.
    pub review_rounds: u32,
    /// The id the agent's CLI uses to resume this piece's conversation.
    pub session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crew {
    pub version: u32,
    pub id: String,
    pub repository: String,
    pub brief: String,
    pub check: String,
    pub plan_summary: String,
    pub started_ms: u64,
    pub updated_ms: u64,
    /// The commit the crew started from.
    pub base: String,
    /// The branch the checkout was on, which a pull request goes into.
    #[serde(default)]
    pub base_branch: String,
    /// The branch everything merges into.
    pub integration: String,
    pub pieces: Vec<PieceState>,
    pub finished: bool,
    pub error: Option<String>,
    pub checkout_untouched: Option<bool>,
}

/// What the user decides about a piece that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run it again from where the integration branch is now.
    Redo(String),
    Skip(String),
    Stop,
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
    if plain(id) { Ok(layout.runs().join(id)) } else { Err("invalid crew id".into()) }
}

/// Writes the state through a temporary file so that a reader never sees half of it.
pub fn save(layout: &Layout, crew: &Crew) -> Result<(), String> {
    let dir = folder(layout, &crew.id)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temporary = dir.join(format!("{STATE}.tmp"));
    fs::write(&temporary, serde_json::to_vec(crew).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&temporary, dir.join(STATE)).map_err(|e| e.to_string())
}

pub fn load(layout: &Layout, id: &str) -> Result<Crew, String> {
    let bytes = fs::read(folder(layout, id)?.join(STATE)).map_err(|e| format!("no such crew: {e}"))?;
    let crew: Crew = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if crew.version > VERSION {
        return Err(format!("this crew was written by a newer Clipier (format {})", crew.version));
    }
    Ok(crew)
}

/// Every crew, the most recent first.
pub fn list(layout: &Layout) -> Vec<Crew> {
    let Ok(entries) = fs::read_dir(layout.runs()) else { return Vec::new() };
    let mut found: Vec<Crew> =
        entries.flatten().filter_map(|entry| load(layout, &entry.file_name().to_string_lossy()).ok()).collect();
    found.sort_by_key(|crew| std::cmp::Reverse(crew.started_ms));
    found
}

/// What a piece's agent has printed so far, from its latest turn.
pub fn progress(layout: &Layout, crew: &str, piece: &PieceState) -> Vec<Event> {
    let Some(agent) = Agent::from_name(&piece.agent) else { return Vec::new() };
    let Ok(dir) = folder(layout, crew) else { return Vec::new() };
    let out = dir.join("out");
    // The turn after a failed check is written next to the first, with `-fix` added.
    let label = format!("{}-{}", piece.piece.id, piece.attempts);
    let bytes = fs::read(out.join(format!("{label}-fix.out"))).or_else(|_| fs::read(out.join(format!("{label}.out"))));
    let Ok(bytes) = bytes else { return Vec::new() };
    let mut transcript = Transcript::new(agent);
    transcript.feed(&bytes);
    transcript.events().iter().map(|event| event.truncated(2_000)).collect()
}

/// What the machine and the project give a crew to work with.
pub struct Setup<'a> {
    pub layout: &'a Layout,
    pub environment: &'a Environment,
    pub cancel: &'a AtomicBool,
    pub roles: &'a Roles,
    /// The agents that are installed and signed in.
    pub available: &'a [Detected],
    pub check: Option<Check>,
    /// Most pieces that run at the same time.
    pub parallel: usize,
    /// Whether the check runs inside a sandbox with no network.
    pub sandbox_check: bool,
}

/// The captain's prompt: the role's own words, the roles it may use, the notes and the brief.
fn captain_prompt(captain: &crate::roles::Role, roles: &Roles, brief: &str, notes: &str) -> String {
    let mut list = String::new();
    for role in roles.list.iter().filter(|role| role.name != captain.name) {
        let mode = if role.mode == Mode::Write { "writes files" } else { "only reads" };
        let _ = writeln!(list, "- `{}` ({mode}): {}", role.name, role.prompt.lines().next().unwrap_or_default());
    }
    format!(
        "{}\n\n## The roles you can give pieces to\n\n{list}\n## The brief\n\n{brief}\n{notes}\n\n\
         ## What to reply with\n\nReply with ONLY a JSON object, no other words:\n\
         {{\"summary\": \"one sentence\", \"pieces\": [{{\"id\": \"short-id\", \"role\": \"builder\", \
         \"title\": \"what it does\", \"brief\": \"what the agent must do, with enough context to do it \
         alone\", \"touches\": [\"files or folders it may change\"], \"after\": [\"ids it waits for\"], \
         \"reviews\": null}}]}}\n\n\
         Rules: at most {} pieces; every piece that writes lists the files it may change; two writing pieces \
         that do not wait for each other must not touch the same files; a piece with `reviews` names a writing \
         piece, uses a role that only reads, and lists that piece in `after`.",
        captain.prompt,
        plan::MAX_PIECES
    )
}

/// What came out of asking the captain.
#[derive(Debug)]
pub struct Drawn {
    pub plan: Plan,
    pub tokens: u64,
}

/// Asks the captain for a plan, and once more if the plan breaks a rule. Read-only.
pub fn draw_plan(setup: &Setup<'_>, repository: &Path, brief: &str, out_dir: &Path) -> Result<Drawn, String> {
    let captain = setup.roles.get("captain").ok_or("there is no `captain` role")?;
    let assigned = captain.assign(setup.available).ok_or("no agent is available to be the captain")?;
    let notes = Memory::open(&setup.layout.memory(), repository).briefing();
    let mut request = Request {
        agent: assigned.agent,
        model: assigned.model,
        effort: None,
        mode: Mode::Read,
        profile: Profile::Focused,
        session: Session::New,
        prompt: captain_prompt(captain, setup.roles, brief, &notes),
        cwd: repository.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
        label: "captain".into(),
        total: Duration::from_mins(10),
    };
    let mut tokens = 0;
    for attempt in 0..2 {
        let turn = take_turn(&request, setup.environment, Duration::from_mins(3), setup.cancel);
        tokens += turn.effective_tokens();
        let _ = Log::new(&setup.layout.usage_file()).record(&usage::Use {
            at_ms: now_ms(),
            agent: request.agent.name().to_owned(),
            model: request.model.clone(),
            purpose: usage::Purpose::Crew,
            outcome: if turn.answer.is_ok() { usage::Outcome::Delivered } else { usage::Outcome::Failed },
            tokens_in: turn.tokens_in,
            tokens_cached: turn.tokens_cached,
            tokens_out: turn.tokens_out,
            duration_ms: u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX),
        });
        let reply = turn.answer.map_err(|(_, detail)| detail)?;
        let problems = match plan::parse(&reply) {
            Ok(plan) => match plan::validate(&plan, setup.roles) {
                Ok(()) => return Ok(Drawn { plan, tokens }),
                Err(problems) => problems,
            },
            Err(problem) => vec![problem],
        };
        if attempt == 1 {
            return Err(format!("the captain's plan breaks the rules: {}", problems.join("; ")));
        }
        let Some(session) = turn.session_id else {
            return Err(format!("the captain's plan breaks the rules: {}", problems.join("; ")));
        };
        request.session = Session::Resume(session);
        request.label = "captain-fix".into();
        request.prompt = format!(
            "The plan cannot be carried out:\n{}\nReply again with ONLY the corrected JSON object.",
            problems.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")
        );
    }
    Err("the captain did not produce a plan".into())
}

impl Crew {
    /// A crew that has not started: every piece waits for its role's agent.
    pub fn new(
        id: &str,
        repository: &Path,
        brief: &str,
        check: &str,
        plan: &Plan,
        setup: &Setup<'_>,
    ) -> Result<Self, String> {
        let mut pieces = Vec::new();
        for piece in &plan.pieces {
            let role = setup.roles.get(&piece.role).ok_or_else(|| format!("no role `{}`", piece.role))?;
            let assigned = role
                .assign(setup.available)
                .ok_or_else(|| format!("no agent is available for the role `{}`", role.name))?;
            pieces.push(PieceState {
                piece: piece.clone(),
                agent: assigned.agent.name().to_owned(),
                model: assigned.model,
                phase: Phase::Waiting,
                summary: String::new(),
                files: Vec::new(),
                tokens: 0,
                duration_ms: 0,
                check: None,
                attempts: 0,
                feedback: String::new(),
                review_rounds: 0,
                session: None,
            });
        }
        Ok(Self {
            version: VERSION,
            id: id.to_owned(),
            repository: repository.to_string_lossy().into_owned(),
            brief: brief.trim().to_owned(),
            check: check.trim().to_owned(),
            plan_summary: plan.summary.clone(),
            started_ms: now_ms(),
            updated_ms: now_ms(),
            base: String::new(),
            base_branch: String::new(),
            integration: format!("clipier/{id}/integration"),
            pieces,
            finished: false,
            error: None,
            checkout_untouched: None,
        })
    }

    fn piece(&self, id: &str) -> Option<&PieceState> {
        self.pieces.iter().find(|p| p.piece.id == id)
    }

    fn piece_mut(&mut self, id: &str) -> Option<&mut PieceState> {
        self.pieces.iter_mut().find(|p| p.piece.id == id)
    }
}

/// The state, updated under a lock and saved each time, so that the screen can follow it from disk.
fn update(layout: &Layout, shared: &Mutex<Crew>, change: impl FnOnce(&mut Crew)) {
    let mut crew = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    change(&mut crew);
    crew.updated_ms = now_ms();
    let _ = save(layout, &crew);
}

/// What a piece's thread sends back to the loop.
enum Outcome {
    /// A writing piece is committed and checked, or a reading piece has reported.
    Ready,
    /// A reviewer's verdict.
    Reviewed {
        approved: bool,
        notes: String,
    },
    Failed(String),
    Stopped,
}

struct Finished {
    piece: String,
    outcome: Outcome,
    /// The worktree the piece worked in, for the loop to keep or remove.
    tree: Option<Worktree>,
}

/// Which pieces can start now.
fn ready(crew: &Crew) -> Vec<String> {
    crew.pieces
        .iter()
        .filter(|state| state.phase == Phase::Waiting)
        .filter(|state| {
            state.piece.after.iter().all(|before| {
                let Some(dep) = crew.piece(before) else { return false };
                if state.piece.reviews.as_deref() == Some(before.as_str()) {
                    dep.phase == Phase::Reviewing
                } else {
                    dep.phase == Phase::Merged
                }
            })
        })
        .map(|state| state.piece.id.clone())
        .collect()
}

/// Pieces that wait for one that was skipped or stopped can never start: they are skipped too.
fn skip_the_orphans(crew: &mut Crew) {
    let dead: HashSet<String> = crew
        .pieces
        .iter()
        .filter(|s| matches!(s.phase, Phase::Skipped | Phase::Stopped))
        .map(|s| s.piece.id.clone())
        .collect();
    for state in &mut crew.pieces {
        if state.phase == Phase::Waiting && state.piece.after.iter().any(|before| dead.contains(before)) {
            state.phase = Phase::Skipped;
            "A piece it waits for was skipped.".clone_into(&mut state.summary);
        }
    }
}

fn finish_with_error(layout: &Layout, shared: &Mutex<Crew>, message: String) {
    update(layout, shared, |crew| {
        crew.error = Some(message);
        crew.finished = true;
    });
}

/// The loop's own view: who is running, and the worktrees it keeps between a piece's turns.
struct Loop<'a> {
    setup: &'a Setup<'a>,
    shared: &'a Mutex<Crew>,
    repo: &'a Repo,
    integration: &'a Worktree,
    running: HashSet<String>,
    trees: HashMap<String, Worktree>,
}

impl<'a> Loop<'a> {
    fn crew(&self) -> std::sync::MutexGuard<'_, Crew> {
        self.shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set(&self, id: &str, change: impl FnOnce(&mut PieceState)) {
        update(self.setup.layout, self.shared, |crew| {
            if let Some(state) = crew.piece_mut(id) {
                change(state);
            }
            skip_the_orphans(crew);
        });
    }

    /// Takes away a piece's worktree. The branch stays, so that the work stays reachable by name.
    fn drop_tree(&mut self, id: &str) {
        if let Some(tree) = self.trees.remove(id) {
            let _ = self.repo.remove_worktree(&tree);
        }
    }

    fn apply_commands(&mut self, commands: &Receiver<Command>) {
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Stop => self.setup.cancel.store(true, Ordering::Relaxed),
                Command::Skip(id) if self.asking(&id) => {
                    self.drop_tree(&id);
                    self.set(&id, |s| s.phase = Phase::Skipped);
                }
                Command::Redo(id) if self.asking(&id) => {
                    if let Some(tree) = self.trees.remove(&id) {
                        let _ = self.repo.remove_worktree(&tree);
                        let _ = self.repo.delete_branch(&tree.branch);
                    }
                    self.set(&id, |s| {
                        s.phase = Phase::Waiting;
                        s.feedback.clear();
                        s.review_rounds = 0;
                        s.session = None;
                        s.check = None;
                        s.files.clear();
                    });
                    // A reviewer that already looked has to look at the new work.
                    let reviewers: Vec<String> = self
                        .crew()
                        .pieces
                        .iter()
                        .filter(|s| s.piece.reviews.as_deref() == Some(id.as_str()))
                        .map(|s| s.piece.id.clone())
                        .collect();
                    for reviewer in reviewers {
                        self.set(&reviewer, |s| s.phase = Phase::Waiting);
                    }
                }
                Command::Skip(_) | Command::Redo(_) => {}
            }
        }
    }

    fn asking(&self, id: &str) -> bool {
        self.crew().piece(id).is_some_and(|s| s.phase.asks())
    }

    fn start<'scope>(&mut self, id: String, sender: &mpsc::Sender<Finished>, scope: &'scope thread::Scope<'scope, 'a>) {
        let own = self.trees.remove(&id);
        let reviewed = self
            .crew()
            .piece(&id)
            .and_then(|s| s.piece.reviews.clone())
            .and_then(|target| self.trees.get(&target).cloned());
        self.running.insert(id.clone());
        let (setup, shared, repo, integration, sender) =
            (self.setup, self.shared, self.repo, self.integration, sender.clone());
        scope.spawn(move || {
            let (outcome, tree) = piece::work(setup, shared, repo, integration, &id, own, reviewed.as_ref());
            let _ = sender.send(Finished { piece: id, outcome, tree });
        });
    }

    /// Merges a finished writing piece into the integration branch.
    fn merge(&mut self, id: &str) {
        let Some(tree) = self.trees.get(id).cloned() else {
            self.set(id, |s| s.phase = Phase::Merged);
            return;
        };
        let title = self.crew().piece(id).map(|s| s.piece.title.clone()).unwrap_or_default();
        let message = format!("chore(clipier): merge {id}: {title}");
        match self.integration.merge(&tree.branch, &message) {
            Ok(Merge::Merged) => {
                self.drop_tree(id);
                let _ = self.repo.delete_branch(&tree.branch);
                self.set(id, |s| s.phase = Phase::Merged);
            }
            Ok(Merge::Conflict(files)) => {
                self.drop_tree(id);
                self.set(id, |s| {
                    s.phase = Phase::Conflict;
                    s.summary = format!("It does not merge cleanly with the work already merged: {}", files.join(", "));
                });
            }
            Err(error) => {
                self.drop_tree(id);
                self.set(id, |s| {
                    s.phase = Phase::Failed;
                    s.summary = error.to_string();
                });
            }
        }
    }

    fn settle(&mut self, finished: Finished) {
        let Finished { piece: id, outcome, tree } = finished;
        self.running.remove(&id);
        if let Some(tree) = tree {
            self.trees.insert(id.clone(), tree);
        }
        match outcome {
            Outcome::Ready => {
                let (writes, has_reviewer) = {
                    let crew = self.crew();
                    (
                        self.trees.contains_key(&id),
                        crew.pieces.iter().any(|s| s.piece.reviews.as_deref() == Some(id.as_str())),
                    )
                };
                if !writes {
                    self.set(&id, |s| s.phase = Phase::Merged);
                } else if has_reviewer {
                    self.set(&id, |s| s.phase = Phase::Reviewing);
                } else {
                    self.merge(&id);
                }
            }
            Outcome::Reviewed { approved, notes } => self.settle_review(&id, approved, &notes),
            Outcome::Failed(reason) => {
                self.drop_tree(&id);
                self.set(&id, |s| {
                    s.phase = Phase::Failed;
                    s.summary = reason;
                });
            }
            Outcome::Stopped => {
                self.drop_tree(&id);
                self.set(&id, |s| s.phase = Phase::Stopped);
            }
        }
    }

    fn settle_review(&mut self, judge: &str, approved: bool, notes: &str) {
        let Some(target) = self.crew().piece(judge).and_then(|s| s.piece.reviews.clone()) else { return };
        self.set(judge, |s| {
            s.phase = Phase::Merged;
            notes.clone_into(&mut s.summary);
        });
        if approved {
            self.merge(&target);
            return;
        }
        let rounds = self.crew().piece(&target).map_or(0, |s| s.review_rounds);
        if rounds == 0 {
            // Back to the writer once, with what the judge asked for; the judge looks again.
            self.set(&target, |s| {
                s.phase = Phase::Waiting;
                s.review_rounds = 1;
                notes.clone_into(&mut s.feedback);
            });
            self.set(judge, |s| s.phase = Phase::Waiting);
        } else {
            self.set(&target, |s| {
                s.phase = Phase::Rejected;
                s.summary = format!("A judge turned it back twice: {notes}");
            });
        }
    }

    fn close(&mut self, before: &Snapshot) {
        let cancelled = self.setup.cancel.load(Ordering::Relaxed);
        let ids: Vec<String> = self.trees.keys().cloned().collect();
        for id in ids {
            self.drop_tree(&id);
        }
        let _ = self.repo.remove_worktree(self.integration);
        let untouched = self.repo.check_untouched(before).is_ok();
        update(self.setup.layout, self.shared, |crew| {
            crew.finished = true;
            crew.checkout_untouched = Some(untouched);
            for state in &mut crew.pieces {
                if cancelled && !state.phase.is_settled() && !state.phase.asks() {
                    state.phase = Phase::Stopped;
                }
            }
        });
    }
}

/// Runs the crew to the end. The state is saved as it moves; `commands` carries the user's decisions.
pub fn carry_out(setup: &Setup<'_>, shared: &Arc<Mutex<Crew>>, commands: &Receiver<Command>) {
    let (id, repository, integration_branch) = {
        let crew = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (crew.id.clone(), PathBuf::from(&crew.repository), crew.integration.clone())
    };
    let opened = Repo::open(&repository).and_then(|repo| repo.snapshot().map(|before| (repo, before)));
    let (repo, before): (Repo, Snapshot) = match opened {
        Ok(opened) => opened,
        Err(error) => return finish_with_error(setup.layout, shared, error.to_string()),
    };
    let integration_dir = setup.layout.worktrees().join(&id).join("integration");
    let integration = match repo.add_worktree(&integration_dir, &integration_branch, &before.head) {
        Ok(tree) => tree,
        Err(error) => return finish_with_error(setup.layout, shared, error.to_string()),
    };
    update(setup.layout, shared, |crew| {
        crew.base.clone_from(&before.head);
        crew.base_branch.clone_from(&before.branch);
    });

    let (sender, finished) = mpsc::channel::<Finished>();
    let mut state =
        Loop { setup, shared, repo: &repo, integration: &integration, running: HashSet::new(), trees: HashMap::new() };
    thread::scope(|scope| {
        loop {
            state.apply_commands(commands);
            let cancelled = setup.cancel.load(Ordering::Relaxed);
            if !cancelled {
                let to_start: Vec<String> =
                    ready(&state.crew()).into_iter().take(setup.parallel.saturating_sub(state.running.len())).collect();
                for piece in to_start {
                    state.set(&piece, |s| s.phase = Phase::Working);
                    state.start(piece, &sender, scope);
                }
            }
            match finished.recv_timeout(TICK) {
                Ok(done) => state.settle(done),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let all_settled = {
                let crew = state.crew();
                state.running.is_empty() && crew.pieces.iter().all(|s| s.phase.is_settled())
            };
            if state.running.is_empty() && (cancelled || all_settled) {
                break;
            }
        }
    });
    state.close(&before);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agents::Agent;
    use std::process::Command as Process;
    use std::sync::mpsc::Sender;

    fn folder_of(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clipier-crew-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Process::new("git")
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

    /// The plan the fake captain returns, escaped to sit inside a JSON string.
    const PLAN: &str = r#"{\"summary\":\"two steps and a look\",\"pieces\":[
        {\"id\":\"schema\",\"role\":\"builder\",\"title\":\"Define the format\",\"brief\":\"write schema.txt\",\"touches\":[\"schema.txt\"],\"after\":[]},
        {\"id\":\"command\",\"role\":\"builder\",\"title\":\"Add the command\",\"brief\":\"write command.txt\",\"touches\":[\"command.txt\"],\"after\":[\"schema\"]},
        {\"id\":\"look\",\"role\":\"reviewer\",\"title\":\"Look over the command\",\"brief\":\"check it\",\"touches\":[],\"after\":[\"command\"],\"reviews\":\"command\"}]}"#;

    /// A fake `claude` that answers by what it is asked: a plan, a file written in its folder, a verdict.
    fn fake(dir: &Path, verdict: &str, writes_outside: bool) -> Environment {
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let outside = if writes_outside { "echo x > other.txt; " } else { "" };
        let plan = PLAN.replace('\n', " ");
        let script = format!(
            r#"#!/bin/sh
prompt=""; while [ $# -gt 0 ]; do [ "$1" = "-p" ] && prompt="$2"; shift; done
emit() {{ printf '%s\n' '{{"type":"system","subtype":"init","session_id":"s"}}'; printf '%s\n' "{{\"type\":\"result\",\"session_id\":\"s\",\"is_error\":false,\"result\":\"$1\",\"usage\":{{}}}}"; }}
case "$prompt" in
  *"What to reply with"*) emit '{plan}' ;;
  *"The change to look over"*) emit '{verdict}' ;;
  *"A reviewer looked at your work"*) echo revised >> command.txt; emit 'revised it' ;;
  *"Your piece: Define"*) echo schema > schema.txt; emit 'wrote the schema' ;;
  *"Your piece: Add the command"*) {outside}echo command > command.txt; emit 'wrote the command' ;;
  *) emit 'what?' ;;
esac
"#
        );
        let path = bin.join("claude");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        Environment {
            home: dir.to_owned(),
            path_prefix: vec![bin],
            vars: vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
        }
    }

    fn available() -> Vec<Detected> {
        vec![Detected {
            agent: Agent::Claude,
            installed: true,
            version: None,
            signed_in: Some(true),
            account: None,
            models: Vec::new(),
            allowance: None,
        }]
    }

    struct Fixture {
        dir: PathBuf,
        repository: PathBuf,
        layout: Layout,
        roles: Roles,
        environment: Environment,
        available: Vec<Detected>,
        cancel: AtomicBool,
    }

    impl Fixture {
        fn new(name: &str, verdict: &str, writes_outside: bool) -> Self {
            let dir = folder_of(name);
            Self {
                repository: repository(&dir),
                layout: Layout::new(&dir.join("data")),
                roles: Roles { list: crate::roles::builtins(), problems: Vec::new() },
                environment: fake(&dir, verdict, writes_outside),
                available: available(),
                cancel: AtomicBool::new(false),
                dir,
            }
        }

        fn setup(&self) -> Setup<'_> {
            Setup {
                layout: &self.layout,
                environment: &self.environment,
                cancel: &self.cancel,
                roles: &self.roles,
                available: &self.available,
                check: None,
                parallel: 3,
                sandbox_check: false,
            }
        }

        /// Draws the plan and carries it out; `decide` answers what the crew asks, from another thread.
        fn run(&self, id: &str, decide: impl FnOnce(&Arc<Mutex<Crew>>, &Sender<Command>) + Send) -> Crew {
            let setup = self.setup();
            let out = self.layout.runs().join(id).join("out");
            let drawn = draw_plan(&setup, &self.repository, "add an export", &out).unwrap();
            let crew = Crew::new(id, &self.repository, "add an export", "", &drawn.plan, &setup).unwrap();
            save(&self.layout, &crew).unwrap();
            let shared = Arc::new(Mutex::new(crew));
            let (sender, commands) = mpsc::channel();
            thread::scope(|scope| {
                let watcher = &shared;
                scope.spawn(move || decide(watcher, &sender));
                carry_out(&setup, &shared, &commands);
            });
            load(&self.layout, id).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn phase(crew: &Crew, id: &str) -> Phase {
        crew.piece(id).unwrap().phase
    }

    /// Waits until `id` asks, then answers.
    fn answer_when_asked(shared: &Arc<Mutex<Crew>>, sender: &Sender<Command>, id: &str, answer: Command) {
        for _ in 0..600 {
            if shared.lock().unwrap().piece(id).is_some_and(|s| s.phase.asks()) {
                sender.send(answer).unwrap();
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_plan_is_carried_out_in_order_reviewed_and_merged_into_one_branch() {
        let fixture = Fixture::new("happy", "approved", false);
        let before = Repo::open(&fixture.repository).unwrap().snapshot().unwrap();
        let crew = fixture.run("calm-heron-0001", |_, _| {});

        assert!(crew.finished && crew.error.is_none(), "{crew:?}");
        for id in ["schema", "command", "look"] {
            assert_eq!(phase(&crew, id), Phase::Merged, "{id}: {}", crew.piece(id).unwrap().summary);
        }
        assert_eq!(crew.piece("look").unwrap().summary, "approved");
        let tree = git(&fixture.repository, &["ls-tree", "--name-only", &crew.integration]);
        assert!(tree.contains("schema.txt") && tree.contains("command.txt"), "{tree}");
        assert_eq!(crew.checkout_untouched, Some(true));
        assert_eq!(crew.base_branch, "main");
        Repo::open(&fixture.repository).unwrap().check_untouched(&before).unwrap();
        let usage = crate::usage::Log::new(&fixture.layout.usage_file()).totals();
        assert!(usage["claude"].uses >= 4, "the captain and every piece went through the usage log");
    }

    #[test]
    fn a_review_that_turns_a_piece_back_twice_asks_the_user_who_skips_it() {
        let fixture = Fixture::new("rejected", "make it shorter", false);
        let crew = fixture.run("calm-heron-0002", |shared, sender| {
            answer_when_asked(shared, sender, "command", Command::Skip("command".into()));
        });
        assert_eq!(phase(&crew, "schema"), Phase::Merged);
        assert_eq!(phase(&crew, "command"), Phase::Skipped);
        assert_eq!(crew.piece("command").unwrap().review_rounds, 1, "it went back to its writer once");
        let tree = git(&fixture.repository, &["ls-tree", "--name-only", &crew.integration]);
        assert!(tree.contains("schema.txt") && !tree.contains("command.txt"), "{tree}");
        assert!(crew.finished);
    }

    #[test]
    fn a_piece_that_writes_outside_its_files_fails_and_what_waits_for_it_is_skipped() {
        let fixture = Fixture::new("scope", "approved", true);
        let crew = fixture.run("calm-heron-0003", |shared, sender| {
            answer_when_asked(shared, sender, "command", Command::Skip("command".into()));
        });
        assert!(crew.piece("command").unwrap().summary.contains("outside the files"), "{:?}", crew.piece("command"));
        assert_eq!(phase(&crew, "command"), Phase::Skipped);
        assert_eq!(phase(&crew, "look"), Phase::Skipped, "a review of a skipped piece has nothing to look at");
        assert!(!git(&fixture.repository, &["ls-tree", "--name-only", &crew.integration]).contains("other.txt"));
    }

    #[test]
    fn a_stopped_crew_stops_every_piece_and_leaves_the_checkout_alone() {
        let fixture = Fixture::new("stop", "approved", false);
        let crew = fixture.run("calm-heron-0004", |shared, sender| {
            // Stop as soon as the first piece starts.
            for _ in 0..600 {
                if shared.lock().unwrap().pieces.iter().any(|s| s.phase != Phase::Waiting) {
                    sender.send(Command::Stop).unwrap();
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        assert!(crew.finished);
        assert!(
            crew.pieces.iter().all(|s| s.phase.is_settled() || s.phase.asks()),
            "{:?}",
            crew.pieces.iter().map(|s| s.phase).collect::<Vec<_>>()
        );
        assert_eq!(crew.checkout_untouched, Some(true));
    }
}
