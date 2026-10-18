//! Where things live on disk. Everything Clipier writes is under one folder, and every module asks
//! here instead of building its own paths.

use std::path::{Path, PathBuf};

use crate::platform;

/// The folders and files of the app's data.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
    }

    /// The user's own folder: `clipier` under the platform's data folder.
    pub fn standard() -> Self {
        Self { root: platform::data_dir().join("clipier") }
    }

    pub fn conversations(&self) -> PathBuf {
        self.root.join("conversations")
    }

    pub fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }

    pub fn worktrees(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    /// The parent of the per-repository note folders.
    pub fn memory(&self) -> PathBuf {
        self.root.join("memory")
    }

    pub fn usage_file(&self) -> PathBuf {
        self.root.join("usage.jsonl")
    }
}

/// The user's own settings folder: `clipier` under the platform's config folder.
pub fn config_folder() -> PathBuf {
    platform::config_dir().join("clipier")
}

/// Where the user's role files live.
pub fn user_roles() -> PathBuf {
    config_folder().join("roles")
}
