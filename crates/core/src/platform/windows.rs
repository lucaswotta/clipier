use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// Without a console of its own, a CLI started by a window would flash one.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// What a CLI installed with npm or a shim may be called, in the order Windows would try.
const EXTENSIONS: [&str; 4] = ["exe", "cmd", "bat", "com"];

pub fn lead_group(command: &mut Command) {
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

pub fn kill_tree(child: &mut Child) {
    // `taskkill /T` ends the whole tree; `child.kill()` alone would leave the tools it started.
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let _ = child.kill();
    let _ = child.wait();
}

pub fn start(command: &mut Command) -> std::io::Result<Child> {
    command.spawn()
}

fn variable(name: &str) -> PathBuf {
    std::env::var_os(name).map(PathBuf::from).unwrap_or_default()
}

pub fn home_dir() -> PathBuf {
    variable("USERPROFILE")
}

pub fn data_dir() -> PathBuf {
    variable("LOCALAPPDATA")
}

pub fn config_dir() -> PathBuf {
    variable("APPDATA")
}

pub fn runnable(folder: &Path, name: &str) -> Option<PathBuf> {
    let direct = folder.join(name);
    if direct.extension().is_some() && direct.is_file() {
        return Some(direct);
    }
    EXTENSIONS.iter().map(|extension| folder.join(format!("{name}.{extension}"))).find(|path| path.is_file())
}
