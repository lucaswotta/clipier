# Leme Pier: architecture

Status: describes what 0.0.1 is. Where the code and this file disagree, the code is right and this file is a bug.

## What it is

Leme Pier is a desktop app that finds the AI coding CLIs already installed and signed in on your
machine (`claude`, `codex`, `agy`, `opencode`), and lets you use them from one place:

- **Chat.** One conversation you can continue with a different agent or model without starting
  over. The next agent is briefed on what happened; nobody rereads the whole repository.
- **Race.** One brief sent to up to four agent and model pairs at once. Each works on its own copy of
  the project. You read the results and keep one.
- **Crew.** One brief, split by a captain into pieces. Each piece goes to a role (builder, reviewer,
  ...), pieces that do not depend on each other run at the same time, and everything merges into one
  branch.
- **Memory.** Notes about a project that every agent reads and may add to.

It does not call any model API. It starts the official CLIs with the sign-ins you already made.

The name is the dock where your agents meet; the executable is `leme`. The tagline is "Every agent, one dock".

### Not goals

No hosted service, no accounts, no telemetry, no model API keys, no plugin system, no scheduling
engine, no autonomous background runs. It is a tool you sit in front of.

## Principles

1. **Small.** Target 4 to 5 thousand lines of Rust, about 2 thousand of JavaScript, tests excluded.
   A feature that needs a new subsystem is a feature we decline.
2. **Explainable.** Each mode must be explainable to a twelve-year-old in two sentences. If it cannot,
   it is redesigned, not documented.
3. **Your checkout is never touched by a Race or a Crew.** Their agents work in git worktrees and
   what they produce lives on branches until you decide. A Chat is the exception, as it is in a
   terminal: the agent edits the folder you point it at, and the screen shows the repository state
   so you can see what changed.
4. **The agents do the thinking.** Leme chooses nothing for the model: no hidden prompts that change
   behavior, no retries that spend tokens silently. It routes, isolates, records and briefs.
5. **Local only.** Everything is a file on your disk. Nothing leaves the machine except what the
   agent CLIs themselves send, and one branch plus a pull request to GitHub when the user clicks for it.
6. **Open from the first commit.** Apache-2.0. History is written to be read.

## Vocabulary

The interface is nautical; the code uses the plain term and the interface maps it.

| Plain term (code) | Shown as | Meaning |
|---|---|---|
| agent | agent | One of the CLIs: `claude`, `codex`, `agy`, `opencode` |
| model | model | The model an agent runs; empty means the CLI's default |
| conversation | conversation | A record of a chat, kept by Leme |
| brief | the brief | What the user asks for |
| race / boat | Race / boat | One brief to several agents in parallel / one of them |
| crew / piece | Crew / piece | A brief carried out by a plan / one step of the plan |
| captain | captain | The role that writes the plan |
| role | role | A named job: who does it (agent, model) and how it is told to |
| note | note | A fact about a project that agents share |
| check | check | The project's own command that says whether work is good (its tests) |

## Repository layout

```
leme-pier/
  Cargo.toml                 workspace
  crates/
    core/                    everything that is not a window (library, testable without one)
      src/
        platform/            the only place that knows Linux from Windows (+ the bubblewrap arguments)
        agents/              one file per CLI (command line, output, detection) + a turn with an agent
        runner/              starts a process, watches it, stops it; classifies why it failed
        session/             conversations and the briefing
        crew/                the plan carried out; piece/ is one piece's work
        race.rs  plan.rs  roles.rs  config.rs
        git.rs  github.rs  check.rs  scope.rs  memory.rs  usage.rs  store.rs  id.rs
  app/
    src-tauri/               Tauri shell: commands only, no logic; the window self-test
    src/                     interface: vanilla JS modules, one catalog per language, one stylesheet
    index.html  package.json  vite.config.js  scripts/check-locales.mjs
  docs/                      ROADMAP and notes
  scripts/verify.sh          fmt, clippy -D warnings, tests (and a Windows lint when its target is installed)
```

`core` has no dependency on Tauri. The shell turns each public function of `core` into a command and
nothing more; if the shell grows logic, the logic moves to `core`.

### Size (Rust, without tests and comments)

| Part | Lines |
|---|---|
| `agents` (four CLIs, output, commands, detection, a turn) | 900 |
| `crew` | 870 |
| `race`, `plan`, `roles`, `config` | 820 |
| `session`, `memory`, `usage`, `store`, `id` | 730 |
| `git`, `check`, `scope` | 510 |
| `runner`, `platform` | 480 |
| `app/src-tauri` (commands, shell, self-test) | 780 |
| **Total** | **about 5,100** |

