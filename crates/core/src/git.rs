//! Git, the way Leme uses it: a worktree per boat or piece, commits on its own branch, merges into
//! an integration branch, diffs, and proof that the user's checkout was never touched.
//!
//! The user's checkout is never written to. Everything happens on branches of Leme's own, in
//! worktrees under the data folder.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::scope;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotARepository(PathBuf),
    NoCommits(PathBuf),
    Git {
        args: String,
        message: String,
    },
    /// The user's checkout changed while Leme was working.
    Touched(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotARepository(path) => write!(formatter, "{} is not a git repository", path.display()),
            Self::NoCommits(path) => write!(formatter, "{} has no commits yet", path.display()),
            Self::Git { args, message } => write!(formatter, "`git {args}` failed: {message}"),
            Self::Touched(what) => write!(formatter, "the repository changed while Leme was working: {what}"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// Directories that running a project's tests or builds fills with files nobody wrote on purpose.
/// Untracked files inside them are neither a change nor part of a commit.
const NOISE_DIRS: [&str; 7] =
    ["__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", "node_modules", "target", ".venv"];

fn is_noise(path: &str) -> bool {
    path.split('/').any(|part| NOISE_DIRS.contains(&part))
}

fn run(dir: &Path, args: &[&str], env: &[(&str, &str)], keep_trailing_newline: bool) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| Error::Git { args: args.join(" "), message: e.to_string() })?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        return Ok(if keep_trailing_newline { text } else { text.trim_end().to_owned() });
    }
    let mut message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if message.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().clone_into(&mut message);
    }
    Err(Error::Git { args: args.join(" "), message })
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    run(dir, args, &[], false)
}

/// An identity for commits when the user has none configured, so that a run never fails on that.
fn fallback_identity(dir: &Path) -> Vec<(&'static str, &'static str)> {
    let has = |key: &str| git(dir, &["config", "--get", key]).is_ok_and(|value| !value.is_empty());
    if has("user.name") && has("user.email") {
        Vec::new()
    } else {
        vec![
            ("GIT_AUTHOR_NAME", "Leme Pier"),
            ("GIT_AUTHOR_EMAIL", "leme@localhost"),
            ("GIT_COMMITTER_NAME", "Leme Pier"),
            ("GIT_COMMITTER_EMAIL", "leme@localhost"),
        ]
    }
}

/// How the user's checkout looked at a moment: to compare against later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub head: String,
    pub branch: String,
    status: String,
}

/// The user's repository.
#[derive(Debug, Clone)]
pub struct Repo {
    dir: PathBuf,
}

/// A working copy of the repository on a branch of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub dir: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    Merged,
    /// The change does not apply on top of what was merged before. Nothing was merged.
    Conflict(Vec<String>),
}

impl Repo {
    pub fn open(dir: &Path) -> Result<Self> {
        if git(dir, &["rev-parse", "--git-dir"]).is_err() {
            return Err(Error::NotARepository(dir.to_path_buf()));
        }
        if git(dir, &["rev-parse", "HEAD"]).is_err() {
            return Err(Error::NoCommits(dir.to_path_buf()));
        }
        Ok(Self { dir: dir.to_path_buf() })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            head: git(&self.dir, &["rev-parse", "HEAD"])?,
            branch: git(&self.dir, &["rev-parse", "--abbrev-ref", "HEAD"])?,
            status: git(&self.dir, &["status", "--porcelain"])?,
        })
    }

    /// Fails if the checkout is not exactly as it was in `before`.
    pub fn check_untouched(&self, before: &Snapshot) -> Result<()> {
        let now = self.snapshot()?;
        if now.head != before.head {
            return Err(Error::Touched(format!("HEAD moved from {} to {}", before.head, now.head)));
        }
        if now.branch != before.branch {
            return Err(Error::Touched(format!("branch `{}` became `{}`", before.branch, now.branch)));
        }
        if now.status != before.status {
            return Err(Error::Touched("the working tree has other changes".into()));
        }
        Ok(())
    }

    /// A new branch at `base`, checked out in a new folder.
    pub fn add_worktree(&self, dir: &Path, branch: &str, base: &str) -> Result<Worktree> {
        git(&self.dir, &["worktree", "add", "-q", "-b", branch, &dir.to_string_lossy(), base])?;
        Ok(Worktree { dir: dir.to_path_buf(), branch: branch.to_owned() })
    }

    /// Removes the folder and keeps the branch, so that the work stays reachable by name.
    pub fn remove_worktree(&self, tree: &Worktree) -> Result<()> {
        git(&self.dir, &["worktree", "remove", "--force", &tree.dir.to_string_lossy()]).map(drop)
    }

    pub fn delete_branch(&self, branch: &str) -> Result<()> {
        git(&self.dir, &["branch", "-D", branch]).map(drop)
    }

    /// The address of a remote, when the repository has it.
    pub fn remote_url(&self, name: &str) -> Option<String> {
        git(&self.dir, &["remote", "get-url", name]).ok().filter(|url| !url.is_empty())
    }

    /// Sends `branch` to the remote called `origin` and makes it track there. Git's own credentials
    /// are used; it never stops to ask a question.
    pub fn push(&self, branch: &str) -> Result<()> {
        git(&self.dir, &["push", "--set-upstream", "origin", &format!("{branch}:{branch}")]).map(drop)
    }

    /// What `branch` adds on top of `base`, cut to `limit` characters.
    pub fn diff(&self, base: &str, branch: &str, limit: usize) -> Result<String> {
        self.diff_of(&[base, branch], limit)
    }

    /// What `branch` changed since it left `base`, ignoring what `base` gained meanwhile.
    pub fn change(&self, base: &str, branch: &str, limit: usize) -> Result<String> {
        self.diff_of(&[&format!("{base}...{branch}")], limit)
    }

    fn diff_of(&self, revisions: &[&str], limit: usize) -> Result<String> {
        let mut args = vec!["diff", "--no-color"];
        args.extend(revisions);
        let text = git(&self.dir, &args)?;
        if text.chars().count() <= limit {
            return Ok(text);
        }
        Ok(format!("{}\n… the diff continues.", text.chars().take(limit).collect::<String>()))
    }
}

