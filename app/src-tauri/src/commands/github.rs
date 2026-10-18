//! Opening a pull request: signing in to GitHub and sending a branch of Leme's own.

use leme_core::git::Repo;
use leme_core::github::{self, DeviceCode, Endpoints, Poll, PullRequest};
use serde::Serialize;

use super::repository_path;

/// The OAuth App the user registered for Leme Pier, from the environment or built in at compile time.
/// Without one, the device flow is not offered; a `gh` sign-in still works.
fn client_id() -> Option<String> {
    std::env::var("LEME_PIER_GITHUB_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("LEME_PIER_GITHUB_CLIENT_ID").map(str::to_owned))
        .filter(|id| !id.trim().is_empty())
}

#[derive(Serialize)]
pub struct Status {
    connected: bool,
    /// `stored` (the device flow), `gh` (the user's own GitHub CLI) or `none`.
    source: &'static str,
    /// Whether the device flow can be started.
    can_connect: bool,
}

#[tauri::command]
pub async fn github_status() -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (connected, source) = if github::stored_token().is_some() {
            (true, "stored")
        } else if github::gh_token().is_some() {
            (true, "gh")
        } else {
            (false, "none")
        };
        Status { connected, source, can_connect: client_id().is_some() }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn github_connect_start() -> Result<DeviceCode, String> {
    let id = client_id().ok_or("no OAuth App is set up; see the README")?;
    tauri::async_runtime::spawn_blocking(move || github::start_device_flow(&Endpoints::default(), &id))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct Progress {
    /// `pending`, `granted` or `denied`.
    status: &'static str,
    message: String,
}

#[tauri::command]
pub async fn github_connect_poll(device_code: String) -> Result<Progress, String> {
    let id = client_id().ok_or("no OAuth App is set up; see the README")?;
    tauri::async_runtime::spawn_blocking(move || {
        Ok(match github::poll_device_flow(&Endpoints::default(), &id, &device_code)? {
            Poll::Pending | Poll::SlowDown => Progress { status: "pending", message: String::new() },
            Poll::Granted(token) => {
                github::save_token(&token).map_err(|e| e.to_string())?;
                Progress { status: "granted", message: String::new() }
            }
            Poll::Denied(why) => Progress { status: "denied", message: why },
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn github_disconnect() {
    github::forget_token();
}

/// Pushes `branch` to origin and opens a pull request into `base`. Only a branch Leme made may be sent.
#[tauri::command]
pub async fn github_open_pr(
    repository: String,
    branch: String,
    base: String,
    title: String,
    body: String,
) -> Result<PullRequest, String> {
    let path = repository_path(&repository)?;
    if !branch.starts_with("leme/") {
        return Err("only a branch Leme Pier made can be sent".into());
    }
    if base.is_empty() || base == "HEAD" {
        return Err("the checkout was not on a branch, so there is nothing to open the pull request into".into());
    }
    if title.trim().is_empty() {
        return Err("give the pull request a title".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let repo = Repo::open(&path).map_err(|e| e.to_string())?;
        let remote = repo.remote_url("origin").ok_or("this repository has no remote called origin")?;
        let (owner, name) = github::remote_slug(&remote).ok_or("origin is not a GitHub repository")?;
        let token = github::stored_token().or_else(github::gh_token).ok_or("connect GitHub first")?;
        repo.push(&branch).map_err(|e| e.to_string())?;
        github::open_pull_request(&Endpoints::default(), &token, &owner, &name, &branch, &base, title.trim(), &body)
    })
    .await
    .map_err(|e| e.to_string())?
}
