//! Races: one brief, several boats. A race runs on its own thread; the screen polls the record.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use leme_core::agents::{Environment, Event};
use leme_core::check;
use leme_core::id::run_name;
use leme_core::platform::sandbox;
use leme_core::race::{self, BoatSpec, Race, Sail};
use leme_core::store::Layout;
use serde::Serialize;
use tauri::State;

use super::repository_path;

#[derive(Default)]
pub struct Races {
    cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

#[derive(Serialize)]
pub struct Live {
    boat: String,
    events: Vec<Event>,
}

#[derive(Serialize)]
pub struct View {
    race: Race,
    running: bool,
    live: Vec<Live>,
}

/// The project's test command, when the folder says which it is.
#[tauri::command]
pub fn race_check(repository: String) -> Result<String, String> {
    let path = repository_path(&repository)?;
    Ok(check::detect(&path).map(|check| check.display()).unwrap_or_default())
}

#[tauri::command]
pub async fn race_improve(repository: String, brief: String, agent: String, model: String) -> Result<String, String> {
    let path = repository_path(&repository)?;
    if brief.trim().is_empty() {
        return Err("write the brief first".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let out = Layout::standard().runs().join("improve");
        let spec = BoatSpec { agent, model };
        race::improve_brief(&path, &out, &brief, &spec, &Environment::current(), &AtomicBool::new(false))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn race_start(
    races: State<'_, Races>,
    repository: String,
    brief: String,
    boats: Vec<BoatSpec>,
    check: String,
) -> Result<String, String> {
    let path = repository_path(&repository)?;
    if brief.trim().is_empty() {
        return Err("write the brief first".into());
    }
    let id = run_name();
    let layout = Layout::standard();
    let race = Race::new(&id, &path, &brief, &check, &boats)?;
    race::save(&layout, &race)?;
    let shared = Arc::new(Mutex::new(race));
    let cancel = Arc::new(AtomicBool::new(false));
    races.cancels.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), Arc::clone(&cancel));
    let cancels = Arc::clone(&races.cancels);
    let run = id.clone();
    std::thread::spawn(move || {
        let environment = Environment::current();
        race::sail(
            &Sail { layout: &layout, environment: &environment, cancel: &cancel, sandbox_check: sandbox::available() },
            &shared,
        );
        cancels.lock().unwrap_or_else(PoisonError::into_inner).remove(&run);
    });
    Ok(id)
}

#[tauri::command]
pub fn race_list(races: State<'_, Races>) -> Vec<RaceSummary> {
    let running = races.cancels.lock().unwrap_or_else(PoisonError::into_inner);
    race::list(&Layout::standard()).iter().map(|race| summarize(race, running.contains_key(&race.id))).collect()
}

/// What the sidebar shows of a race.
#[derive(Serialize)]
pub struct RaceSummary {
    run: String,
    request: String,
    repository: String,
    started_ms: u64,
    /// `running`, `finished` or `interrupted`.
    phase: &'static str,
    done: usize,
    failed: usize,
}

fn summarize(race: &Race, running: bool) -> RaceSummary {
    use race::Phase;
    let count = |wanted: Phase| race.boats.iter().filter(|b| b.phase == wanted).count();
    RaceSummary {
        run: race.id.clone(),
        request: race.brief.clone(),
        repository: race.repository.clone(),
        started_ms: race.started_ms,
        phase: if race.finished {
            "finished"
        } else if running {
            "running"
        } else {
            "interrupted"
        },
        done: count(Phase::Done),
        failed: count(Phase::Failed) + count(Phase::Stopped),
    }
}

#[tauri::command]
pub fn race_view(races: State<'_, Races>, id: String) -> Result<View, String> {
    let layout = Layout::standard();
    let running = races.cancels.lock().unwrap_or_else(PoisonError::into_inner).contains_key(&id);
    let race = race::load(&layout, &id)?;
    let live = race
        .boats
        .iter()
        .filter(|boat| matches!(boat.phase, race::Phase::Working | race::Phase::Checking))
        .map(|boat| Live { boat: boat.id.clone(), events: race::progress(&layout, &id, boat) })
        .collect();
    Ok(View { race, running, live })
}

#[tauri::command]
pub fn race_diff(id: String, boat: String) -> Result<String, String> {
    race::diff(&race::load(&Layout::standard(), &id)?, &boat)
}

#[tauri::command]
pub fn race_cancel(races: State<'_, Races>, id: String) {
    if let Some(cancel) = races.cancels.lock().unwrap_or_else(PoisonError::into_inner).get(&id) {
        cancel.store(true, Ordering::Relaxed);
    }
}
