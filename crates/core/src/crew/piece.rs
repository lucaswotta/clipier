//! One piece of a Crew, from the start of its work to what it reports back.

use std::fmt::Write as _;
use std::path::Path;

use super::{Crew, Outcome, Phase, PieceState, Setup, update};
use crate::agents::{Agent, Mode, Profile, Request, Session, Turn, take_turn};
use crate::check::{self, Check};
use crate::git::{Repo, Worktree};
use crate::memory::Memory;
use crate::race::BoatCheck;
use crate::roles::Role;
use crate::runner::Failure;
use crate::session::{NOTE_PREFIX, cut, take_notes};
use crate::usage::{self, Log};

const PIECE_LIMIT: std::time::Duration = std::time::Duration::from_mins(30);
const PIECE_STALL: std::time::Duration = std::time::Duration::from_mins(5);
const CHECK_LIMIT: std::time::Duration = std::time::Duration::from_mins(10);
const CHANGE_LIMIT: usize = 24_000;
const SUMMARY_CUT: usize = 4_000;
const NOTES_PER_PIECE: usize = 2;

struct Piece<'a> {
    setup: &'a Setup<'a>,
    shared: &'a std::sync::Mutex<Crew>,
    repo: &'a Repo,
    integration: &'a Worktree,
    id: &'a str,
    role: &'a Role,
    run: String,
    integration_branch: String,
    repository: std::path::PathBuf,
}

impl Piece<'_> {
    fn state(&self) -> PieceState {
        let crew = self.shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        crew.piece(self.id).cloned().expect("the loop only starts pieces of the plan")
    }

    fn set(&self, change: impl FnOnce(&mut PieceState)) {
        update(self.setup.layout, self.shared, |crew| {
            if let Some(state) = crew.piece_mut(self.id) {
                change(state);
            }
        });
    }

    fn out_dir(&self) -> std::path::PathBuf {
        self.setup.layout.runs().join(&self.run).join("out")
    }

    /// One turn with the piece's agent; when the agent is unusable or limited, once with the role's
    /// fallback. Everything that goes through is added to the usage log.
    fn turn(&self, prompt: &str, session: &Session, cwd: &Path, label: &str) -> Turn {
        let mut state = self.state();
        let mut fell_back = false;
        loop {
            let Some(agent) = Agent::from_name(&state.agent) else {
                return failed_turn(format!("unknown agent `{}`", state.agent));
            };
            let request = Request {
                agent,
                model: state.model.clone(),
                effort: None,
                mode: self.role.mode,
                profile: Profile::Focused,
                session: if fell_back { Session::New } else { session.clone() },
                prompt: prompt.to_owned(),
                cwd: cwd.to_path_buf(),
                out_dir: self.out_dir(),
                label: label.to_owned(),
                total: PIECE_LIMIT,
            };
            let turn = take_turn(&request, self.setup.environment, PIECE_STALL, self.setup.cancel);
            let outcome = match &turn.answer {
                Ok(_) => usage::Outcome::Delivered,
                Err((Failure::Cancelled, _)) => usage::Outcome::Stopped,
                Err(_) => usage::Outcome::Failed,
            };
            let _ = Log::new(&self.setup.layout.usage_file()).record(&usage::Use {
                at_ms: super::now_ms(),
                agent: state.agent.clone(),
                model: state.model.clone(),
                purpose: usage::Purpose::Crew,
                outcome,
                tokens_in: turn.tokens_in,
                tokens_cached: turn.tokens_cached,
                tokens_out: turn.tokens_out,
                duration_ms: u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX),
            });
            let unreachable = matches!(&turn.answer, Err((Failure::Unusable | Failure::Limited, _)));
            let Some(fallback) = self.role.fallback.as_ref().filter(|_| unreachable && !fell_back) else {
                return turn;
            };
            fell_back = true;
            state.agent.clone_from(&fallback.agent);
            state.model.clone_from(&fallback.model);
            self.set(|s| {
                s.agent.clone_from(&fallback.agent);
                s.model.clone_from(&fallback.model);
            });
        }
    }

    fn notes(&self) -> String {
        Memory::open(&self.setup.layout.memory(), &self.repository).briefing()
    }

    fn learn(&self, reply: &str) -> String {
        let (reply, notes) = take_notes(reply);
        Memory::open(&self.setup.layout.memory(), &self.repository).propose_up_to(&notes, &self.run, NOTES_PER_PIECE);
        reply
    }

    /// What the pieces this one comes after said, since the merged code does not say why.
    fn before_it(&self, state: &PieceState) -> String {
        let crew = self.shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut lines = String::new();
        for before in state.piece.after.iter().filter_map(|id| crew.piece(id)) {
            let _ = writeln!(lines, "- `{}` ({}): {}", before.piece.id, before.piece.title, cut(&before.summary, 600));
        }
        if lines.is_empty() { String::new() } else { format!("\n## What the pieces before yours did\n\n{lines}") }
    }
}

