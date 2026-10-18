# Security

## What Clipier does and does not protect

Clipier starts the agent CLIs you already use, with the sign-ins you already made. It does not
replace their own safety.

It guarantees:

- A Race or a Crew never touches your checkout, branch or index: agents work in git worktrees, and
  the result is checked at the end (`checkout_untouched` in the run record).
- A writing piece may change only the files its plan lists; anything else is reported and blocks the
  merge.
- Your project's check runs with secrets removed from the environment, with a time limit and, where
  bubblewrap exists (Linux), with no network and only the worktree writable.
- Environment variables that look like secrets are removed from every agent process. Agents that run
  commands without asking (`agy`, `opencode`) only run inside a sandbox, and are unavailable where
  there is none.
- A guard variable stops an agent from starting runs of its own.
- Notes written to the project's memory that look like credentials are refused.
- The only network request Clipier makes itself is to GitHub, when you click *Open a pull request*. It sends one
  branch whose name starts with `clipier/` and the title and description you see. The sign-in token is kept
  in `~/.config/clipier/github-token`, readable only by you, and can be deleted from the interface.
  See [docs/GITHUB.md](docs/GITHUB.md).

It does not guarantee:

- A Chat edits the folder you point it at, as the CLI would in a terminal.
- Agents run with your permissions. A model can be wrong or be manipulated by what it reads.
- Notes are read by later agents as data. Treat the memory folder as untrusted input if you share it.
- On Windows there is no sandbox for the check yet.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting on this repository
(Security tab, "Report a vulnerability"). We will answer within a week.
