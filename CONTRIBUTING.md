# Contributing

Bug reports and pull requests are welcome. For a large change, open an issue first so we can agree on the approach before you spend time on it. For a security problem, follow [SECURITY.md](SECURITY.md) instead of opening an issue.

## Setup

You need the tools listed under Requirements in the [README](README.md). Then:

```sh
corepack enable
pnpm install
```

`pnpm install` runs `scripts/install-hooks.sh`, which sets `core.hooksPath` to `.githooks` in your clone. Check it with `git config core.hooksPath`.

The Rust tests build on macOS and Linux. On Linux the desktop crate links WebKitGTK, so install the packages that `.github/actions/install-linux-deps/action.yml` lists first. The app itself targets macOS only.

## The pre-commit hook

The hook is required. Don't commit with `--no-verify`, and don't turn it off in your git config. It runs these steps in order and stops at the first that fails:

1. `scripts/check-ai-files.sh` refuses any staged file that `.gitignore` excludes. That covers agent instruction and settings files such as `CLAUDE.md`, `AGENTS.md`, `.claude/`, `.codex/` and `.cursor/`, even when they're added with `git add -f`. Keep those files in your clone and out of commits.
2. `scripts/check-license-headers.sh` refuses any file that still carries the proprietary license MaxHQ had before it became MIT licensed. A branch started before the switch can bring one back when it merges.
3. If your clone has a `docs/private-patterns.txt`, the hook refuses added lines that match any pattern in it, one extended regex per line. Use it for your own host names, network names and paths. `docs/` is ignored, so a fresh clone has no such file and skips this step.
4. lint-staged runs Prettier, ESLint with `--fix`, and rustfmt on the staged files.
5. `pnpm typecheck` and `cargo clippy --workspace --all-targets --locked -- -D warnings` check the whole tree.
6. `pnpm test` runs every test: the Rust workspace, then the desktop, phone, shared and script tests.

The hook checks your working tree, not only what you staged. Don't edit files in the clone until it finishes. A full run takes a few minutes.

## CI

Every push to `main` or `dev`, and every pull request, runs these jobs:

- `ai-files` runs the same checks as steps 1 and 2 of the hook.
- `rust` runs `cargo fmt --check`, clippy with warnings as errors, and `cargo test --workspace --locked` on Ubuntu.
- `rust-macos` runs `cargo test --workspace --locked` on macOS.
- `frontend` runs `pnpm typecheck`, `pnpm lint`, `pnpm format:check` and the desktop, phone, shared and script tests. `pnpm lint` first checks that every English string has a translation in each of the other four languages, with the same placeholders.
- `licenses` runs cargo-deny's license, advisory and source checks, and `node scripts/check-licenses.mjs`.
- The Linux binaries workflow builds a static `maxd` for `x86_64-unknown-linux-musl` and uploads it as an artifact.

CI can't run the private names check, since `docs/private-patterns.txt` stays on your machine. CI runs clippy on Linux only. The hook runs it on your machine, so on a Mac it also covers code behind `cfg(target_os = "macos")`.

## Changing the protocol

`crates/protocol` holds the types that `maxd`, the Mac app and the phone app send each other. The TypeScript bindings in `packages/shared/src/bindings.ts` and the JSON Schemas in `crates/protocol/schema` are generated from them. After you change a type, regenerate them:

```sh
pnpm bindings
```

The tests fail while the committed files don't match the Rust types.

Devices often run a different commit from the app, so a protocol change has to say who can still read it. The comment above `PROTOCOL_VERSION` in `crates/protocol/src/lib.rs` has the rules. In short:

- An additive change, such as a new message or a new field with `#[serde(default)]`, bumps `PROTOCOL_VERSION`.
- A breaking change, such as a renamed or removed field or a field with no default, bumps `OLDEST_PROTOCOL_VERSION` too.

## Tests

- Use placeholders for every host, user, network and path, such as `devbox.example.ts.net`, `hooks.example.com`, `/home/me` and `/Users/me`. Never put a real host name, IP address, token or account in a test.
- Tests don't reach the internet and don't need a signed-in agent CLI. `crates/fake-agent` stands in for one.
- A test that has to read the real machine, such as its processes, its agent sessions or the Tailscale client, gets `#[ignore = "<why>"]` so `cargo test` skips it. Run one on purpose with `cargo test -p <crate> <name> -- --ignored`.
- A test that runs git clears the `GIT_*` variables first. The hook sets some of them, and a test that inherits them can change your clone's git config.

## Dependencies

- Each new dependency's license has to be on the allowlist in `deny.toml`, which `scripts/check-licenses.mjs` repeats for npm. `pnpm check:licenses` runs both. It needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny).
- pnpm only installs versions that are at least three days old, and runs no install scripts unless `pnpm-workspace.yaml` lists the package under `allowBuilds`.
- Rust crates come from crates.io only.

## Commit messages

Commits follow the conventional commit style, with a lowercase summary in plain words:

```
fix(maxd): show a process's new command after it execs

The Mac's scanner kept each process's command line by pid and start, and
an exec keeps both. A shell that exec'd into a server went on showing as
the shell. The name changes on exec, so a new name now reads the command
line again. The live process-list test waits for the new commands too.
```

Common types are `feat`, `fix`, `test`, `refactor`, `docs`, `chore` and `ci`. The scope names the part you changed, such as `maxd`, `daemon`, `app`, `desktop`, `mobile`, `protocol`, `core` or `shared`. The body says what changed and why, wrapped at 72 columns. Keep one change per commit.

## File headers

New source files start with these two lines, in the comment style of the file type:

```
// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
```

Markdown and JSON files go without. If you wrote a new file and want your name on it too, add your own copyright line under that one. If you copy code from another project, keep its license notice at the top of the file and add the project to [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).

## License

MaxHQ is under the [MIT License](LICENSE). By sending a pull request, you agree that your contribution is under the same license. There's no contributor license agreement to sign. Changes to `spec/workflow` are under the Apache License 2.0, like the rest of that folder.