fn failed_turn(detail: String) -> Turn {
    Turn {
        answer: Err((Failure::Unusable, detail)),
        session_id: None,
        events: Vec::new(),
        tokens_in: 0,
        tokens_cached: 0,
        tokens_out: 0,
        duration: std::time::Duration::ZERO,
    }
}

fn stopped_or_failed(turn: &Turn, detail: &str) -> Outcome {
    if matches!(&turn.answer, Err((Failure::Cancelled, _))) {
        Outcome::Stopped
    } else {
        Outcome::Failed(cut(detail, SUMMARY_CUT))
    }
}

fn verdict(check: &Check, tree: &Worktree, piece: &Piece<'_>) -> BoatCheck {
    let sandbox = if piece.setup.sandbox_check {
        check::Sandbox::Bubblewrap { home: crate::platform::home_dir() }
    } else {
        check::Sandbox::None
    };
    match check::run(check, &tree.dir, &sandbox, CHECK_LIMIT, piece.setup.cancel) {
        Ok(result) => BoatCheck { passed: result.passed(), headline: result.headline(), output: result.output },
        Err(error) => {
            BoatCheck { passed: false, headline: "the check could not run".into(), output: error.to_string() }
        }
    }
}

/// A writing piece: the agent edits its worktree, the change is checked, scoped and committed.
fn write(piece: &Piece<'_>, state: &PieceState, own: Option<Worktree>) -> (Outcome, Option<Worktree>) {
    let branch = format!("clipier/{}/{}", piece.run, piece.id);
    let tree = if let Some(tree) = own {
        tree
    } else {
        let dir = piece.setup.layout.worktrees().join(&piece.run).join(piece.id);
        match piece.repo.add_worktree(&dir, &branch, &piece.integration_branch) {
            Ok(tree) => tree,
            Err(error) => return (Outcome::Failed(error.to_string()), None),
        }
    };
    let touches = state.piece.touches.join(", ");
    let (prompt, session) = match (&state.session, state.feedback.is_empty()) {
        (Some(session), false) => (
            format!("A reviewer looked at your work and asked for these changes:\n\n{}\n\nMake them.", state.feedback),
            Session::Resume(session.clone()),
        ),
        _ => (
            format!(
                "{}\n\n## Your piece: {}\n\n{}\n{}\n## Rules\n\nWork only inside this folder, and change only: {touches}. When \
                 done, reply with a short summary of what you changed. If you learn something about this project that a \
                 later agent could not see in the code, end your reply with a line `{NOTE_PREFIX} <short title> | <one \
                 or two sentences>`. Never put secrets in a note.\n{}",
                piece.role.prompt,
                state.piece.title,
                state.piece.brief,
                piece.before_it(state),
                piece.notes()
            ),
            Session::New,
        ),
    };
    let mut turn = piece.turn(&prompt, &session, &tree.dir, &format!("{}-{}", piece.id, state.attempts));
    let mut tokens = turn.effective_tokens();
    let mut duration_ms = u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX);
    let reply = match turn.answer.clone() {
        Ok(reply) => piece.learn(&reply),
        Err((_, detail)) => return (stopped_or_failed(&turn, &detail), Some(tree)),
    };
    let mut session_id = turn.session_id.clone().or_else(|| state.session.clone());
    let mut files = tree.changed_files(&piece.integration_branch).unwrap_or_default();
    match tree.out_of_scope(&piece.integration_branch, &state.piece.touches) {
        Ok(outside) if outside.is_empty() => {}
        Ok(outside) => {
            return (
                Outcome::Failed(format!("It wrote outside the files it may change: {}", outside.join(", "))),
                Some(tree),
            );
        }
        Err(error) => return (Outcome::Failed(error.to_string()), Some(tree)),
    }
    if files.is_empty() {
        return (Outcome::Failed(format!("It changed nothing. It said: {}", cut(&reply, SUMMARY_CUT))), Some(tree));
    }
    if let Err(error) = tree.commit(&format!("feat({}): {}", piece.id, cut(&state.piece.title, 50))) {
        return (Outcome::Failed(error.to_string()), Some(tree));
    }
    piece.set(|s| s.phase = Phase::Checking);
    let mut check_result = piece.setup.check.as_ref().map(|check| verdict(check, &tree, piece));
    if let (Some(failed), Some(session), Some(check)) =
        (check_result.as_ref().filter(|c| !c.passed), session_id.clone(), piece.setup.check.as_ref())
    {
        // One more try, in the same conversation, with what the check said.
        let again =
            format!("The project's check failed:\n\n{}\n\n{}\n\nFix it.", failed.headline, cut(&failed.output, 3_000));
        turn =
            piece.turn(&again, &Session::Resume(session), &tree.dir, &format!("{}-{}-fix", piece.id, state.attempts));
        tokens += turn.effective_tokens();
        duration_ms += u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX);
        if let Err((_, detail)) = &turn.answer {
            return (stopped_or_failed(&turn, detail), Some(tree));
        }
        session_id = turn.session_id.clone().or(session_id);
        files = tree.changed_files(&piece.integration_branch).unwrap_or(files);
        let _ = tree.commit(&format!("fix({}): make the check pass", piece.id));
        check_result = Some(verdict(check, &tree, piece));
    }
    let passed = check_result.as_ref().is_none_or(|c| c.passed);
    piece.set(|s| {
        s.summary = reply;
        s.files = files;
        s.tokens += tokens;
        s.duration_ms += duration_ms;
        s.session = session_id;
        s.check = check_result.clone();
        s.feedback.clear();
    });
    if passed {
        (Outcome::Ready, Some(tree))
    } else {
        let failure = check_result.map(|c| format!("{}\n{}", c.headline, cut(&c.output, 1_500))).unwrap_or_default();
        (Outcome::Failed(format!("The check failed after a second try: {failure}")), Some(tree))
    }
}

