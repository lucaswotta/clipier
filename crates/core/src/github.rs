//! The one thing Leme Pier asks of GitHub: to open a pull request from a branch of its own, when the
//! user clicks for it. Signing in uses GitHub's device flow, which needs no secret in this program.
//! It makes no other network request, and sends no analytics.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::platform;
use crate::runner::capture;

const TIMEOUT: Duration = Duration::from_secs(20);
const USER_AGENT: &str = concat!("leme-pier/", env!("CARGO_PKG_VERSION"));
const API_VERSION: &str = "2022-11-28";

/// Where GitHub is. Tests point it at a server of their own.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub web: String,
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { web: "https://github.com".into(), api: "https://api.github.com".into() }
    }
}

/// What the user must do to sign in: open the address and type the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds between two asks.
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user has not finished yet.
    Pending,
    /// Ask less often.
    SlowDown,
    Granted(String),
    Denied(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequest {
    pub url: String,
    pub number: u64,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(TIMEOUT)).build().into()
}

fn describe(error: &ureq::Error) -> String {
    format!("could not reach GitHub: {error}")
}

/// `owner` and `repo` of a GitHub remote, in any of the shapes git writes it.
pub fn remote_slug(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let path =
        ["https://github.com/", "http://github.com/", "git@github.com:", "ssh://git@github.com/", "git://github.com/"]
            .iter()
            .find_map(|prefix| url.strip_prefix(prefix))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then(|| (owner.to_owned(), repo.to_owned()))
}

/// Asks GitHub for a code the user will type at `verification_uri`.
pub fn start_device_flow(endpoints: &Endpoints, client_id: &str) -> Result<DeviceCode, String> {
    let mut response = agent()
        .post(format!("{}/login/device/code", endpoints.web))
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .send_form([("client_id", client_id), ("scope", "repo")])
        .map_err(|e| describe(&e))?;
    let body: Value = response.body_mut().read_json().map_err(|e| describe(&e))?;
    serde_json::from_value(body.clone()).map_err(|_| {
        body["error_description"].as_str().map_or_else(|| "GitHub did not start the sign-in".to_owned(), str::to_owned)
    })
}

