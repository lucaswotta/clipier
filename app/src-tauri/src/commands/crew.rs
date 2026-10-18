//! Crews: the captain draws a plan, the user reads it, and the crew carries it out on its own
//! thread while the screen polls the record.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use leme_core::agents::{Environment, Event};
use leme_core::check::{self, Check};
use leme_core::config;
use leme_core::crew::{self, Command, Crew, PieceState, Setup};
use leme_core::id::{run_name, uuid};
use leme_core::memory::{Memory, Stored};
use leme_core::plan::{Piece, Plan};
use leme_core::platform::sandbox;
use leme_core::roles::{self, Roles};
use leme_core::store::{self, Layout};
use serde::{Deserialize, Serialize};
use tauri::State;

use super::{Agents, repository_path};

struct Pending {
    plan: Plan,
    repository: PathBuf,
    brief: String,
}

struct Active {
    commands: Sender<Command>,
}

#[derive(Default)]
pub struct Crews {
    plans: Mutex<HashMap<String, Pending>>,
    /// The stop switch of each plan that is waiting for the captain.
    planning: Mutex<HashMap<String, Arc<AtomicBool>>>,
    active: Arc<Mutex<HashMap<String, Active>>>,
}

#[derive(Serialize)]
pub struct PieceView {
    #[serde(flatten)]
    piece: Piece,
    role_title: String,
    agent: String,
    model: String,
}

#[derive(Serialize)]
pub struct PlanView {
    id: String,
    summary: String,
    pieces: Vec<PieceView>,
    /// The check the project names, else the one detected, for the user to confirm or change.
    check: String,
    approve_plan: bool,
    notes: Vec<Stored>,
}

fn roles_for(repository: &Path) -> Roles {
    roles::load(&store::user_roles(), &config::project_folder(repository).join("roles"))
}

/// Asks the captain for a plan. Takes as long as the captain does; nothing runs yet.
#[tauri::command]
pub async fn crew_plan(
    agents: State<'_, Agents>,
    crews: State<'_, Crews>,
    request_id: String,
    repository: String,
    brief: String,
) -> Result<PlanView, String> {
    let path = repository_path(&repository)?;
    if brief.trim().is_empty() {
        return Err("write the brief first".into());
    }
    let project = config::load(&path)?;
    let cancel = Arc::new(AtomicBool::new(false));
    crews.planning.lock().unwrap_or_else(PoisonError::into_inner).insert(request_id.clone(), Arc::clone(&cancel));
    let available = agents.available();
    let (worker_path, worker_brief) = (path.clone(), brief.clone());
    let drawn = tauri::async_runtime::spawn_blocking(move || {
        let layout = Layout::standard();
        let roles = roles_for(&worker_path);
        let environment = Environment::current();
        let setup = Setup {
            layout: &layout,
            environment: &environment,
            cancel: &cancel,
            roles: &roles,
            available: &available,
            check: None,
            parallel: project.parallel,
            sandbox_check: false,
        };
        let out = layout.runs().join("planning").join(uuid());
        let drawn = crew::draw_plan(&setup, &worker_path, &worker_brief, &out)?;
        let pieces = drawn
            .plan
            .pieces
            .iter()
            .map(|piece| {
                let role = roles.get(&piece.role);
                let assigned = role.and_then(|role| role.assign(&available));
                PieceView {
                    piece: piece.clone(),
                    role_title: role.map(|r| r.title.clone()).unwrap_or_default(),
                    agent: assigned.as_ref().map(|a| a.agent.name().to_owned()).unwrap_or_default(),
                    model: assigned.map(|a| a.model).unwrap_or_default(),
                }
            })
            .collect();
        Ok::<_, String>((drawn.plan, pieces, project))
    })
    .await
    .map_err(|e| e.to_string());
    crews.planning.lock().unwrap_or_else(PoisonError::into_inner).remove(&request_id);
    let (plan, pieces, project) = drawn??;
    let id = uuid();
    let check = if project.check.is_empty() {
        check::detect(&path).map(|c| c.display()).unwrap_or_default()
    } else {
        project.check.clone()
    };
    let view = PlanView {
        id: id.clone(),
        summary: plan.summary.clone(),
        pieces,
        check,
        approve_plan: project.approve_plan,
        notes: Memory::open(&Layout::standard().memory(), &path).notes(),
    };
    crews.plans.lock().unwrap_or_else(PoisonError::into_inner).insert(id, Pending { plan, repository: path, brief });
    Ok(view)
}

