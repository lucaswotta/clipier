//! What agents learn about a project and the next ones should not have to find out again.
//!
//! An agent may end a reply with a few lasting facts (a command, a convention, where
//! something lives). Leme Pier curates them: they are cut to size, refused if they look like a
//! secret, merged when they repeat, and kept as plain markdown files per repository. Every later
//! conversation and piece starts with them, which costs a few hundred tokens instead of a search.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// A fact an agent offers for the memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub text: String,
}

const MAX_NOTES: usize = 40;
/// Most notes one reply may add, and most it may offer for that.
pub const MAX_PER_REPLY: usize = 3;
const MAX_TITLE: usize = 80;
const MAX_TEXT: usize = 300;
/// How much of the memory goes into a briefing.
const BRIEFING_LIMIT: usize = 1_600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stored {
    pub id: String,
    pub title: String,
    pub text: String,
    /// Where the note came from: a conversation or a run.
    pub source: String,
}

pub struct Memory {
    dir: PathBuf,
}

/// A folder name for a repository: its name plus a hash of where it is.
fn repository_key(repository: &Path) -> String {
    let canonical = fs::canonicalize(repository).unwrap_or_else(|_| repository.to_path_buf());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.to_string_lossy().bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let name: String = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .take(32)
        .collect();
    format!("{name}-{hash:016x}")
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Whether a text looks like a credential or personal data, which memory must never keep.
fn looks_secret(text: &str) -> bool {
    let lower = text.to_lowercase();
    let markers =
        ["password", "passwd", "secret", "token", "api_key", "apikey", "private key", "authorization:", "bearer "];
    if markers.iter().any(|m| lower.contains(m)) {
        return true;
    }
    // A long run of key-like characters is almost never prose.
    text.split_whitespace().any(|word| {
        let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        word.len() >= 32 && word.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '='))
    })
}

fn cut(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        format!("{}…", flat.chars().take(limit - 1).collect::<String>().trim_end())
    }
}

impl Memory {
    pub fn open(state_dir: &Path, repository: &Path) -> Self {
        Self { dir: state_dir.join("memory").join(repository_key(repository)) }
    }

    pub fn notes(&self) -> Vec<Stored> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        let mut found: Vec<Stored> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".md")?.to_owned();
                read_note(&entry.path(), id)
            })
            .collect();
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found
    }

    /// Keeps the notes worth keeping and returns them. Anything that looks like a secret, repeats
    /// a note already kept or arrives once the memory is full is left out.
    #[cfg(test)]
    pub fn propose(&self, notes: &[Note], source: &str) -> Vec<Stored> {
        self.propose_up_to(notes, source, MAX_PER_REPLY)
    }

    /// Like [`Memory::propose`], keeping up to `limit` notes: a reply has little room and
    /// a repository has much to say.
    pub fn propose_up_to(&self, notes: &[Note], source: &str, limit: usize) -> Vec<Stored> {
        let mut existing = self.notes();
        let mut accepted = Vec::new();
        for note in notes.iter().take(limit * 2) {
            if accepted.len() >= limit {
                break;
            }
            let (title, text) = (cut(&note.title, MAX_TITLE), cut(&note.text, MAX_TEXT));
            if title.is_empty() || text.is_empty() || looks_secret(&title) || looks_secret(&text) {
                continue;
            }
            let repeated = existing
                .iter()
                .any(|n| normalize(&n.title) == normalize(&title) || normalize(&n.text) == normalize(&text));
            if repeated || existing.len() >= MAX_NOTES {
                continue;
            }
            let next =
                existing.iter().filter_map(|n| n.id.split('-').next()?.parse::<u32>().ok()).max().unwrap_or(0) + 1;
            let slug: String = title
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
                .take(40)
                .collect();
            let stored =
                Stored { id: format!("{next:03}-{}", slug.trim_matches('-')), title, text, source: source.to_owned() };
            if write_note(&self.dir, &stored).is_ok() {
                existing.push(stored.clone());
                accepted.push(stored);
            }
        }
        accepted
    }

    pub fn forget(&self, id: &str) -> bool {
        id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && fs::remove_file(self.dir.join(format!("{id}.md"))).is_ok()
    }

    /// The notes as a section of a briefing, or nothing when there are none.
    pub fn briefing(&self) -> String {
        let mut out = String::new();
        for note in self.notes() {
            let line = format!("- **{}**: {}\n", note.title, note.text);
            if out.len() + line.len() > BRIEFING_LIMIT {
                break;
            }
            out.push_str(&line);
        }
        if out.is_empty() {
            return out;
        }
        format!(
            "\n\n## What earlier agents learned about this project\n\nTrust the repository over these notes if they disagree.\n\n{out}"
        )
    }
}

