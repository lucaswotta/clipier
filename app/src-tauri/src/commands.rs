//! What the interface can ask for. Each function calls the core and returns what it said.

pub mod chat;
pub mod crew;
pub mod github;
pub mod race;
pub mod roles;

pub use chat::Chats;
pub use crew::Crews;
pub use race::Races;

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use clipier_core::agents::{self, Detected};
use clipier_core::store::Layout;
use clipier_core::usage::{Log, Summary};

/// The agents as the last detection found them, so that a crew does not wait four seconds to ask.
#[derive(Default)]
pub struct Agents {
    found: Mutex<Vec<Detected>>,
}

impl Agents {
    fn lock(&self) -> MutexGuard<'_, Vec<Detected>> {
        self.found.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The last detection, or a new one when there is none.
    pub fn available(&self) -> Vec<Detected> {
        let known = self.lock().clone();
        if !known.is_empty() {
            return known;
        }
        let found = agents::detect_all();
        self.lock().clone_from(&found);
        found
    }
}

/// An absolute path to a folder, or why it is not one.
fn repository_path(text: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(text.trim());
    if !path.is_absolute() {
        return Err("the repository must be an absolute path".into());
    }
    if !path.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    Ok(path)
}

/// The four agent CLIs as this machine has them. Takes a few seconds: it asks each CLI.
#[tauri::command]
pub async fn detect_agents(agents: tauri::State<'_, Agents>) -> Result<Vec<Detected>, String> {
    let found = tauri::async_runtime::spawn_blocking(agents::detect_all).await.map_err(|e| e.to_string())?;
    agents.lock().clone_from(&found);
    Ok(found)
}

/// What each agent has done through Clipier, the busiest first.
#[tauri::command]
pub fn usage_summary() -> Vec<Summary> {
    Log::new(&Layout::standard().usage_file()).summary()
}