/// Stops a plan request that is waiting for the captain.
#[tauri::command]
pub fn crew_stop_planning(crews: State<'_, Crews>, request_id: String) {
    if let Some(cancel) = crews.planning.lock().unwrap_or_else(PoisonError::into_inner).get(&request_id) {
        cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts the crew on a plan the user approved. Returns the run id.
#[tauri::command]
pub fn crew_start(
    agents: State<'_, Agents>,
    crews: State<'_, Crews>,
    plan_id: String,
    check: String,
) -> Result<String, String> {
    let parsed = if check.trim().is_empty() { None } else { Some(Check::parse(&check)?) };
    let pending = crews
        .plans
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&plan_id)
        .ok_or("that plan is no longer available")?;
    let project = config::load(&pending.repository)?;
    let available = agents.available();
    let id = run_name();
    let layout = Layout::standard();
    let roles = roles_for(&pending.repository);
    let cancel = Arc::new(AtomicBool::new(false));
    let environment = Environment::current();
    let crew = {
        let setup = Setup {
            layout: &layout,
            environment: &environment,
            cancel: &cancel,
            roles: &roles,
            available: &available,
            check: None,
            parallel: project.parallel,
            sandbox_check: false,
        };
        Crew::new(&id, &pending.repository, &pending.brief, &check, &pending.plan, &setup)?
    };
    crew::save(&layout, &crew)?;
    let shared = Arc::new(Mutex::new(crew));
    let (sender, commands) = mpsc::channel();
    crews.active.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), Active { commands: sender });
    let active = Arc::clone(&crews.active);
    let run = id.clone();
    std::thread::spawn(move || {
        let setup = Setup {
            layout: &layout,
            environment: &environment,
            cancel: &cancel,
            roles: &roles,
            available: &available,
            check: parsed,
            parallel: project.parallel,
            sandbox_check: sandbox::available(),
        };
        crew::carry_out(&setup, &shared, &commands);
        active.lock().unwrap_or_else(PoisonError::into_inner).remove(&run);
    });
    Ok(id)
}

/// What the sidebar shows of a crew.
#[derive(Serialize)]
pub struct CrewSummary {
    run: String,
    request: String,
    repository: String,
    started_ms: u64,
    /// `running`, `finished` or `interrupted`.
    phase: &'static str,
    done: usize,
    failed: usize,
}

#[tauri::command]
pub fn crew_list(crews: State<'_, Crews>) -> Vec<CrewSummary> {
    use leme_core::crew::Phase;
    let running = crews.active.lock().unwrap_or_else(PoisonError::into_inner);
    crew::list(&Layout::standard())
        .iter()
        .map(|crew| {
            let count = |wanted: &[Phase]| crew.pieces.iter().filter(|p| wanted.contains(&p.phase)).count();
            CrewSummary {
                run: crew.id.clone(),
                request: crew.brief.clone(),
                repository: crew.repository.clone(),
                started_ms: crew.started_ms,
                phase: if crew.finished {
                    "finished"
                } else if running.contains_key(&crew.id) {
                    "running"
                } else {
                    "interrupted"
                },
                done: count(&[Phase::Merged]),
                failed: count(&[Phase::Failed, Phase::Conflict, Phase::Rejected, Phase::Skipped, Phase::Stopped]),
            }
        })
        .collect()
}

#[derive(Serialize)]
pub struct Live {
    piece: String,
    events: Vec<Event>,
}

#[derive(Serialize)]
pub struct View {
    crew: Crew,
    running: bool,
    live: Vec<Live>,
}

#[tauri::command]
pub fn crew_view(crews: State<'_, Crews>, id: String) -> Result<View, String> {
    use leme_core::crew::Phase;
    let layout = Layout::standard();
    let running = crews.active.lock().unwrap_or_else(PoisonError::into_inner).contains_key(&id);
    let crew = crew::load(&layout, &id)?;
    let live = crew
        .pieces
        .iter()
        .filter(|p: &&PieceState| matches!(p.phase, Phase::Working | Phase::Checking))
        .map(|p| Live { piece: p.piece.id.clone(), events: crew::progress(&layout, &id, p) })
        .collect();
    Ok(View { crew, running, live })
}

/// What the user decided about a piece that asked.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserCommand {
    Redo { piece: String },
    Skip { piece: String },
    Stop,
}

#[tauri::command]
pub fn crew_command(crews: State<'_, Crews>, id: String, command: UserCommand) -> Result<(), String> {
    let active = crews.active.lock().unwrap_or_else(PoisonError::into_inner);
    let running = active.get(&id).ok_or("this crew is not running")?;
    let command = match command {
        UserCommand::Redo { piece } => Command::Redo(piece),
        UserCommand::Skip { piece } => Command::Skip(piece),
        UserCommand::Stop => Command::Stop,
    };
    running.commands.send(command).map_err(|_| "this crew has finished".to_owned())
}
