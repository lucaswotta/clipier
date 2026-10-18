use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

pub fn lead_group(command: &mut Command) {
    command.process_group(0);
}

pub fn kill_tree(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: killpg only sends a signal to the process group this child leads.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.wait();
}

pub fn start(command: &mut Command) -> std::io::Result<Child> {
    let mut attempts_left = 5;
    loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts_left > 0 => {
                attempts_left -= 1;
                thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn xdg(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home_dir().join(fallback))
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn runnable(folder: &Path, name: &str) -> Option<PathBuf> {
    let path = folder.join(name);
    let mode = std::fs::metadata(&path).ok().filter(std::fs::Metadata::is_file)?.permissions().mode();
    (mode & 0o111 != 0).then_some(path)
}
