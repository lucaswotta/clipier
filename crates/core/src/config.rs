//! The project's own settings: `.leme-pier/config.toml` in the repository, all optional.
//!
//! ```toml
//! check = "cargo test"       # what a piece must pass
//! captain = "captain"        # which role plans
//! approve_plan = true        # false: the crew starts as soon as the plan is valid
//! parallel = 3               # pieces that may run at the same time
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::crew::DEFAULT_PARALLEL;

/// Where a project keeps its settings and its roles.
pub fn project_folder(repository: &Path) -> PathBuf {
    repository.join(".leme-pier")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    /// The command that tells whether a piece is good; empty to detect it.
    pub check: String,
    pub captain: String,
    pub approve_plan: bool,
    pub parallel: usize,
}

impl Default for Project {
    fn default() -> Self {
        Self { check: String::new(), captain: "captain".into(), approve_plan: true, parallel: DEFAULT_PARALLEL }
    }
}

fn value(raw: &str) -> String {
    let raw = raw.split(" #").next().unwrap_or(raw).trim();
    raw.trim_matches(|c| c == '"' || c == '\'').to_owned()
}

/// Reads the settings. A line it does not know is reported, not ignored.
pub fn parse(text: &str) -> Result<Project, String> {
    let mut project = Project::default();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let (key, raw) = line.split_once('=').ok_or_else(|| format!("`{line}` is not `key = value`"))?;
        let raw = value(raw);
        match key.trim() {
            "check" => project.check = raw,
            "captain" => project.captain = raw,
            "approve_plan" => {
                project.approve_plan = match raw.as_str() {
                    "true" => true,
                    "false" => false,
                    other => return Err(format!("`approve_plan` is true or false, not `{other}`")),
                };
            }
            "parallel" => {
                project.parallel = raw
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=8).contains(n))
                    .ok_or_else(|| format!("`parallel` is a number from 1 to 8, not `{raw}`"))?;
            }
            other => return Err(format!("unknown setting `{other}`")),
        }
    }
    Ok(project)
}

/// The project's settings, or the defaults when there is no file.
pub fn load(repository: &Path) -> Result<Project, String> {
    match fs::read_to_string(project_folder(repository).join("config.toml")) {
        Ok(text) => parse(&text).map_err(|error| format!(".leme-pier/config.toml: {error}")),
        Err(_) => Ok(Project::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_are_read_with_comments_and_defaults_for_what_is_missing() {
        let text = "# the tests\ncheck = \"cargo test\"   # run in every piece\napprove_plan = false\nparallel = 2\n";
        let project = parse(text).unwrap();
        assert_eq!((project.check.as_str(), project.captain.as_str()), ("cargo test", "captain"));
        assert_eq!((project.approve_plan, project.parallel), (false, 2));
        assert_eq!(parse("").unwrap(), Project::default());
    }

    #[test]
    fn a_setting_that_is_wrong_says_which() {
        for (text, wanted) in [
            ("flavor = sweet", "unknown setting"),
            ("parallel = 99", "from 1 to 8"),
            ("approve_plan = maybe", "true or false"),
            ("check", "not `key = value`"),
        ] {
            assert!(parse(text).unwrap_err().contains(wanted), "{text}");
        }
    }

    #[test]
    fn a_project_without_a_file_gets_the_defaults() {
        assert_eq!(load(Path::new("/definitely/not/here")).unwrap(), Project::default());
    }
}