/// One ask of whether the user has finished signing in.
pub fn poll_device_flow(endpoints: &Endpoints, client_id: &str, device_code: &str) -> Result<Poll, String> {
    let mut response = agent()
        .post(format!("{}/login/oauth/access_token", endpoints.web))
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .send_form([
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .map_err(|e| describe(&e))?;
    let body: Value = response.body_mut().read_json().map_err(|e| describe(&e))?;
    if let Some(token) = body["access_token"].as_str() {
        return Ok(Poll::Granted(token.to_owned()));
    }
    Ok(match body["error"].as_str() {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::SlowDown,
        Some("expired_token") => Poll::Denied("the code expired; start again".into()),
        Some("access_denied") => Poll::Denied("you declined the sign-in".into()),
        other => Poll::Denied(body["error_description"].as_str().or(other).unwrap_or("GitHub refused").to_owned()),
    })
}

/// Opens a pull request from `head` into `base`, or returns the one already open for that branch.
#[allow(clippy::too_many_arguments, reason = "each one is a different part of the request")]
pub fn open_pull_request(
    endpoints: &Endpoints,
    token: &str,
    owner: &str,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
) -> Result<PullRequest, String> {
    let api = agent();
    let authorize = |request: ureq::RequestBuilder<ureq::typestate::WithBody>| {
        request
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("User-Agent", USER_AGENT)
    };
    let mut response = authorize(api.post(format!("{}/repos/{owner}/{repo}/pulls", endpoints.api)))
        .send_json(serde_json::json!({ "title": title, "head": head, "base": base, "body": body }))
        .map_err(|e| describe(&e))?;
    let status = response.status().as_u16();
    let reply: Value = response.body_mut().read_json().unwrap_or(Value::Null);
    if status == 201 {
        return from_reply(&reply);
    }
    let message = reply["message"].as_str().unwrap_or("GitHub refused the pull request");
    let detail = reply["errors"][0]["message"].as_str().unwrap_or_default();
    if status == 422 && detail.contains("already exists") {
        return existing(endpoints, token, owner, repo, head);
    }
    Err(match status {
        401 => "GitHub did not accept the sign-in; connect again".to_owned(),
        403 | 404 => format!("GitHub says: {message}. The sign-in may lack access to {owner}/{repo}"),
        _ => format!("GitHub says: {message} {detail}").trim().to_owned(),
    })
}

fn from_reply(reply: &Value) -> Result<PullRequest, String> {
    let url = reply["html_url"].as_str().ok_or("GitHub's answer had no address")?;
    Ok(PullRequest { url: url.to_owned(), number: reply["number"].as_u64().unwrap_or_default() })
}

fn existing(endpoints: &Endpoints, token: &str, owner: &str, repo: &str, head: &str) -> Result<PullRequest, String> {
    let mut response = agent()
        .get(format!("{}/repos/{owner}/{repo}/pulls?state=open&head={owner}:{head}", endpoints.api))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| describe(&e))?;
    let reply: Value = response.body_mut().read_json().map_err(|e| describe(&e))?;
    from_reply(&reply[0]).map_err(|_| "a pull request already exists for this branch".to_owned())
}

/// The file that holds the token the device flow gave.
fn token_file() -> PathBuf {
    crate::store::config_folder().join("github-token")
}

pub fn stored_token() -> Option<String> {
    fs::read_to_string(token_file()).ok().map(|text| text.trim().to_owned()).filter(|token| !token.is_empty())
}

pub fn save_token(token: &str) -> std::io::Result<()> {
    let file = token_file();
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    fs::write(&file, token)?;
    platform::keep_private(&file)
}

pub fn forget_token() {
    let _ = fs::remove_file(token_file());
}

/// The token of the user's own `gh` CLI, when it is installed and signed in: no setup needed.
pub fn gh_token() -> Option<String> {
    let program = platform::find_program("gh", &platform::path_folders())?;
    let answer = capture(&program, &["auth", "token"], Duration::from_secs(10))?;
    answer.success.then(|| answer.stdout.trim().to_owned()).filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// A server that answers each request in turn with the next canned `(status, body)` and remembers
    /// what it was asked.
    fn serve(answers: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for (status, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    request.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&request).into_owned();
                    let Some((head, rest)) = text.split_once("\r\n\r\n") else { continue };
                    let wanted = head
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if rest.len() >= wanted || n == 0 {
                        break;
                    }
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&request).into_owned());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (address, seen)
    }

    fn endpoints(address: &str) -> Endpoints {
        Endpoints { web: address.to_owned(), api: address.to_owned() }
    }

    #[test]
    fn a_github_remote_is_read_in_every_shape_git_writes_it() {
        for url in [
            "https://github.com/lucaswotta/leme-pier.git",
            "https://github.com/lucaswotta/leme-pier",
            "git@github.com:lucaswotta/leme-pier.git",
            "ssh://git@github.com/lucaswotta/leme-pier.git",
        ] {
            assert_eq!(remote_slug(url), Some(("lucaswotta".into(), "leme-pier".into())), "{url}");
        }
        assert_eq!(remote_slug("https://gitlab.com/a/b.git"), None);
        assert_eq!(remote_slug("git@github.com:onlyowner"), None);
    }

    #[test]
    fn signing_in_with_the_device_flow_goes_from_a_code_to_a_token() {
        let (address, seen) = serve(vec![
            (
                200,
                r#"{"device_code":"dc","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","interval":5,"expires_in":900}"#,
            ),
            (200, r#"{"error":"authorization_pending"}"#),
            (200, r#"{"error":"slow_down"}"#),
            (200, r#"{"access_token":"gho_secret","token_type":"bearer"}"#),
            (200, r#"{"error":"access_denied"}"#),
        ]);
        let endpoints = endpoints(&address);
        let code = start_device_flow(&endpoints, "client").unwrap();
        assert_eq!((code.user_code.as_str(), code.interval), ("ABCD-1234", 5));
        assert_eq!(poll_device_flow(&endpoints, "client", &code.device_code).unwrap(), Poll::Pending);
        assert_eq!(poll_device_flow(&endpoints, "client", &code.device_code).unwrap(), Poll::SlowDown);
        assert_eq!(
            poll_device_flow(&endpoints, "client", &code.device_code).unwrap(),
            Poll::Granted("gho_secret".into())
        );
        assert!(
            matches!(poll_device_flow(&endpoints, "client", "dc").unwrap(), Poll::Denied(why) if why.contains("declined"))
        );
        let first = seen.lock().unwrap()[0].clone();
        assert!(first.starts_with("POST /login/device/code") && first.contains("client_id=client"), "{first}");
    }

    #[test]
    fn a_pull_request_is_opened_with_the_token_and_an_existing_one_is_returned_instead_of_an_error() {
        let (address, seen) = serve(vec![
            (201, r#"{"html_url":"https://github.com/o/r/pull/7","number":7}"#),
            (
                422,
                r#"{"message":"Validation Failed","errors":[{"message":"A pull request already exists for o:leme/x"}]}"#,
            ),
            (200, r#"[{"html_url":"https://github.com/o/r/pull/3","number":3}]"#),
            (401, r#"{"message":"Bad credentials"}"#),
        ]);
        let endpoints = endpoints(&address);
        let opened = open_pull_request(&endpoints, "tok", "o", "r", "leme/x", "main", "Title", "Body").unwrap();
        assert_eq!(opened, PullRequest { url: "https://github.com/o/r/pull/7".into(), number: 7 });
        let request = seen.lock().unwrap()[0].clone();
        assert!(
            request.starts_with("POST /repos/o/r/pulls")
                && request.to_lowercase().contains("authorization: bearer tok"),
            "{request}"
        );
        let compact = request.replace([' ', '\n'], "");
        assert!(compact.contains(r#""head":"leme/x""#) && compact.contains(r#""base":"main""#), "{request}");

        let again = open_pull_request(&endpoints, "tok", "o", "r", "leme/x", "main", "Title", "Body").unwrap();
        assert_eq!(again.number, 3);
        let rejected = open_pull_request(&endpoints, "bad", "o", "r", "leme/x", "main", "T", "B").unwrap_err();
        assert!(rejected.contains("connect again"), "{rejected}");
    }

    #[test]
    fn nothing_is_sent_when_github_cannot_be_reached() {
        let closed = Endpoints { web: "http://127.0.0.1:1".into(), api: "http://127.0.0.1:1".into() };
        assert!(
            open_pull_request(&closed, "t", "o", "r", "h", "b", "t", "b")
                .unwrap_err()
                .contains("could not reach GitHub")
        );
    }

    /// Opens a real pull request with the token of the user's `gh`; run by hand with
    /// `LEME_PR_REPO=<folder> LEME_PR_BRANCH=<branch> LEME_PR_BASE=main cargo test -p leme-core -- --ignored real_pull_request`.
    #[test]
    #[ignore = "pushes a branch and opens a pull request on GitHub"]
    fn real_pull_request() {
        let (Ok(folder), Ok(branch), Ok(base)) =
            (std::env::var("LEME_PR_REPO"), std::env::var("LEME_PR_BRANCH"), std::env::var("LEME_PR_BASE"))
        else {
            panic!("set LEME_PR_REPO, LEME_PR_BRANCH and LEME_PR_BASE");
        };
        let repo = crate::git::Repo::open(std::path::Path::new(&folder)).unwrap();
        let (owner, name) = remote_slug(&repo.remote_url("origin").unwrap()).unwrap();
        let token = gh_token().expect("gh is signed in");
        repo.push(&branch).unwrap();
        let opened = open_pull_request(
            &Endpoints::default(),
            &token,
            &owner,
            &name,
            &branch,
            &base,
            "Self test: please close",
            "Opened by a test of Leme Pier.",
        )
        .unwrap();
        println!("OPENED {}", opened.url);
        let again = open_pull_request(&Endpoints::default(), &token, &owner, &name, &branch, &base, "x", "x").unwrap();
        assert_eq!(again.number, opened.number, "asking twice returns the same pull request");
    }
}
