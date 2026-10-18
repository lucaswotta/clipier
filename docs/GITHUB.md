# Opening a pull request from Clipier

A finished boat (Race) and a finished Crew show an **Open a pull request** button. It sends the branch
that holds the work to `origin` and opens a pull request into the branch your checkout was on. Nothing
is sent until you click, and the text you see (title and description) is what is sent.

It is the only network request Clipier makes itself, and it goes to `github.com` and
`api.github.com` only.

## Signing in

Clipier looks for a sign-in in this order:

1. **The device flow**, if an OAuth App is set up (below). You open an address, type a code, and GitHub
   hands Clipier a token, which is kept in `~/.config/clipier/github-token`, readable by you only. The
   button *Sign out of GitHub* deletes it.
2. **Your own `gh` sign-in**, if the [GitHub CLI](https://cli.github.com) is installed and signed in
   (`gh auth login`). Clipier asks `gh auth token` for it when it needs it and keeps nothing.

If neither exists, the panel says so and shows what to do.

## Setting up the OAuth App (once)

The device flow needs an OAuth App, so that GitHub shows *Clipier* when it asks you to approve.

1. GitHub → Settings → Developer settings → OAuth Apps → *New OAuth App*.
2. Name: `Clipier`. Homepage: this repository. Authorization callback URL: any (the device flow does
   not use it, but the form needs one; `http://localhost` is fine).
3. After creating it, tick **Enable Device Flow** and save.
4. Copy the *Client ID* (it is not a secret).
5. Give it to Clipier: either set `CLIPIER_GITHUB_CLIENT_ID` in the environment when you start it, or
   build with it (`CLIPIER_GITHUB_CLIENT_ID=... npx tauri build`) so that it is part of the program.

The app asks for the `repo` scope, which is what GitHub requires to open a pull request in a private
repository. It can read and write the repositories you can; Clipier only uses it to push one branch whose
name starts with `clipier/` and to open a pull request.

## What is and is not sent

Sent: the branch, and a pull request with the title, description and branch names shown on screen.
Not sent: your conversations, your notes, your usage log, or anything about your machine.
Clipier refuses to push a branch that does not start with `clipier/`.
