# Releasing MaxHQ

A release is a `vX.Y.Z` tag on `main`. The tag builds the Mac app for Apple silicon and Intel, a static `maxd` for both Linux CPUs, and the files the app's updater reads, and puts them all on a draft GitHub release. Installed apps see a release only once the draft is published.

The version lives in one place: `version` under `[workspace.package]` in the root `Cargo.toml`. Every crate and the app take it from there.

## Make a release

1. Merge what you want to ship into `dev` and push. Wait for CI and Linux binaries to pass on that commit, or let the pipeline wait for them.
2. In Actions, open **Release pipeline** and click **Run workflow**. Pick:
    - **release_type**: `patch`, `minor` or `major`.
    - **dry_run**: does everything except push, and prints the tag it would make. Use it the first time.
    - **publish**: tick it to publish the release as soon as it's built. Leave it off to get a draft you check first.
3. The pipeline:
    1. checks you're allowed to release,
    2. pins dev's current commit, so later pushes to dev aren't included,
    3. waits up to 90 minutes for the CI and Linux binaries runs on that commit, and stops if either failed,
    4. merges the commit into `main`, bumps the version, commits and tags `vX.Y.Z`,
    5. pushes `main` and the tag together, so neither lands without the other,
    6. links the release run in its summary.
4. The tag starts **Release**. The Linux, Mac and iPhone builds don't wait. Once the Linux builds and a Mac build have passed, that Mac's signing job waits for you to approve the `macos-signing` environment: open the run and click **Review deployments**, tick it, then **Approve and deploy**. Approve both within a week, while the Mac builds are still kept.
5. Once every build is signed, the TestFlight upload waits for another approval of `macos-signing`. Click **Review deployments** again, tick it, then **Approve and deploy**. Approve it within a day, while the iPhone build is still kept.
6. When the run finishes:
    - With **publish** ticked, the release is published and marked latest.
    - Without it, open the draft on the Releases page, check the notes and the assets, and click **Publish release**.
7. The last job merges `main` back into `dev`, so dev has the version bump. If that merge conflicts, it opens a pull request instead.

## What the Release run does

```
prepare-release ─┬─> linux-maxd (x86_64, aarch64) ─┬─> sign-macos (aarch64, x64) ─┬─> release-testflight ─> publish-latest-json ─> sync-dev
                 ├─> build-macos (aarch64, x64) ───┘                              │
                 └─> build-ios (iPhone archive) ──────────────────────────────────┘
```

