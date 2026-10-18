//! The only module that tells one operating system from another. Nothing else may use
//! `cfg(target_os)` or `std::os::unix`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

pub mod sandbox;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

/// Makes `command` lead its own process group, so that [`kill_tree`] can end it together with
/// everything it starts.
pub fn lead_group(command: &mut Command) {
    os::lead_group(command);
}

/// Ends the process and everything it started, then collects it.
pub fn kill_tree(child: &mut Child) {
    os::kill_tree(child);
}

/// Starts the command, trying again for a moment when the system says the program is busy: a CLI
/// that updates itself is briefly open for writing, and running it then fails.
pub fn start(command: &mut Command) -> std::io::Result<Child> {
    os::start(command)
}

/// Makes a file readable by its owner only, for a secret such as a token. Windows keeps a user's
/// profile folder private already, so there it does nothing.
pub fn keep_private(path: &Path) -> std::io::Result<()> {
    os::keep_private(path)
}

/// Where the user's home is.
pub fn home_dir() -> PathBuf {
    os::home_dir()
}

/// Where the app keeps what it writes (conversations, runs, worktrees, notes).
pub fn data_dir() -> PathBuf {
    os::data_dir()
}

/// Where the user's settings and roles live.
pub fn config_dir() -> PathBuf {
    os::config_dir()
}

/// The first runnable file called `name` in `folders`, in order. On Windows the usual executable
/// extensions are tried too, because a CLI installed with npm is a `.cmd` file there.
pub fn find_program(name: &str, folders: &[PathBuf]) -> Option<PathBuf> {
    folders.iter().find_map(|folder| os::runnable(folder, name))
}

/// The folders of the `PATH` variable, in order.
pub fn path_folders() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|paths| std::env::split_paths(&paths).collect()).unwrap_or_default()
}

/// Whether `path` names a file that can be run.
pub fn is_runnable(path: &Path) -> bool {
    path.parent()
        .zip(path.file_name())
        .is_some_and(|(folder, name)| os::runnable(folder, &name.to_string_lossy()).is_some())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_found_in_the_first_folder_that_has_it() {
        let found = find_program("sh", &[PathBuf::from("/nonexistent-folder"), PathBuf::from("/bin")]);
        assert_eq!(found, Some(PathBuf::from("/bin/sh")));
        assert_eq!(find_program("no-such-program-here", &[PathBuf::from("/bin")]), None);
    }

    #[test]
    fn a_secret_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let file = std::env::temp_dir().join(format!("clipier-private-{}", std::process::id()));
        std::fs::write(&file, "token").unwrap();
        keep_private(&file).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_file(&file);
    }
}
