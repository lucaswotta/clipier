//! Conversations: a turn runs on its own thread, and the screen polls for the record and for what
//! the running agent has printed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use leme_core::agents::{Environment, Event};
use leme_core::id::run_name;
use leme_core::session::{Choice, Entry, Meta, Store};
use leme_core::store::Layout;
use serde::Serialize;
use tauri::State;

use super::repository_path;

struct Turn {
    cancel: Arc<AtomicBool>,
    agent: String,
    model: String,
    /// The output file of the turn, once it has started.
    label: Arc<Mutex<Option<String>>>,
}

#[derive(Default)]
pub struct Chats {
    turns: Arc<Mutex<HashMap<String, Turn>>>,
}

fn store() -> Store {
    Store::new(&Layout::standard())
}

#[derive(Serialize)]
pub struct Summary {
    #[serde(flatten)]
    meta: Meta,
    busy: bool,
}

#[derive(Serialize)]
pub struct Working {
    agent: String,
    model: String,
    events: Vec<Event>,
}

#[derive(Serialize)]
pub struct View {
    meta: Meta,
    /// How many entries the record has in all; the next poll asks from here.
    total: usize,
    entries: Vec<Entry>,
    working: Option<Working>,
}

#[tauri::command]
pub fn chat_list(chats: State<'_, Chats>) -> Vec<Summary> {
    let turns = chats.turns.lock().unwrap_or_else(PoisonError::into_inner);
    store().list().into_iter().map(|meta| Summary { busy: turns.contains_key(&meta.id), meta }).collect()
}

#[tauri::command]
pub fn chat_create(repository: String) -> Result<String, String> {
    let path = repository_path(&repository)?;
    let id = run_name();
    store().create(&id, &path.to_string_lossy())?;
    Ok(id)
}

#[tauri::command]
pub fn chat_view(chats: State<'_, Chats>, id: String, since: usize) -> Result<View, String> {
    let store = store();
    let meta = store.meta(&id)?;
    let log = store.log(&id)?;
    let total = log.len();
    let working = chats.turns.lock().unwrap_or_else(PoisonError::into_inner).get(&id).map(|turn| {
        let label = turn.label.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let events = label.map(|label| store.progress(&id, &label, &turn.agent)).unwrap_or_default();
        Working { agent: turn.agent.clone(), model: turn.model.clone(), events }
    });
    Ok(View { meta, total, entries: log.into_iter().skip(since).collect(), working })
}

#[tauri::command]
pub fn chat_send(
    chats: State<'_, Chats>,
    id: String,
    text: String,
    agent: String,
    model: String,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("write something to send".into());
    }
    let store = store();
    store.meta(&id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let label = Arc::new(Mutex::new(None));
    {
        let mut turns = chats.turns.lock().unwrap_or_else(PoisonError::into_inner);
        if turns.contains_key(&id) {
            return Err("an agent is still answering in this conversation".into());
        }
        turns.insert(
            id.clone(),
            Turn { cancel: Arc::clone(&cancel), agent: agent.clone(), model: model.clone(), label: Arc::clone(&label) },
        );
    }
    let turns = Arc::clone(&chats.turns);
    std::thread::spawn(move || {
        let choice = Choice { agent, model };
        let started = |name: &str| *label.lock().unwrap_or_else(PoisonError::into_inner) = Some(name.to_owned());
        if let Err(error) = store.take_turn(&id, &text, &choice, &Environment::current(), &cancel, &started) {
            store.fail(&id, &choice, &error);
        }
        turns.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
    });
    Ok(())
}

#[tauri::command]
pub fn chat_cancel(chats: State<'_, Chats>, id: String) {
    if let Some(turn) = chats.turns.lock().unwrap_or_else(PoisonError::into_inner).get(&id) {
        turn.cancel.store(true, Ordering::Relaxed);
    }
}
