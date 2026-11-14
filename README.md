# MaxHQ

MaxHQ is a Mac app for running coding agents on several machines at once. You start, watch and answer Claude Code, Codex and Cursor agents from one window, whether they run on your Mac, on a Linux server, or in Incus containers on that server. An iPhone app shows the same agents and lets you answer them when you're away from the Mac.

A daemon called `maxd` runs on every device. It starts the agent CLIs, keeps them running after you close the app, and tells the apps what they're doing. The apps reach each `maxd` over SSH.

## Status

MaxHQ is pre-release. Signed builds come out as GitHub releases, or you can build it from source. The protocol between the apps and `maxd` still changes often, and so do the screens.

## Download

The latest release has a signed, notarized app for each Mac:

- Apple silicon: [MaxHQ_aarch64.dmg](../../releases/latest/download/MaxHQ_aarch64.dmg)
- Intel: [MaxHQ_x64.dmg](../../releases/latest/download/MaxHQ_x64.dmg)

Open the DMG and drag MaxHQ to Applications. The app carries `maxd` for the Mac and a static `maxd` for x86_64 and arm64 Linux, which it copies to the Linux devices you add. The same Linux binaries are on each release as `maxd-x86_64-unknown-linux-musl` and `maxd-aarch64-unknown-linux-musl`, with their sums in `SHA256SUMS`. The app checks for updates against the latest release.

## What it does

- Runs Claude Code, Codex and Cursor agents. Any other agent that speaks the Agent Client Protocol (ACP) works too, added as a local command or installed from the ACP registry.
- Keeps each agent's CLI alive in a small holder process, so agents keep working when the app quits or `maxd` restarts.
- Shows each agent's messages and tool calls, and puts its permission prompts in front of you to allow or deny.
- Shows how much of their rate limits your Claude and Codex accounts have used. Cursor usage is off by default, since it reads Cursor's dashboard API.
- Adds a Linux machine from your SSH config. The app checks the host key with you, copies a static `maxd` there, and adds a key to `authorized_keys` whose forced command allows only `maxd proxy`.
- Creates container workspaces on a Linux host with Incus, with start, stop, snapshots and egress rules.
- Checks a Linux host against one spec file, `maxhq-host.toml`, and fixes what it can (`maxd host check`, `maxd host apply`). On NixOS the module in `nix/` reads the same file.
- Opens a project in Cursor or VS Code. A project in a container opens through Remote-SSH.
- Finds ports an agent's programs listen on and forwards them to your Mac. Uploads and downloads files and folders.
- Keeps notes per project, synced between your devices.
- Lists GitHub pull requests, links them to agents, and approves or merges them. This needs a GitHub sign-in or a GitHub App you set up.
- Runs scheduled tasks on a timer, from a webhook, or on GitHub events.
- Lets an agent start, message, wait on and stop other agents, on its own device or another one, once you allow it. `maxd mcp` serves the same tools to other programs over MCP.
- Lends test machines to agents: Linux containers, macOS VMs, the iOS Simulator and the Android emulator.
- Keeps a library of agent skills and copies them into projects. Changes come to you as proposals.
- Draws workflow runs that follow the `maxhq.workflow` spec in `spec/workflow`. It reads their files and never runs their scripts.
- Has a dispatcher agent on the Mac that turns a plain request into a plan, and runs the plan only after you confirm it.
- Can keep the Mac awake while agents work.

The iPhone app pairs with the Mac by QR code. It reaches your devices over SSH, or over TLS pinned at pairing when you turn on a device's network access. It can notify you when an agent asks for permission, and you can answer from the notification. Push notifications need your own APNs key.

Tailscale is optional. If you use it, adding a device lists your tailnet's machines, and webhooks can use Tailscale Funnel. Everything works without it over plain SSH.

## How it fits together

```
MaxHQ app (Mac) ── Unix socket ──> maxd on the Mac ──> agent CLIs
       │
       └──────────── SSH ────────> maxd on a Linux host ──> agent CLIs
                                        └─> Incus containers, each with a guest maxd

iPhone app ── SSH, or pinned TLS ──> maxd on each device
```

The app runs `maxd proxy` on each host through your system's `ssh`, so your SSH config, keys and jump hosts apply. Devices also link to each other's `maxd`, which is how notes sync and how an agent reaches agents on another device.

