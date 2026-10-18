# Contributing to Leme Pier

Thanks for looking. The project is small on purpose: a feature that needs a new subsystem is one
we will probably decline, and a change that makes the code shorter is welcome. Read
[ARCHITECTURE.md](ARCHITECTURE.md) first; it says what lives where and what does not.

## Before you open a pull request

```sh
scripts/verify.sh            # format, lint, tests: what CI runs
scripts/verify.sh quick      # format and lint only, while you work
```

- Code, comments and docs are in English. Commits are Conventional Commits in English
  (`feat(core): ...`, `fix(agents): ...`), with a subject in the imperative, lowercase, no final period.
- A test names a behavior that would hurt a user if it broke. Tests that restate the code are not
  wanted. The default suite stays under 30 seconds and calls no real agent.
- `platform/` is the only place that tells Linux from Windows. Windows code is compiled by CI; to lint
  it locally run `rustup target add x86_64-pc-windows-gnu` once, and `verify.sh` does the rest.
- A new dependency needs a reason in the commit message.
- No telemetry, no network request of our own, no analytics. Not even opt-in.

## Adding an agent

An agent is one file in `crates/core/src/agents/` that knows how to build the command line, read
the CLI's output and ask it what it is. Add a recorded output of the real CLI as a test fixture.

## Reporting a problem

Use the issue templates. For a security problem, read [SECURITY.md](SECURITY.md) instead.
