//! What each agent did through Leme Pier: one line per use, appended to a file, read back as totals
//! for the Agents screen. It is Leme's own record, so it counts exactly what went through it.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// What the use was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Chat,
    Race,
    Crew,
    /// Rewriting a brief.
    Brief,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Delivered,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Use {
    pub at_ms: u64,
    pub agent: String,
    pub model: String,
    pub purpose: Purpose,
    pub outcome: Outcome,
    pub tokens_in: u64,
    pub tokens_cached: u64,
    pub tokens_out: u64,
    pub duration_ms: u64,
}

impl Use {
    /// A cached input token costs about a tenth of a fresh one.
    pub fn effective_tokens(&self) -> u64 {
        self.tokens_in.saturating_add(self.tokens_out).saturating_add(self.tokens_cached / 10)
    }
}

/// Totals for one agent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Totals {
    pub uses: u64,
    pub delivered: u64,
    pub tokens: u64,
    pub duration: Duration,
}

impl Totals {
    pub fn average_duration(&self) -> Option<Duration> {
        u32::try_from(self.uses).ok().filter(|uses| *uses > 0).map(|uses| self.duration / uses)
    }
}

/// One agent's totals, in numbers the screen can show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub agent: String,
    pub uses: u64,
    pub delivered: u64,
    pub tokens: u64,
    pub average_ms: Option<u64>,
}

pub struct Log {
    file: PathBuf,
}

impl Log {
    pub fn new(file: &Path) -> Self {
        Self { file: file.to_path_buf() }
    }

    pub fn record(&self, usage: &Use) -> std::io::Result<()> {
        if let Some(folder) = self.file.parent() {
            fs::create_dir_all(folder)?;
        }
        let mut line = serde_json::to_vec(usage).map_err(std::io::Error::other)?;
        line.push(b'\n');
        OpenOptions::new().create(true).append(true).open(&self.file)?.write_all(&line)
    }

    /// Every use, in order. A line cut short by a crash is skipped.
    pub fn read(&self) -> Vec<Use> {
        let text = fs::read_to_string(&self.file).unwrap_or_default();
        text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    /// Totals per agent.
    pub fn totals(&self) -> BTreeMap<String, Totals> {
        let mut totals: BTreeMap<String, Totals> = BTreeMap::new();
        for usage in self.read() {
            let entry = totals.entry(usage.agent.clone()).or_default();
            entry.uses += 1;
            entry.delivered += u64::from(usage.outcome == Outcome::Delivered);
            entry.tokens = entry.tokens.saturating_add(usage.effective_tokens());
            entry.duration += Duration::from_millis(usage.duration_ms);
        }
        totals
    }

    /// The totals as a list for the screen, the busiest agent first.
    pub fn summary(&self) -> Vec<Summary> {
        let mut list: Vec<Summary> = self
            .totals()
            .into_iter()
            .map(|(agent, totals)| Summary {
                average_ms: totals.average_duration().map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                agent,
                uses: totals.uses,
                delivered: totals.delivered,
                tokens: totals.tokens,
            })
            .collect();
        list.sort_by_key(|entry| std::cmp::Reverse(entry.uses));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(agent: &str, outcome: Outcome, tokens_in: u64, cached: u64, duration_ms: u64) -> Use {
        Use {
            at_ms: 1,
            agent: agent.into(),
            model: "m".into(),
            purpose: Purpose::Chat,
            outcome,
            tokens_in,
            tokens_cached: cached,
            tokens_out: 5,
            duration_ms,
        }
    }

    #[test]
    fn totals_count_uses_deliveries_tokens_with_cache_at_a_tenth_and_the_average_time() {
        let file = std::env::temp_dir().join(format!("leme-usage-{}", std::process::id())).join("usage.jsonl");
        let _ = fs::remove_dir_all(file.parent().unwrap());
        let log = Log::new(&file);
        log.record(&usage("claude", Outcome::Delivered, 100, 1000, 2000)).unwrap();
        log.record(&usage("claude", Outcome::Failed, 10, 0, 4000)).unwrap();
        log.record(&usage("codex", Outcome::Delivered, 1, 0, 1000)).unwrap();
        OpenOptions::new().append(true).open(&file).unwrap().write_all(b"{\"at_ms\":1,\"agen").unwrap();

        let totals = log.totals();
        let claude = &totals["claude"];
        assert_eq!((claude.uses, claude.delivered), (2, 1));
        assert_eq!(claude.tokens, (100 + 5 + 100) + (10 + 5), "cached input counts a tenth");
        assert_eq!(claude.average_duration(), Some(Duration::from_secs(3)));
        assert_eq!(totals["codex"].uses, 1);
        assert_eq!(log.read().len(), 3, "the torn line is skipped");
        let _ = fs::remove_dir_all(file.parent().unwrap());
    }
}