impl Worktree {
    /// Every path changed, added or removed, committed or not, relative to `base`.
    pub fn changed_files(&self, base: &str) -> Result<Vec<String>> {
        let status = run(&self.dir, &["status", "--porcelain=v1", "-z", "--untracked-files=all"], &[], true)?;
        let mut files = BTreeSet::new();
        let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
        while let Some(entry) = entries.next() {
            let (code, path) = entry.split_at(entry.len().min(3));
            if code == "?? " && is_noise(path) {
                continue;
            }
            files.insert(path.to_owned());
            // A rename lists its origin as the next entry.
            if (code.starts_with(['R', 'C']) || code[1..].starts_with(['R', 'C']))
                && let Some(origin) = entries.next()
            {
                files.insert(origin.to_owned());
            }
        }
        let committed = git(&self.dir, &["diff", "--name-only", &format!("{base}...HEAD")]).unwrap_or_default();
        files.extend(committed.lines().map(str::to_owned));
        Ok(files.into_iter().collect())
    }

    /// The changed files that fall outside every pattern of `allowed`.
    pub fn out_of_scope(&self, base: &str, allowed: &[String]) -> Result<Vec<String>> {
        Ok(self
            .changed_files(base)?
            .into_iter()
            .filter(|file| !allowed.iter().any(|pattern| scope::matches(pattern, file)))
            .collect())
    }

    /// Commits everything changed. Returns the commit, or `None` when nothing changed.
    pub fn commit(&self, message: &str) -> Result<Option<String>> {
        let excludes: Vec<String> = NOISE_DIRS.iter().map(|dir| format!(":(exclude,glob)**/{dir}/**")).collect();
        let mut add = vec!["add", "-A", "--", "."];
        add.extend(excludes.iter().map(String::as_str));
        git(&self.dir, &add)?;
        if git(&self.dir, &["diff", "--cached", "--name-only"])?.is_empty() {
            return Ok(None);
        }
        run(&self.dir, &["commit", "-q", "-m", message], &fallback_identity(&self.dir), false)?;
        Ok(Some(git(&self.dir, &["rev-parse", "HEAD"])?))
    }