The interface is about 2,500 lines of JavaScript, catalogs and CSS. For comparison, the prototype had
13,600 lines of Rust. It stays as a private archive.

## Core modules

### `agents`

Each CLI is one file with the same three things, chosen by an `Agent` enum: how to build its command
line from a `Request`, how to read one line of its output into a `Transcript`, and how to ask it what it
is (`detect`).

Detection asks each CLI itself, without a model call, and was checked by hand against the four CLIs on
the author's machine:

| CLI | Models | Usage and limits |
|---|---|---|
| `claude` | `claude -p "/model"` prints the aliases it accepts (parsed from text) | `claude -p "/usage"` prints the session and weekly percentage used and when each resets |
| `codex` | `codex debug models` prints the catalog as JSON (slug, display name, reasoning levels, visibility) | to investigate |
| `agy` | `agy models` prints id and name per line | known only when a call fails ("quota reached, resets in ...") unless a command is found |
| `opencode` | `opencode models` prints `provider/model` per line | to investigate |

Slash commands work non-interactively for Claude; for the others the table lists what exists. Every
parser is tested against a recorded output, and a CLI whose command stops working degrades to "default
model only" instead of failing.

Each adapter is one file and carries what was expensive to learn: how the CLI names a session, how it
resumes it, how it asks for a sandbox, how it reports tokens, what its output looks like. A CLI that
changes its format breaks one file and one test. `Transcript` collects the events (message, command,
edit, tool, error), the final answer, the session id and the token counts.

### `runner`

Starts a `Spawn`, copies its output to a file as it arrives (so a killed run still leaves what it
printed), feeds it to the adapter's parser, and enforces three limits: total time, silence, and
cancellation. It stops the whole process tree. Failures are classified once, here: unusable (not
signed in, unknown model), limited (quota, with the reset time when the CLI says it), or transient.
A quota that resets in hours counts as unusable for the run, not as something to wait for.

### `platform`

The only module with `cfg(target_os)`. It provides: stopping a process tree, the data and config
folders, resolving a CLI on `PATH` (including `.cmd` shims on Windows), and the check sandbox (see
Safety). Nothing else in the code base may import `std::os::unix`.

### `session`

A conversation is a folder: `meta.json` (repository, title, each agent's own session id, how much of
the log each agent has seen) and `log.jsonl` (user messages, agent replies with edited files and
commands, failures).

When an agent takes a turn it gets one of two things:

- it spoke last and has its own session: the plain message, resumed in its own session, no briefing;
- otherwise a **briefing** built without a model: what it has not seen (newest entries first, capped),
  the state of the repository (`git status`, diff size), and the project's notes.

A reply may end with `NOTE: title | text` lines. They are removed from the reply and offered to
memory.

### `memory`

Markdown files per repository, one note per file, with front matter. Secret-looking text is refused,
duplicates are dropped, each turn may add at most a few notes, and the user can forget any note.

### `usage`

An append-only log of what each agent did through Leme: agent, model, role, tokens (new, cached, out),
duration, outcome. The Agents screen reads it. It replaces the statistics the prototype read from the
old tooling.

### `git`

Worktrees under the data folder, one per boat or piece, branched from the starting commit; commit
with a fallback identity; merge a piece into the integration branch; diff; changed files; a scope
check (real glob matching) so a piece that wrote outside its declared paths is reported; noise
folders (`node_modules`, `target`, ...) ignored. It can state that the user's checkout is exactly as it
was.

### `check`

Detects the project's test command (`cargo test`, `npm test`, `pytest`, `go test`, `make test`, ...)
or takes one from `.leme-pier/config.toml` or the form. Runs it in a worktree with a time limit, keeps the
tail of the output, and reports passed or failed.

## Roles

A role answers: who does this job and how are they told to. A role is a markdown file.

```markdown
---
name: builder
title: Shipwright          # shown name; the interface may translate built-ins
mode: write                # read | write
agent: codex
model: ""                  # empty = the CLI's default
fallback:                  # optional, one only
  agent: claude
  model: sonnet
---
You implement the piece you are given inside this worktree. Change only what the piece needs.
When done, reply with a short summary of what you changed.
```

- **Built-in** roles ship with the app: `captain` (read), `builder` (write), `reviewer` (read),
  `tester` (read). They exist in English, Portuguese and Spanish.
- **User** roles: `~/.config/leme-pier/roles/*.md`. **Project** roles: `.leme-pier/roles/*.md`,
  committed with the code. A file with the same `name` overrides the one before it
  (project over user over built-in).
- **Fallback** is used once, and only when the agent is unusable or limited, never for a bad answer.
  It is the only automatic recovery in the app.
- A role that names an agent that is not installed is shown as unavailable in the Agents screen and
  cannot be assigned.

