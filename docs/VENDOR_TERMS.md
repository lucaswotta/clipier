# What the agent vendors' terms say about driving their CLIs

Read on 2026-10-18. Not legal advice; the vendors' pages are the source and they change.

Leme Pier starts each vendor's official CLI, unmodified, as a subprocess. The CLI keeps its own sign-in.
Leme Pier never reads, copies or stores a vendor credential, and it does not call any model API.

## Anthropic (`claude`)

Source: [Claude Code legal and compliance](https://code.claude.com/docs/en/legal-and-compliance).

- Third-party developers may not "offer Claude.ai login into their own applications, or route requests
  through Free, Pro, or Max plan credentials on behalf of their users". Leme Pier does neither.
- The same page says the rule does not prevent "an end user from signing in to the unmodified Claude Code
  binary with their own Claude subscription".
- "Advertised usage limits for Pro and Max plans assume ordinary, individual usage". A Race with four boats
  or a Crew with parallel pieces spends allowance faster than a single chat. The interface shows what each
  agent used.
- The names and logos of Claude Code and Anthropic may not be part of a product name or logo.

## OpenAI (`codex`)

Source: [Codex authentication](https://learn.chatgpt.com/docs/auth).

- The documentation recommends API key authentication for programmatic workflows such as CI/CD, and says
  access tokens are for "trusted scripts, schedulers, and private CI runners".
- It does not forbid using the ChatGPT sign-in through another program, and it says to treat
  `~/.codex/auth.json` like a password. Leme Pier does not read that file.
- The consumer [Terms of Use](https://openai.com/policies/row-terms-of-use/) bar automatically or
  programmatically extracting data or output. No official statement was found on how this applies to the
  Codex CLI.

## Google (`agy`)

Source: [Antigravity terms](https://antigravity.google/terms?hl=en), section 6.

- "Using third party software, tools, or services to access the Service (e.g. using OpenClaw with Antigravity
  OAuth) is a breach of this Agreement", and may lead to suspension of the account.
- No official statement was found on whether starting the official `agy` binary counts. A forum
  [reply](https://discuss.ai.google.dev/t/is-orchestrating-the-official-antigravity-cli-from-a-local-multi-agent-coding-application-allowed/181088)
  from a user (not identified as Google staff) says it is allowed for single-user local use.
- This is the least certain of the four. Use `agy` through Leme Pier at your own risk.

## opencode

`opencode` is not a vendor account. What applies is the terms of the provider whose account you sign in with.
Anthropic's rule above excludes Free, Pro and Max sign-ins through third-party programs.

## What Leme Pier does about it

- It starts the CLIs unmodified and leaves authentication to them.
- It does not collect, store or forward vendor credentials.
- Its one credential is a GitHub token, kept in `~/.config/leme-pier/github-token` ([docs/GITHUB.md](GITHUB.md)).