| Job                   | Runs on                             | What it does                                                                                                                                                                                                                                                                                                                                  |
| --------------------- | ----------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `prepare-release`     | Ubuntu                              | Checks the tag matches `Cargo.toml`, reads whether the tag asks to publish and which step of a [change of the update key](#change-the-update-key) this release is, and creates the draft with generated notes.                                                                                                                                |
| `linux-maxd`          | Ubuntu x86_64 and Ubuntu arm64      | Builds a static musl `maxd` on each CPU and checks it's static. These jobs get no secrets.                                                                                                                                                                                                                                                    |
| `build-macos`         | macOS 15 on Apple silicon and Intel | Builds `maxd`, `maxd-vm` and the unsigned app, and hands them to `sign-macos` as a workflow artifact. These jobs get no secrets and no token that can write.                                                                                                                                                                                  |
| `sign-macos`          | macOS 15 on Apple silicon and Intel | Puts the Linux `maxd` in the bundle, signs, notarizes, makes the DMG and the updater archive, and uploads them. The Apple silicon job also signs the model catalog. It installs no package and runs nothing the build made: only Apple's tools and the release scripts, which need nothing but Node. Runs in the `macos-signing` environment. |
| `build-ios`           | macOS 26                            | Builds the phone's Rust core, prebuilds and archives the iPhone app with no App Store Connect key, and keeps the archive for a day. Its build number comes from the run, so it only grows. It gets no secrets and no token that can write.                                                                                                    |
| `release-testflight`  | macOS 26                            | Once the Linux, Mac and iPhone builds have all passed, exports the kept archive as an `.ipa`, signed by Xcode through an App Store Connect API key, and uploads it to TestFlight with Apple's `altool`. It installs no package. Runs in the `macos-signing` environment.                                                                      |
| `publish-latest-json` | Ubuntu                              | After the TestFlight upload, uploads the Linux `maxd` and `SHA256SUMS`, writes the update manifests from both Macs' updater signatures, uploads the signed model catalog, writes `mobile.json` and adds the iPhone build to the notes, and publishes the draft when the tag asked for it.                                                     |
| `sync-dev`            | Ubuntu                              | Merges `main` into `dev`.                                                                                                                                                                                                                                                                                                                     |

`scripts/release/package-macos.sh` runs in two parts. `build-macos` runs its `build`, with no Apple or update secret in its environment. `sign-macos` runs its `assemble` on that build, which signs in this order: `maxd` and `maxd-vm`, then `maxd-release.json` (the sums of every `maxd` in the bundle) with the update key, then the app, then notarizes and staples the app, then makes the DMG and signs, notarizes and staples it, and last makes the updater `.app.tar.gz` and signs it with the update key.

A release is all or nothing. The iPhone builds beside the Macs, so the release takes no longer, but it reaches TestFlight only once every Linux, Mac and iPhone build has passed, and the draft is published only after that upload. When any build fails, or you turn an approval down, nothing ships: not in TestFlight and not on GitHub. Mac assets already on the draft stay unpublished. **Re-run failed jobs** finishes the same release once the cause is fixed.

The builds travel to the jobs that sign them as workflow artifacts: each Mac build from `build-macos` to `sign-macos`, kept for a week, and the iPhone archive from `build-ios` to `release-testflight`, kept for one day. The jobs that build get no secret, so nothing a dependency runs during a build can read a key. `sign-macos` unpacks the build outside its checkout, so nothing in it can replace a script the job runs. The signing settings and the keys never leave a job: each job that needs a key writes it from the environment secret and removes it at the end, even when a step failed.

The publish choice travels in the tag's message. When the pipeline's **publish** box is ticked, the tag's message ends with the line `Publish-Release: true`, and `publish-latest-json` publishes the draft only when it finds that line. A tag pushed by hand, or a lightweight tag, always leaves a draft.

## Release assets

| File                                     | What it is                                                                                                                             |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `MaxHQ_aarch64.dmg`                      | The app for Apple silicon Macs. The README links to it.                                                                                |
| `MaxHQ_x64.dmg`                          | The app for Intel Macs.                                                                                                                |
| `MaxHQ_<version>_aarch64.app.tar.gz`     | The Apple silicon app as the updater downloads it.                                                                                     |
| `MaxHQ_<version>_aarch64.app.tar.gz.sig` | Its signature from the update key.                                                                                                     |
| `MaxHQ_<version>_x64.app.tar.gz`         | The Intel app as the updater downloads it.                                                                                             |
| `MaxHQ_<version>_x64.app.tar.gz.sig`     | Its signature from the update key.                                                                                                     |
| `maxd-x86_64-unknown-linux-musl`         | Static `maxd` for x86_64 Linux. The app carries the same file.                                                                         |
| `maxd-aarch64-unknown-linux-musl`        | Static `maxd` for arm64 Linux. The app carries the same file.                                                                          |
| `SHA256SUMS`                             | The sums of the two Linux binaries, in `sha256sum` format.                                                                             |
| `latest.json`                            | What the installed app's updater reads: the version, notes, and each Mac's archive URL and signature.                                  |
| `latest-v2.json`                         | The same, for apps from a [change of the update key](#change-the-update-key) on. Releases before the change don't have it.             |
| `model-catalog.json`                     | The local model catalog every `maxd` refreshes from.                                                                                   |
| `model-catalog.json.sig`                 | Its signature from the update key. `maxd` uses a refreshed catalog only when it checks out, and keeps the one built into it otherwise. |
| `mobile.json`                            | What the iPhone app reads to offer a newer TestFlight build: the version, build number, protocol range, notes link and date.           |

The DMG names have no version in them, so `releases/latest/download/MaxHQ_aarch64.dmg` always points at the newest published release.

Inside the app, `Contents/MacOS` holds `maxhq-desktop`, `maxd` and `maxd-vm`, and `Contents/Resources` holds both Linux binaries, `SHA256SUMS`, `maxd-release.json` and `maxd-release.json.sig`.

## Secrets and settings

Nothing here is set up for you. Add them under the repository's **Settings**.

| Name                                      | Where                              | What it is                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ----------------------------------------- | ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `RELEASE_AUTOMATION_PAT`                  | Repository secret                  | A fine-grained personal access token for this repository only, with **Contents**, **Pull requests** and **Workflows** set to read and write. The pipeline pushes `main` and the tag with it. `GITHUB_TOKEN` can't do this job: its pushes start no workflows, so the tag would never build, and it can't push commits that change files in `.github/workflows`.                                                                                                            |
| `RELEASE_ALLOWED_ACTORS`                  | Repository variable                | Optional. GitHub user names allowed to run the release workflows, separated by commas. Without it, only the repository's owner can.                                                                                                                                                                                                                                                                                                                                        |
| `TAURI_SIGNING_PRIVATE_KEY`               | `macos-signing` environment secret | The update key from `tauri signer generate`, as the file's contents. It signs the updater archives and `maxd-release.json`. The app checks both with the public key built into it, so losing this key means installed apps can't update. Keep a copy offline.                                                                                                                                                                                                              |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`      | `macos-signing` environment secret | The key's password, if it has one.                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `TAURI_SIGNING_PRIVATE_KEY_NEXT`          | `macos-signing` environment secret | Only during a [change of the update key](#change-the-update-key): the new key, as the file's contents.                                                                                                                                                                                                                                                                                                                                                                     |
| `TAURI_SIGNING_PRIVATE_KEY_NEXT_PASSWORD` | `macos-signing` environment secret | The new key's password.                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `APPLE_CERTIFICATE`                       | `macos-signing` environment secret | The Developer ID Application certificate and its private key, exported as a `.p12` and base64-encoded (`base64 -i cert.p12`).                                                                                                                                                                                                                                                                                                                                              |
| `APPLE_CERTIFICATE_PASSWORD`              | `macos-signing` environment secret | The password the `.p12` was exported with.                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `APPLE_API_KEY_ID`                        | `macos-signing` environment secret | The ID of an App Store Connect API key with the Developer role, used to notarize.                                                                                                                                                                                                                                                                                                                                                                                          |
| `APPLE_API_ISSUER`                        | `macos-signing` environment secret | That key's issuer ID.                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `APPLE_API_PRIVATE_KEY`                   | `macos-signing` environment secret | The contents of that key's `AuthKey_<id>.p8` file.                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `APPLE_APP_MANAGER_KEY_ID`                | `macos-signing` environment secret | The ID of a second App Store Connect API key, a team key with the Admin role. The iPhone job signs and uploads with it. Xcode makes the distribution certificate in Apple's cloud through it, which Apple allows only for Admin: App Manager fails with "Cloud signing permission error", and the Developer key above can't make App Store distribution signing either. The name stays from an earlier setup. Its issuer is the team's, so `APPLE_API_ISSUER` serves both. |
| `APPLE_APP_MANAGER_PRIVATE_KEY`           | `macos-signing` environment secret | The contents of that key's `AuthKey_<id>.p8` file.                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `IOS_TEAM_ID`                             | Repository variable                | The 10-character Apple Team ID the iPhone app is signed for.                                                                                                                                                                                                                                                                                                                                                                                                               |
| `IOS_BUNDLE_ID`                           | Repository variable                | The iPhone app's bundle ID, as its App Store Connect record has it.                                                                                                                                                                                                                                                                                                                                                                                                        |
| `IOS_APP_STORE_ID`                        | Repository variable                | Optional. The app's Apple ID from its App Store Connect record, all digits. The phone's Update in TestFlight button opens the app's page with it, and opens TestFlight itself without it.                                                                                                                                                                                                                                                                                  |

Create the `macos-signing` environment under **Settings > Environments**, add yourself as a required reviewer, and limit its deployment branches and tags to `v*` tags. The update key goes there and never in plain repository secrets, so only an approved signing job on a release tag can read it. Inside that job, only the steps that check or sign with the key get it.

Without the Apple secrets the Mac signing jobs still run, sign ad hoc and skip notarizing. The iPhone build stops at its first step when a variable is missing, and the TestFlight upload stops at its first step when its key is missing, after the builds; either way nothing ships. A Mac build signed ad hoc won't open on another Mac without a right-click, so don't publish it. The update key is always required.

## Change the update key

The app's updater trusts one public key, the one built into it, so moving to a new update key takes two releases. `maxd` trusts a list: `RELEASE_PUBLIC_KEY` in `crates/daemon/src/fleet/release.rs`, and the key in `crates/daemon/src/fleet/next-release-key.pub` while that file holds one. `prepare-release` reads which step each release is and prints it in the run's summary, so every step below is an ordinary release.

1. Make the new key, with a password: `pnpm --filter maxhq-desktop tauri signer generate -w <path to the new key>`. Keep a copy offline, as with the current key.
2. Put the contents of its `.pub` file, as they are, on the line under the comments in `crates/daemon/src/fleet/next-release-key.pub`, and merge that into `dev`.
3. Add the new key and its password to the `macos-signing` environment as `TAURI_SIGNING_PRIVATE_KEY_NEXT` and `TAURI_SIGNING_PRIVATE_KEY_NEXT_PASSWORD`. Keep the current key's secrets.
4. Make a release: the bridge. It's signed with the current key, so every installed app takes it, but the app in it trusts the new key and reads `latest-v2.json`. The bridge writes both `latest.json` and `latest-v2.json`. Publish it and keep it: `latest.json` stays at the bridge from now on, so an app that skips it still comes to it first. Then run **Update everything** in the app, so every device's `maxd` trusts the new key too.
5. Make the next release. It finds `latest-v2.json` on the bridge, signs everything with the new key, writes `latest-v2.json`, and carries the bridge's `latest.json` forward unchanged.
6. Once your devices run a release from after the bridge, finish the change: put the new public key in `RELEASE_PUBLIC_KEY` and in `plugins.updater.pubkey` in `apps/desktop/src-tauri/tauri.conf.json`, empty `next-release-key.pub` down to its comments, store the new key and its password under `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and delete the `_NEXT` secrets. Releases from then on sign with the new key and keep writing `latest-v2.json`, and `maxd` trusts only the new key.

While the change runs:

- A device whose `maxd` is older than the bridge doesn't trust the new key. It refuses `maxd` from releases after the bridge until its `maxd` is replaced by the bridge's or a later one, and it keeps the model catalog built into it.
- `sign-macos` checks each key against the public key it must match before it signs anything, and `publish-latest-json` checks that every signature in a manifest is by the key that manifest's readers trust.
- Don't delete the bridge release, or `latest.json` on later releases: apps that haven't taken the bridge read it.

## When something fails

- **The pipeline stopped while waiting for CI.** Fix the failure on dev, push, and run the pipeline again. Nothing was pushed.
- **The pipeline failed before pushing.** Nothing changed on GitHub. Fix the cause and run it again.
- **The push failed.** It's atomic, so neither `main` nor the tag moved. Run the pipeline again.
- **A Release job failed after the tag was pushed.** Open the run and click **Re-run failed jobs**. Uploads replace files already on the draft, so re-running is safe. If the code needs a fix, delete the draft and the tag (`git push origin :refs/tags/vX.Y.Z`), fix on dev, and release again. The next run bumps from the version on `main`, so you may skip a number; that's fine.
- **The Mac signing jobs or the TestFlight upload are waiting.** They need your approval of `macos-signing`; see steps 4 and 5 above.
- **The iPhone build failed.** Nothing ships until it passes. Fix the cause, then **Re-run failed jobs**: the new attempt gets a higher build number, and the TestFlight upload and the publishing follow it.
- **A Linux or Mac build failed after the iPhone build passed.** The iPhone build waits; it isn't in TestFlight. **Re-run failed jobs** rebuilds what failed, then exports and uploads the kept iPhone archive. When more than a day has passed, the kept archive is gone: re-run `build-ios` on its own instead (**Re-run this job**), which builds it again under a higher number, then uploads and publishes. A Mac build is kept for a week; after that, re-run `build-macos` the same way.
- **The TestFlight upload failed.** **Re-run failed jobs** exports and uploads the same build again. When App Store Connect says it already has that build number, re-run `build-ios` instead, as above.
- **`publish-latest-json` says the last release has no `latest.json` to carry forward.** During a [change of the update key](#change-the-update-key), each release copies `latest.json` from the release before it, and that one lost it. Download `latest.json` from the bridge release, upload it to the latest release with `gh release upload`, then **Re-run failed jobs**.
- **Notarization was rejected.** The job prints Apple's log. Usually a binary is missing the hardened runtime or a timestamp; `scripts/release/verify-signing.sh` names it.
- **`main` was bumped but no tag exists.** Run **Release tag**. It tags `main`'s head with the version in `Cargo.toml` and pushes the tag, with the same **publish** box.
- **You want to release `main` as it is, without merging dev.** Run **Release bump**, then **Release tag**.
- **`sync-dev` opened a pull request.** Resolve the conflicts in it and merge it into dev.
- **Check a published release.** `sh scripts/release/validate-release.sh` downloads `latest.json`, and `latest-v2.json` when the release has one, and checks every URL in them, the DMGs, the Linux binaries, `SHA256SUMS`, and the model catalog and its `.sig`.

## Build a release locally

`scripts/release/package-macos.sh` does on your Mac what the two Mac jobs do. With no identity it signs ad hoc, and it skips Linux binaries it can't find with a warning:

```sh
TAURI_SIGNING_PRIVATE_KEY_PATH=<path to your update key> MAXHQ_RELEASES_REPO=<owner>/<repo> \
  sh scripts/release/package-macos.sh --linux-dir <folder with the Linux maxd>
```

`MAXHQ_RELEASES_REPO` names the repository whose releases the app updates from. The build writes its `latest.json` link into the app, so no tracked file names the owner; without it the app can't look for updates.

`pnpm --filter maxhq-mobile mobile:testflight` uploads an iPhone build from your Mac under the number in `apps/mobile/native-build.json`, signed through the account in Xcode, or through an API key when `MAXHQ_ASC_KEY_PATH`, `MAXHQ_ASC_KEY_ID` and `MAXHQ_ASC_ISSUER_ID` are set. Local numbers stay below 10000 and the release job's start at 10000, so the two never meet.

Set `MAXHQ_SIGN_IDENTITY` to sign with a Developer ID from your keychain, and add `--notarize` with `APPLE_API_KEY`, `APPLE_API_ISSUER` and `APPLE_API_KEY_PATH` set to notarize. `MAXHQ_UNSIGNED_UPDATES=1` skips the update key entirely, for a build you only run yourself; Update everything then asks before it sends that build's maxd to other devices. The files land in `target/release/bundle/release`.