Choosing models per role happens in one screen: a row per role with an agent picker and a model
picker. It writes the user file.

### Project config

`.leme-pier/config.toml`, optional:

```toml
check = "cargo test"       # what a piece must pass
captain = "captain"        # which role plans
approve_plan = true        # false: the crew starts as soon as the plan is valid
parallel = 3               # pieces that may run at the same time
```

## Race

1. Input: repository, brief, 1 to 4 boats (agent and model each), optional check.
2. Each boat gets a worktree and runs in its own thread with the brief, a one-line statement that
   others work on the same task elsewhere, and the project's notes.
3. When a boat finishes: commit, record the changed files, run the check, record the verdict.
4. Output per boat: summary, files, check verdict, tokens, duration, branch name, diff. Branches are
   `leme/<run>/<boat>`. The screen shows the commands to inspect and merge one.
5. Nothing is integrated automatically. The worktrees are removed at the end; the branches stay.

"Improve my brief" is a separate, optional, read-only turn that rewrites the brief; the user can undo
it and edit it. It is never applied silently.

## Crew (the simple mode)

The crew carries out a plan. There is no scheduler, no retry policy and no budget engine.

### The plan

The captain role returns JSON, validated before anything starts:

```json
{
  "summary": "Add the export command in two steps, then review.",
  "pieces": [
    { "id": "schema", "role": "builder", "title": "Define the export format",
      "brief": "...", "touches": ["src/export/format.rs"], "after": [] },
    { "id": "command", "role": "builder", "title": "Add the command",
      "brief": "...", "touches": ["src/cli/export.rs"], "after": ["schema"] },
    { "id": "look", "role": "reviewer", "title": "Look over the command",
      "brief": "...", "touches": [], "after": ["command"], "reviews": "command" }
  ]
}
```

Rules, all checked in `plan.rs`: ids are unique; every `role` exists and is available; `after` refers
to existing pieces and has no cycle; at most 12 pieces; two write pieces that can run at the same
time must not overlap in `touches`; a `reviews` piece names a write piece and uses a read role. A plan
that breaks a rule is rejected with the reason and the captain is asked once to fix it.

### Execution

The user reads the plan and approves it (`approve_plan = false` skips the reading; the plan is still
validated and still shown while it runs). Then, in a loop:

1. A piece is **ready** when everything in its `after` has merged (or, for a review, finished).
2. Ready pieces start, up to `parallel` at a time (default 3), each in a worktree branched from
   the integration branch as it is now. A piece is also given the summaries of the pieces it comes
   `after`, since the merged code does not say why it was written that way.
3. A finished write piece is committed, scope-checked, run through the check, and merged into the
   integration branch (`leme/<run>/integration`). A conflict is never forced.
4. A finished review piece returns `approved` or a list of changes. Changes send the reviewed piece
   back once with the notes; a second rejection asks the user.
5. The run ends when every piece merged, was skipped or stopped.

### What asks the user

Exactly three situations, each with the same choices (redo, skip, stop):

- a piece failed (agent error, or the check failed after one retry);
- a piece does not merge cleanly;
- a review rejected a piece twice.

Everything else proceeds. A cancel button stops every process.

## Data on disk

| What | Where (Linux) |
|---|---|
| Conversations | `$XDG_DATA_HOME/leme-pier/conversations/<id>/` |
| Races and crews | `.../runs/<id>/` (`run.json`, `events.jsonl`, `out/`) |
| Worktrees | `.../worktrees/<run>/<piece>/` |
| Memory | `.../memory/<repo-key>/<note>.md` |
| Usage | `.../usage.jsonl` |
| User roles and settings | `$XDG_CONFIG_HOME/leme-pier/` |
| Project settings and roles | `.leme-pier/` in the repository |

Every JSON file carries a `version` field. A reader that meets a newer version refuses it and says
so. Writes go through a temporary file and a rename. A line cut short by a crash is skipped on read.

## Safety model

What Leme guarantees:

- The user's checkout, branch and index are untouched; Leme can verify it and says so when it cannot.
- Each boat and piece writes only in its worktree. A write piece's `touches` is enforced after the
  fact: out-of-scope changes are reported and block the merge.
- The project's check runs with secrets removed from the environment, with a time limit, and in a
  sandbox without network (bubblewrap) where the platform has one.
- Environment variables that look like secrets are removed from every agent process.
- Leme never starts a run from inside a run it started (a guard variable), so an agent cannot
  recursively spawn a crew.
- Notes that look like credentials are never stored.

What it does not guarantee, and says so in `SECURITY.md`:

- The agents run with your sign-ins and your permissions. Leme asks each CLI for its edit-accepting,
  sandboxed mode where one exists; it does not replace the CLI's own safety.
