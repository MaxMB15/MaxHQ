# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub. Open the repository's Security tab and click "Report a vulnerability". Don't open a public issue or pull request for them.

A useful report says:

- what an attacker can do, and what they need first (a network position, a paired phone, a shell on one of your devices)
- the commit you tested
- steps or a test that shows it

I read every report, but I can't promise a reply time. Once I've looked at it, I'll tell you what I plan to do.

## Supported versions

MaxHQ is pre-release and has no releases yet. Only the latest commit on `main` gets fixes.

## Scope

These are in scope:

- `maxd`: its local socket, `maxd proxy` and the key checks behind it, links between devices, the network access endpoints for phones, the webhook listener, and the guest `maxd` in container workspaces
- the Mac app, including what its web view can reach on disk and the program admission prompts
- the iPhone app and the Rust core it shares with the Mac app
- pairing a phone, network access tokens, and the TLS pinning between them
- adding a device: the host key check, the `maxd` it copies there, and the forced-command key it adds to `authorized_keys`
- agent tools that start or message other agents, and the permission checks on them
- `maxd host apply`, `maxd service install` and the NixOS module

## What MaxHQ does by design

The agents MaxHQ starts are Claude Code, Codex, Cursor and other agent CLIs. They read files, run commands and reach the network with the permissions of the user that runs them, on every device you add. That's what they're for. Each CLI's own permission settings decide what it may do without asking, and MaxHQ passes its prompts on to you. An agent doing what its settings allow isn't a MaxHQ vulnerability. MaxHQ letting an agent or another program past a check it should have stopped at is one.

Problems in the agent CLIs themselves, or in OpenSSH, Tailscale, Incus or macOS, belong with those projects. If MaxHQ uses one of them in an unsafe way, that's in scope here.