| Path                | What it holds                                                                     |
| ------------------- | --------------------------------------------------------------------------------- |
| `apps/desktop`      | The Mac app: Tauri 2, React and TypeScript                                        |
| `apps/mobile`       | The iPhone app: Expo and React Native, calling the Rust core through UniFFI       |
| `packages/shared`   | TypeScript both apps use, including the bindings generated from the Rust types    |
| `crates/daemon`     | `maxd`                                                                            |
| `crates/app`        | What both apps do apart from their windows                                        |
| `crates/client`     | Connecting to `maxd` over SSH, a socket or TLS, and setting up a new device       |
| `crates/core`       | Logic and storage shared by `maxd` and the app                                    |
| `crates/protocol`   | The wire types, which the TypeScript bindings and JSON Schemas are generated from |
| `crates/hold`       | The holder that keeps one agent CLI alive across `maxd` restarts                  |
| `crates/wire`       | One writer per connection, so control messages overtake file data                 |
| `crates/transfer`   | File and folder transfers that stay inside their folder                           |
| `crates/term`       | Headless terminal emulation                                                       |
| `crates/vm`         | The macOS test VM helper                                                          |
| `crates/workflow`   | The `maxhq.workflow` reader                                                       |
| `crates/mobile`     | The phone app's Rust side                                                         |
| `crates/fake-agent` | A stand-in agent CLI for tests and benchmarks                                     |
| `nix`               | A flake with `maxd`, a NixOS module and its VM test                               |
| `spec/workflow`     | The `maxhq.workflow` spec and its JSON Schemas                                    |

## Requirements

- macOS 15 or later, with the Xcode Command Line Tools.
- Rust stable, through [rustup](https://rustup.rs).
- Node 24 and pnpm 12. Run `corepack enable` and pnpm picks up the version in `package.json`.
- The agent CLIs you want, signed in on each device. A device's page in the app can install and update them.
- For a Linux host, x86_64 or aarch64 Linux with systemd. `maxd` runs as a systemd user service. Turn on lingering (`sudo loginctl enable-linger <user>`) to keep it running after you log out. The app tells you when it's off.
- For container workspaces, Incus on that host.
- For the iPhone app, Xcode, an Apple developer team and an iPhone.

## Build and run

```sh
corepack enable
pnpm install
cargo build -p maxd
pnpm dev
```

`pnpm install` also points git at the hooks in `.githooks` (see [CONTRIBUTING.md](CONTRIBUTING.md)).

`pnpm dev` runs `tauri dev`, which builds the app and serves the frontend on `localhost:1420`. On macOS the dev build runs as "MaxHQ Dev.app" inside `target/debug`, with its own bundle ID so it can show notifications without clashing with an installed copy. The app looks for `maxd` beside its own binary, so build `maxd` first. On a new Mac, open the Mac's device page and click Start maxd.

To work on the frontend alone, `pnpm dev:web` serves it in a browser with sample data and no Rust.

`pnpm build` runs `tauri build`, which makes the app without `maxd` or the Linux binaries. `scripts/release/package-macos.sh` makes the full bundle the releases ship, with both of them inside; see [.github/RELEASING.md](.github/RELEASING.md).

### Linux binaries

Adding or updating a Linux device copies a static `maxd` built for musl. A dev build of the app reads them from the folder `MAXHQ_FLEET_BIN_DIR` names. Each file is named for its target, such as `maxd-x86_64-unknown-linux-musl`. A `SHA256SUMS` file beside them is optional, and when it's there the sums must match.

Build one on Linux with musl tools installed:

```sh
cargo build --release --locked -p maxd --target x86_64-unknown-linux-musl
```

The Linux binaries workflow builds the same file for every push and pull request. Download it with `gh run download` (the artifact is `maxd-linux-<commit>`), then start the app with the folder set:

```sh
MAXHQ_FLEET_BIN_DIR=/Users/me/maxd-linux pnpm dev
```

### NixOS

The flake lives in `nix/`, so point a system flake at it with `?dir=nix`:

```nix
{
  inputs.maxhq.url = "github:<owner>/<repo>?dir=nix";

  outputs = { nixpkgs, maxhq, ... }: {
    nixosConfigurations.devbox = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        maxhq.nixosModules.default
        {
          services.maxd = {
            enable = true;
            user = "me";
            host = nixpkgs.lib.importTOML ./maxhq-host.toml;
          };
        }
      ];
    };
  };
}
```

`nix/maxhq-host.example.toml` shows every section of the host spec. `host` is optional. Without it the module only runs `maxd`.

### iPhone app

Copy `apps/mobile/ios.local.example.json` to `apps/mobile/ios.local.json` and fill in your team ID and bundle ID. Git ignores that file. Then:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
pnpm mobile:dev --simulator   # or without --simulator for a connected iPhone
```

`mobile:dev` builds the Rust core, makes a new dev build when the native parts changed, and starts Metro. `pnpm mobile:install` puts a release build on your iPhone, and `pnpm mobile:testflight` uploads one with the account signed in to Xcode. If `pod install` crashes, set `LANG=en_US.UTF-8`.

## Checks

```sh
pnpm test             # Rust tests, then the desktop, phone, shared and script tests
pnpm typecheck
pnpm lint
pnpm format:check
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
pnpm check:licenses   # needs cargo-deny
```

CI runs all of these. [CONTRIBUTING.md](CONTRIBUTING.md) has the details.

## License

MaxHQ is under the [MIT License](LICENSE). The `maxhq.workflow` spec in `spec/workflow` is under the Apache License 2.0, as `spec/workflow/LICENSE` says. [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) lists the third-party copyright notices.
