//! What the interface can ask for. Each function calls the core and returns what it said.

use leme_core::agents::{self, Detected};
use leme_core::store::Layout;
use leme_core::usage::{Log, Summary};

/// The four agent CLIs as this machine has them. Takes a few seconds: it asks each CLI.
#[tauri::command]
pub async fn detect_agents() -> Vec<Detected> {
    tauri::async_runtime::spawn_blocking(agents::detect_all).await.expect("detecting agents does not panic")
}

/// What each agent has done through Leme, the busiest first.
#[tauri::command]
pub fn usage_summary() -> Vec<Summary> {
    Log::new(&Layout::standard().usage_file()).summary()
}