- Where there is no sandbox (Windows), the check runs unsandboxed. The interface says so.

## The app

Tauri 2. The webview loads local files only; capabilities grant the commands and nothing else (a test
asserts that core APIs are denied). The interface polls the shell for state instead of subscribing to
events, which keeps the permission surface minimal.

Screens (first release): **Chat**, **Set course** (Race and Crew), **Agents** (installed, signed in,
version, models, usage, roles with agent pickers). The sidebar lists past runs. A day and a night
palette. The language follows the system: English, Brazilian Portuguese, Spanish, English as the
fallback; there is no picker.

Front-end modules: `chat`, `course` (form, race view), `crew` (plan, run), `agents`, `fleet`
(shared agent list), `i18n` with one catalog per language, `markdown` (a tiny safe renderer),
`style.css`. No framework, no bundled state library.

## Platforms

- **Linux** is complete in 0.0.1: `.deb` and AppImage.
- **Windows** is the next target; `platform` is shaped for it (process tree through Job Objects,
  `.cmd` resolution, `AppData`, no bubblewrap). CI builds and tests on Windows from the first release
  so it does not rot; running real agents there is verified by hand.
- **macOS** after Windows; needs a paid developer account to notarize.

## Testing

Tests exist to catch what would hurt a user. They are not a measure of effort, and there is no target
count. The rules:

- **A test names a behavior.** "A quota that resets in hours makes the agent unusable" is a test.
  A test that restates the code, checks a getter or exists to raise a number is deleted.
- **Fast by construction.** The whole default suite runs in under 30 seconds. A test that needs more
  time does not belong in it. `scripts/verify.sh quick` (format, clippy, the tests of the crate being
  changed) is what runs while working; the full `verify.sh` runs in CI.
- **What is tested, and how:**
  - the four output parsers and the model and usage detection, each against a recorded output of the
    real CLI;
  - the runner (limits, cancel, process tree) with tiny shell scripts as fake agents;
  - git operations (worktrees, commit, merge, scope, conflict) in a temporary repository;
  - plan validation (cycles, overlap, unknown roles), sessions and the briefing, memory filtering;
  - one end-to-end test of a Race and one of a Crew with fake agents.
- **Not in the default suite:** real agents, benchmarks and the window self-test. Tests against real
  agents are marked `#[ignore]` and run by hand. The window self-test (`LEME_PIER_SELFTEST=<file>`,
  covering the three languages) runs when the interface changes and before a release, not on every
  commit.
- A Tauri IPC test checks that commands work and that disallowed APIs are denied.
- No benchmark ships in 0.0.1. Whether a briefing saves tokens is measured after the app is usable;
  the method and the results go to `docs/`.

## Release and project

- Version `0.0.1`; semantic versioning with 0.x meaning "may break". GitHub Releases carry the
  packages and a changelog.
- License: Apache-2.0. `CONTRIBUTING.md` (verify before a PR, English in code and docs, commits as
  Conventional Commits), `SECURITY.md` (the model above and how to report), issue and PR templates.
- No telemetry. The app makes no network request of its own.

## What the prototype had that this does not

| Prototype | Fate |
|---|---|
| `scheduler`, `orchestrator`, `planner` | Replaced by `plan` and `crew`: no state machine, no policy engine |
| Role chains, budgets, limits, per-role stats from the old tooling | Replaced by role files with one fallback; own `usage` log |
| `legacy` (reads the old harness), On deck, Logbook, run drawer | Dropped |
| `bench`, `headless run`, `gc`, `voyages` CLI | Dropped; a CLI can come later over `core` |
| Review chains across model families | Kept as the `reviewer` role, chosen by the user |

## How the history reads

Each commit builds and passes `scripts/verify.sh` on its own, on Linux and on Windows: the workspace
and CI, the platform layer and the runner, what each CLI prints, the command lines, detection, git and
the check, memory and usage, a turn with an agent, conversations, roles and plans, the Race, the Crew,
the window's shell and commands, the interface, and the window self-test.

## Risks and open points

- **Name.** A company called Leme (leme.ai) ships an AI product and has a CLI repository for coding
  agents. The trademark search (INPI and others) is pending and the name is kept in one place so it can
  change.
- **Terms of use.** The agent vendors' terms on driving their official CLIs from another program must
  be read before the repository is announced.
- **Format drift.** The CLIs change their output. The adapters and their recorded-stream tests are the
  defense; a breakage is a one-file fix.
- **Promise to prove.** That a briefing makes the next agent cheaper than starting cold is an
  assumption with one data point. It is measured after 0.0.1 and reported either way.
- **Windows.** `agy` and some CLIs may not run there; unknown until tested.