    /// Brings `branch` into this worktree's branch, or reports the conflicting files and leaves the
    /// branch as it was.
    pub fn merge(&self, branch: &str, message: &str) -> Result<Merge> {
        let identity = fallback_identity(&self.dir);
        match run(&self.dir, &["merge", "--no-edit", "-m", message, branch], &identity, false) {
            Ok(_) => Ok(Merge::Merged),
            Err(error) => {
                let files = git(&self.dir, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                let _ = git(&self.dir, &["merge", "--abort"]);
                if files.is_empty() {
                    return Err(error);
                }
                Ok(Merge::Conflict(files.lines().map(str::to_owned).collect()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox {
        root: PathBuf,
        repo: Repo,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("leme-git-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let dir = root.join("repo");
            std::fs::create_dir_all(&dir).unwrap();
            for args in
                [&["init", "-q", "-b", "main"][..], &["config", "user.name", "T"], &["config", "user.email", "t@t"]]
            {
                git(&dir, args).unwrap();
            }
            std::fs::write(dir.join("a.txt"), "one\n").unwrap();
            git(&dir, &["add", "-A"]).unwrap();
            git(&dir, &["commit", "-q", "-m", "init"]).unwrap();
            Self { repo: Repo::open(&dir).unwrap(), root }
        }

        fn tree(&self, name: &str, base: &str) -> Worktree {
            self.repo.add_worktree(&self.root.join(name), &format!("leme/{name}"), base).unwrap()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_worktree_commits_on_its_own_branch_and_the_checkout_is_proved_untouched() {
        let sandbox = Sandbox::new("commit");
        let before = sandbox.repo.snapshot().unwrap();
        let tree = sandbox.tree("boat", "main");
        std::fs::write(tree.dir.join("b.txt"), "two\n").unwrap();
        std::fs::create_dir_all(tree.dir.join("node_modules/x")).unwrap();
        std::fs::write(tree.dir.join("node_modules/x/i.js"), "noise").unwrap();

        assert_eq!(tree.changed_files("main").unwrap(), ["b.txt"], "noise folders are not a change");
        assert!(tree.commit("feat: add b").unwrap().is_some());
        assert_eq!(tree.commit("again").unwrap(), None, "nothing left to commit");
        assert_eq!(tree.changed_files("main").unwrap(), ["b.txt"], "a committed change is still a change");
        assert!(git(&sandbox.repo.dir, &["show", "leme/boat:b.txt"]).unwrap().contains("two"));
        assert!(
            !git(&sandbox.repo.dir, &["ls-tree", "-r", "--name-only", "leme/boat"]).unwrap().contains("node_modules")
        );
        sandbox.repo.check_untouched(&before).unwrap();

        std::fs::write(sandbox.repo.dir.join("a.txt"), "edited\n").unwrap();
        assert!(matches!(sandbox.repo.check_untouched(&before), Err(Error::Touched(_))));
    }

    #[test]
    fn a_piece_that_wrote_outside_its_scope_is_reported() {
        let sandbox = Sandbox::new("scope");
        let tree = sandbox.tree("piece", "main");
        std::fs::create_dir_all(tree.dir.join("src")).unwrap();
        std::fs::write(tree.dir.join("src/lib.rs"), "x").unwrap();
        std::fs::write(tree.dir.join("README.md"), "y").unwrap();
        let outside = tree.out_of_scope("main", &["src/**".to_owned()]).unwrap();
        assert_eq!(outside, ["README.md"]);
    }

    #[test]
    fn a_merge_brings_work_in_and_a_conflict_is_reported_without_forcing_it() {
        let sandbox = Sandbox::new("merge");
        let integration = sandbox.tree("integration", "main");
        let first = sandbox.tree("first", "main");
        let second = sandbox.tree("second", "main");
        std::fs::write(first.dir.join("a.txt"), "from first\n").unwrap();
        first.commit("first").unwrap();
        std::fs::write(second.dir.join("a.txt"), "from second\n").unwrap();
        second.commit("second").unwrap();

        assert_eq!(integration.merge(&first.branch, "merge first").unwrap(), Merge::Merged);
        let result = integration.merge(&second.branch, "merge second").unwrap();
        assert_eq!(result, Merge::Conflict(vec!["a.txt".into()]));
        // Git on Windows may turn line endings into CRLF, hence the trim.
        assert_eq!(std::fs::read_to_string(integration.dir.join("a.txt")).unwrap().trim(), "from first");
        assert_eq!(git(&integration.dir, &["status", "--porcelain"]).unwrap(), "", "the failed merge was undone");
    }

    #[test]
    fn a_branch_is_pushed_to_origin_and_the_remote_is_named() {
        let sandbox = Sandbox::new("push");
        assert_eq!(sandbox.repo.remote_url("origin"), None);
        assert!(sandbox.repo.push("main").is_err(), "there is no remote to push to");
        let bare = sandbox.root.join("origin.git");
        git(&sandbox.root, &["init", "-q", "--bare", "origin.git"]).unwrap();
        git(&sandbox.repo.dir, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
        let tree = sandbox.tree("boat", "main");
        std::fs::write(tree.dir.join("b.txt"), "b").unwrap();
        tree.commit("b").unwrap();
        sandbox.repo.push(&tree.branch).unwrap();
        assert!(git(&bare, &["branch", "--list", "leme/boat"]).unwrap().contains("leme/boat"));
        assert_eq!(sandbox.repo.remote_url("origin").as_deref(), Some(bare.to_string_lossy().as_ref()));
    }

    #[test]
    fn removing_a_worktree_keeps_its_branch_and_a_repository_needs_a_commit() {
        let sandbox = Sandbox::new("remove");
        let tree = sandbox.tree("keep", "main");
        std::fs::write(tree.dir.join("c.txt"), "c").unwrap();
        tree.commit("c").unwrap();
        sandbox.repo.remove_worktree(&tree).unwrap();
        assert!(!tree.dir.exists());
        assert!(sandbox.repo.diff("main", "leme/keep", 1000).unwrap().contains("+c"));
        let empty = sandbox.root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]).unwrap();
        assert!(matches!(Repo::open(&empty), Err(Error::NoCommits(_))));
        assert!(matches!(Repo::open(&sandbox.root), Err(Error::NotARepository(_))));
    }
}
