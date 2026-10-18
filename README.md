# Leme Pier

**Every agent, one dock.**

Leme Pier is a desktop app that finds the AI coding CLIs already installed and signed in on your machine (`claude`, `codex`, `agy`, `opencode`) and lets you use them from one place. It starts the official CLIs with the sign-ins you already made, sends nothing anywhere by itself, and keeps no account.

![A chat that moved from one agent to another](docs/images/chat.png)

## Why

If you pay for more than one coding agent, you know the cost of switching: the next agent has to read the whole repository again, and you cannot easily ask three of them the same thing and compare. Leme Pier keeps one record of what was said and done, so the next agent is briefed on what it missed instead of starting cold, and it runs agents side by side, each on its own copy of your project.

## What it does

- **Chat.** One conversation you can continue with another agent or model. A reply names the agent and model that wrote it. When the agent changes, the next one gets a short briefing built without a model call: what it has not seen, the state of the repository and the project's notes. An agent that spoke last resumes its own session with no briefing at all.
- **Race.** One brief sent to up to four agent and model pairs at once. Each works on its own git worktree. Your project's check (its tests) runs on each result, and the work waits on a branch per boat: you read what each did and keep one.
- **Crew.** A captain role splits a brief into pieces. You read the plan, then the pieces run side by side where they do not depend on each other, each on its own worktree. A piece is checked, can be looked over by a reviewer, and merges into one integration branch. When something fails, does not merge or is turned back twice, it asks you; nothing else does.
- **Notes.** Facts about your project that agents leave for the next one (a command, a convention), kept as plain markdown files you can read and delete.
- **Agents.** What each CLI says about itself: version, sign-in, models, how much of the Claude allowance is used, what has gone through Leme, and who plays each role.

![One brief, three boats](docs/images/race.png)

![A plan the captain drew, to read before anything starts](docs/images/plan.png)

## Requirements

- Linux (Windows is the next target and is built by CI, but has not been used with real agents yet).
- `git`, and at least one of the agent CLIs, signed in.
- `bubblewrap` (`bwrap`) is recommended: it lets the project's check run without network, and it is required to use `agy` and `opencode`, which run commands without asking.

## Install

There is no release yet. To build it:

```sh
git clone https://github.com/lucaswotta/leme-pier
cd leme-pier/app
npm ci && npm run build
npx tauri build --bundles deb     # a .deb in ../target/release/bundle/deb/
```

Or run it from source with `npm run tauri dev`. Rust (stable) and the Tauri system libraries are needed; see [CONTRIBUTING.md](CONTRIBUTING.md).

## Safe by construction

A Race or a Crew never touches your checkout, branch or index: agents work in git worktrees, and the result is verified at the end. A piece may change only the files its plan lists. The check runs with secrets removed and, where there is a sandbox, with no network. Notes that look like credentials are refused. See [SECURITY.md](SECURITY.md) for what is and is not guaranteed; a Chat, for example, edits the folder you point it at, as the CLI would in a terminal.

## What it does not do

No accounts, no telemetry, no network request of its own, no model API keys, no plugins, no background runs. It is a tool you sit in front of.

## Languages

English, Brazilian Portuguese and Spanish, following the system language (English when it is none of them).

## Status

Early: the first release is 0.0.1. [ARCHITECTURE.md](ARCHITECTURE.md) says how it is built and why it is small on purpose. Whether the briefing really makes the next agent cheaper than starting cold is an assumption that has one data point so far; it will be measured and the result published either way.

If Leme Pier saves you time, a star helps other people find it. Issues and pull requests are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).

[Versão em português](README.pt-BR.md)
