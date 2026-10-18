//! Roles, the project's settings and its notes.

use leme_core::agents::Agent;
use leme_core::config::{self, Project};
use leme_core::memory::{Memory, Stored};
use leme_core::roles::{self, Assignment, Fallback, Role, Source};
use leme_core::store::{self, Layout};
use serde::Serialize;
use tauri::State;

use super::{Agents, repository_path};

#[derive(Serialize)]
pub struct RoleView {
    #[serde(flatten)]
    role: Role,
    /// Who the role goes to on this machine, or `None` when no agent can do it.
    assigned: Option<Assignment>,
}

#[derive(Serialize)]
pub struct RolesView {
    roles: Vec<RoleView>,
    /// Role files that could not be read.
    problems: Vec<String>,
}

fn folders(repository: Option<&str>) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let project = match repository.filter(|text| !text.trim().is_empty()) {
        Some(text) => config::project_folder(&repository_path(text)?).join("roles"),
        None => std::path::PathBuf::new(),
    };
    Ok((store::user_roles(), project))
}

/// Every role as it applies here: built-in, the user's and the project's, and who each goes to.
#[tauri::command]
pub fn roles_list(agents: State<'_, Agents>, repository: Option<String>) -> Result<RolesView, String> {
    let (user, project) = folders(repository.as_deref())?;
    let loaded = roles::load(&user, &project);
    let available = agents.available();
    let roles = loaded.list.into_iter().map(|role| RoleView { assigned: role.assign(&available), role }).collect();
    Ok(RolesView { roles, problems: loaded.problems })
}

/// Saves a choice of agent and model for a role in the user's own folder, keeping the rest of the
/// role as it is.
#[tauri::command]
pub fn role_save(
    name: String,
    agent: String,
    model: String,
    fallback_agent: String,
    fallback_model: String,
) -> Result<(), String> {
    if !agent.is_empty() && Agent::from_name(&agent).is_none() {
        return Err(format!("unknown agent `{agent}`"));
    }
    let (user, _) = folders(None)?;
    let loaded = roles::load(&user, std::path::Path::new(""));
    let mut role = loaded.get(&name).cloned().ok_or_else(|| format!("no role called `{name}`"))?;
    role.agent = agent;
    role.model = model;
    role.fallback = (!fallback_agent.is_empty()).then_some(Fallback { agent: fallback_agent, model: fallback_model });
    role.source = Source::User;
    roles::save(&user, &role).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn project_settings(repository: String) -> Result<Project, String> {
    config::load(&repository_path(&repository)?)
}

/// The project's notes, which every agent that works on it reads.
#[tauri::command]
pub fn notes_list(repository: String) -> Result<Vec<Stored>, String> {
    Ok(Memory::open(&Layout::standard().memory(), &repository_path(&repository)?).notes())
}

#[tauri::command]
pub fn note_forget(repository: String, id: String) -> Result<(), String> {
    if Memory::open(&Layout::standard().memory(), &repository_path(&repository)?).forget(&id) {
        Ok(())
    } else {
        Err("that note is already gone".into())
    }
}