/// A reading piece: it reports, and when it reviews it says whether it approves.
fn read(piece: &Piece<'_>, state: &PieceState, reviewed: Option<&Worktree>) -> Outcome {
    let mut prompt =
        format!("{}\n\n## Your piece: {}\n\n{}\n", piece.role.prompt, state.piece.title, state.piece.brief);
    let cwd = match (&state.piece.reviews, reviewed) {
        (Some(_), Some(tree)) => {
            let change = piece.repo.change(&piece.integration_branch, &tree.branch, CHANGE_LIMIT).unwrap_or_default();
            let _ = write!(
                prompt,
                "\n## The change to look over\n\n```diff\n{change}\n```\n\nReply with `approved` on the first line if it is \
                 good. Otherwise reply with the changes you want, each one specific.\n"
            );
            tree.dir.clone()
        }
        (Some(_), None) => return Outcome::Failed("there is nothing to review".into()),
        (None, _) => piece.integration.dir.clone(),
    };
    prompt.push_str(&piece.before_it(state));
    let turn = piece.turn(&prompt, &Session::New, &cwd, &format!("{}-{}", piece.id, state.attempts));
    let reply = match turn.answer.clone() {
        Ok(reply) => reply,
        Err((_, detail)) => return stopped_or_failed(&turn, &detail),
    };
    piece.set(|s| {
        s.tokens += turn.effective_tokens();
        s.duration_ms += u64::try_from(turn.duration.as_millis()).unwrap_or(u64::MAX);
    });
    if state.piece.reviews.is_none() {
        piece.set(|s| s.summary = piece.learn(&reply));
        return Outcome::Ready;
    }
    let approved = reply
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim().to_lowercase().starts_with("approved"));
    Outcome::Reviewed { approved, notes: reply.trim().to_owned() }
}

/// Does the piece's work and says how it went. For a writing piece, the worktree it worked in comes
/// back too, for the loop to keep or remove.
pub(super) fn work(
    setup: &Setup<'_>,
    shared: &std::sync::Mutex<Crew>,
    repo: &Repo,
    integration: &Worktree,
    id: &str,
    own: Option<Worktree>,
    reviewed: Option<&Worktree>,
) -> (Outcome, Option<Worktree>) {
    let (run, integration_branch, repository, state) = {
        let crew = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            crew.id.clone(),
            crew.integration.clone(),
            std::path::PathBuf::from(&crew.repository),
            crew.piece(id).cloned().expect("the loop only starts pieces of the plan"),
        )
    };
    let Some(role) = setup.roles.get(&state.piece.role) else {
        return (Outcome::Failed(format!("no role `{}`", state.piece.role)), own);
    };
    update(setup.layout, shared, |crew| {
        if let Some(s) = crew.piece_mut(id) {
            s.attempts += 1;
        }
    });
    let state = PieceState { attempts: state.attempts + 1, ..state };
    let piece = Piece { setup, shared, repo, integration, id, role, run, integration_branch, repository };
    if role.mode == Mode::Write { write(&piece, &state, own) } else { (read(&piece, &state, reviewed), own) }
}
