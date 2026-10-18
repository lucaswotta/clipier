# Changelog

Versions follow [semantic versioning](https://semver.org); while the major version is 0, anything may change.

## 0.0.1 (2026-10-18)

The first release.

- **Chat** across the four agent CLIs (`claude`, `codex`, `agy`, `opencode`) with a briefing, built without a
  model call, for the agent that takes over.
- **Race**: one brief to up to four agent and model pairs, each on its own git worktree, with the project's
  check run on each result.
- **Crew**: a captain's plan, read before it starts, carried out by roles in parallel and merged into one
  branch; it asks only when a piece fails, does not merge, or is turned back twice.
- **Open a pull request** from a finished boat or crew, through the device flow or the user's own `gh` sign-in.
- **Agents** screen: what each CLI says about itself, the Claude allowance, usage through Leme, role pickers.
- Roles as markdown files (built in, per user, per project) and `.leme-pier/config.toml`.
- English, Brazilian Portuguese and Spanish, following the system language.
- Linux: a `.deb`. Windows: an installer built by CI, not signed (Windows may warn) and not yet used with
  real agents.

Known limits: `agy` and `opencode` only run where `bubblewrap` exists, so not on Windows yet. The claim that a
briefing makes the next agent cheaper than starting cold has one data point and is not measured.