fn write_note(dir: &Path, note: &Stored) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let body =
        format!("---\ntitle: {}\nsource: {}\n---\n\n{}\n", note.title.replace('\n', " "), note.source, note.text);
    fs::write(dir.join(format!("{}.md", note.id)), body)
}

fn read_note(path: &Path, id: String) -> Option<Stored> {
    let content = fs::read_to_string(path).ok()?;
    let rest = content.strip_prefix("---\n")?;
    let (head, body) = rest.split_once("\n---\n")?;
    let field = |key: &str| head.lines().find_map(|l| l.strip_prefix(&format!("{key}: "))).map(str::to_owned);
    Some(Stored {
        id,
        title: field("title")?,
        text: body.trim().to_owned(),
        source: field("source").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(name: &str) -> Memory {
        let root = std::env::temp_dir().join(format!("leme-pier-memory-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Memory::open(&root, Path::new("/some/project"))
    }

    fn note(title: &str, text: &str) -> Note {
        Note { title: title.into(), text: text.into() }
    }

    #[test]
    fn a_note_is_kept_as_a_markdown_file_and_read_back() {
        let memory = memory("roundtrip");
        let kept = memory
            .propose(&[note("Run the tests", "python3 -m unittest from the repository root.")], "calm-heron-1a2b");
        assert_eq!(kept.len(), 1);
        let back = memory.notes();
        assert_eq!((back[0].title.as_str(), back[0].source.as_str()), ("Run the tests", "calm-heron-1a2b"));
        assert!(
            fs::read_to_string(memory.dir.join(format!("{}.md", back[0].id)))
                .unwrap()
                .starts_with("---\ntitle: Run the tests")
        );
    }

    #[test]
    fn something_that_looks_like_a_secret_is_never_kept() {
        let memory = memory("secrets");
        let kept = memory.propose(
            &[
                note("Login", "the password is hunter2"),
                note("Deploy", "use token ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
                note("Key", "AKIAIOSFODNN7EXAMPLEAKIAIOSFODNN7EXAMPLE is the id"),
                note("Fine", "The build output goes to dist/."),
            ],
            "r",
        );
        assert_eq!(kept.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["Fine"]);
    }

    #[test]
    fn a_repeated_fact_is_not_stored_twice() {
        let memory = memory("repeats");
        memory.propose(&[note("Tests", "Run python3 -m unittest.")], "a");
        let again = memory
            .propose(&[note("tests", "something else"), note("Other title", "  run   PYTHON3 -m unittest. ")], "b");
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(memory.notes().len(), 1);
    }

    #[test]
    fn notes_are_cut_to_size_and_only_three_are_taken_per_report() {
        let memory = memory("limits");
        let long = "word ".repeat(200);
        let kept = memory.propose(&[note("A", &long), note("B", "b"), note("C", "c"), note("D", "d")], "r");
        assert_eq!(kept.len(), 3);
        assert!(kept[0].text.chars().count() <= MAX_TEXT && kept[0].text.ends_with('…'));
    }

    #[test]
    fn the_memory_stops_taking_notes_when_full() {
        let memory = memory("full");
        for i in 0..MAX_NOTES + 5 {
            memory.propose(&[note(&format!("Fact {i}"), &format!("distinct text {i}"))], "r");
        }
        assert_eq!(memory.notes().len(), MAX_NOTES);
    }

    #[test]
    fn the_briefing_lists_the_notes_within_its_limit_and_is_empty_without_any() {
        let memory = memory("briefing");
        assert_eq!(memory.briefing(), "");
        for i in 0..40 {
            memory.propose(&[note(&format!("Fact {i}"), &"word ".repeat(50))], "r");
        }
        let text = memory.briefing();
        assert!(text.contains("**Fact 0**") && text.len() < BRIEFING_LIMIT + 200, "{}", text.len());
    }

    #[test]
    fn a_forgotten_note_is_gone_and_a_bad_id_cannot_reach_other_files() {
        let memory = memory("forget");
        let id = memory.propose(&[note("Tests", "Run them.")], "r")[0].id.clone();
        assert!(!memory.forget("../../etc/passwd") && !memory.forget("missing"));
        assert!(memory.forget(&id));
        assert_eq!(memory.notes(), Vec::new());
    }

    #[test]
    fn each_repository_has_its_own_memory() {
        let root = std::env::temp_dir().join(format!("leme-pier-memory-two-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Memory::open(&root, Path::new("/a/project")).propose(&[note("T", "t")], "r");
        assert_eq!(Memory::open(&root, Path::new("/b/project")).notes(), Vec::new());
        assert_eq!(Memory::open(&root, Path::new("/a/project")).notes().len(), 1);
    }
}
