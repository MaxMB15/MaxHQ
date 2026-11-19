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
4. The tag starts **Release**. Its two Mac jobs and its iPhone build each wait for you to approve the `macos-signing` environment: open the run and click **Review deployments**, tick all three, then **Approve and deploy**. The Linux jobs don't wait.
5. Once every build has passed, the TestFlight upload waits for a second approval of `macos-signing`. Click **Review deployments** again, tick it, then **Approve and deploy**. Approve it within a day, while the iPhone build is still kept.
6. When the run finishes:
    - With **publish** ticked, the release is published and marked latest.
    - Without it, open the draft on the Releases page, check the notes and the assets, and click **Publish release**.
7. The last job merges `main` back into `dev`, so dev has the version bump. If that merge conflicts, it opens a pull request instead.

## What the Release run does

```
prepare-release ─┬─> linux-maxd (x86_64, aarch64) ─> release-macos (aarch64, x64) ─┬─> release-testflight ─> publish-latest-json ─> sync-dev
                 └─> release-ios (iPhone) ─────────────────────────────────────────┘
```

| Job                   | Runs on                             | What it does                                                                                                                                                                                                                                                             |
| --------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `prepare-release`     | Ubuntu                              | Checks the tag matches `Cargo.toml`, reads whether the tag asks to publish, and creates the draft with generated notes.                                                                                                                                                  |
| `linux-maxd`          | Ubuntu x86_64 and Ubuntu arm64      | Builds a static musl `maxd` on each CPU and checks it's static. These jobs get no secrets.                                                                                                                                                                               |
| `release-macos`       | macOS 15 on Apple silicon and Intel | Builds the app, `maxd` and `maxd-vm`, puts the Linux `maxd` in the bundle, signs, notarizes, makes the DMG and the updater archive, and uploads them. Runs in the `macos-signing` environment.                                                                           |
| `release-ios`         | macOS 26                            | Builds the phone's Rust core, prebuilds, archives and exports the iPhone app as an `.ipa`, signed by Xcode through an App Store Connect API key, and keeps it for a day. Its build number comes from the run, so it only grows. Runs in the `macos-signing` environment. |
| `release-testflight`  | macOS 26                            | Once the Linux, Mac and iPhone builds have all passed, uploads the kept `.ipa` to TestFlight with Apple's `altool`. It checks out no code. Runs in the `macos-signing` environment.                                                                                      |
| `publish-latest-json` | Ubuntu                              | After the TestFlight upload, uploads the Linux `maxd` and `SHA256SUMS`, writes `latest.json` from both Macs' updater signatures, writes `mobile.json` and adds the iPhone build to the notes, and publishes the draft when the tag asked for it.                         |
| `sync-dev`            | Ubuntu                              | Merges `main` into `dev`.                                                                                                                                                                                                                                                |

Inside each Mac job, `scripts/release/package-macos.sh` builds first, with no Apple or update secrets in its environment. Then it signs in this order: `maxd` and `maxd-vm`, then `maxd-release.json` (the sums of every `maxd` in the bundle) with the update key, then the app, then notarizes and staples the app, then makes the DMG and signs, notarizes and staples it, and last makes the updater `.app.tar.gz` and signs it with the update key.

A release is all or nothing. The iPhone builds beside the Macs, so the release takes no longer, but it reaches TestFlight only once every Linux, Mac and iPhone build has passed, and the draft is published only after that upload. When any build fails, or you turn an approval down, nothing ships: not in TestFlight and not on GitHub. Mac assets already on the draft stay unpublished. **Re-run failed jobs** finishes the same release once the cause is fixed.

The signed `.ipa` travels from `release-ios` to `release-testflight` as a workflow artifact kept for one day. The signing settings and the App Store Connect key never leave a job: each job that needs the key writes it from the environment secret and removes it at the end.

The publish choice travels in the tag's message. When the pipeline's **publish** box is ticked, the tag's message ends with the line `Publish-Release: true`, and `publish-latest-json` publishes the draft only when it finds that line. A tag pushed by hand, or a lightweight tag, always leaves a draft.

## Release assets

| File                                     | What it is                                                                                                                   |
| ---------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `MaxHQ_aarch64.dmg`                      | The app for Apple silicon Macs. The README links to it.                                                                      |
| `MaxHQ_x64.dmg`                          | The app for Intel Macs.                                                                                                      |
| `MaxHQ_<version>_aarch64.app.tar.gz`     | The Apple silicon app as the updater downloads it.                                                                           |
| `MaxHQ_<version>_aarch64.app.tar.gz.sig` | Its signature from the update key.                                                                                           |
| `MaxHQ_<version>_x64.app.tar.gz`         | The Intel app as the updater downloads it.                                                                                   |
| `MaxHQ_<version>_x64.app.tar.gz.sig`     | Its signature from the update key.                                                                                           |
| `maxd-x86_64-unknown-linux-musl`         | Static `maxd` for x86_64 Linux. The app carries the same file.                                                               |
| `maxd-aarch64-unknown-linux-musl`        | Static `maxd` for arm64 Linux. The app carries the same file.                                                                |
| `SHA256SUMS`                             | The sums of the two Linux binaries, in `sha256sum` format.                                                                   |
| `latest.json`                            | What the installed app's updater reads: the version, notes, and each Mac's archive URL and signature.                        |
| `mobile.json`                            | What the iPhone app reads to offer a newer TestFlight build: the version, build number, protocol range, notes link and date. |

The DMG names have no version in them, so `releases/latest/download/MaxHQ_aarch64.dmg` always points at the newest published release.

Inside the app, `Contents/MacOS` holds `maxhq-desktop`, `maxd` and `maxd-vm`, and `Contents/Resources` holds both Linux binaries, `SHA256SUMS`, `maxd-release.json` and `maxd-release.json.sig`.

## Secrets and settings

Nothing here is set up for you. Add them under the repository's **Settings**.

| Name                                 | Where                              | What it is                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------ | ---------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `RELEASE_AUTOMATION_PAT`             | Repository secret                  | A fine-grained personal access token for this repository only, with **Contents**, **Pull requests** and **Workflows** set to read and write. The pipeline pushes `main` and the tag with it. `GITHUB_TOKEN` can't do this job: its pushes start no workflows, so the tag would never build, and it can't push commits that change files in `.github/workflows`. |
| `RELEASE_ALLOWED_ACTORS`             | Repository variable                | Optional. GitHub user names allowed to run the release workflows, separated by commas. Without it, only the repository's owner can.                                                                                                                                                                                                                             |
| `TAURI_SIGNING_PRIVATE_KEY`          | `macos-signing` environment secret | The update key from `tauri signer generate`, as the file's contents. It signs the updater archives and `maxd-release.json`. The app checks both with the public key built into it, so losing this key means installed apps can't update. Keep a copy offline.                                                                                                   |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `macos-signing` environment secret | The key's password, if it has one.                                                                                                                                                                                                                                                                                                                              |
| `APPLE_CERTIFICATE`                  | `macos-signing` environment secret | The Developer ID Application certificate and its private key, exported as a `.p12` and base64-encoded (`base64 -i cert.p12`).                                                                                                                                                                                                                                   |
| `APPLE_CERTIFICATE_PASSWORD`         | `macos-signing` environment secret | The password the `.p12` was exported with.                                                                                                                                                                                                                                                                                                                      |
| `APPLE_API_KEY_ID`                   | `macos-signing` environment secret | The ID of an App Store Connect API key with the Developer role, used to notarize.                                                                                                                                                                                                                                                                               |
| `APPLE_API_ISSUER`                   | `macos-signing` environment secret | That key's issuer ID.                                                                                                                                                                                                                                                                                                                                           |
| `APPLE_API_PRIVATE_KEY`              | `macos-signing` environment secret | The contents of that key's `AuthKey_<id>.p8` file.                                                                                                                                                                                                                                                                                                              |
| `APPLE_APP_MANAGER_KEY_ID`           | `macos-signing` environment secret | The ID of a second App Store Connect API key, with the App Manager role. The iPhone job signs and uploads with it; the Developer key above can't make App Store distribution signing. Its issuer is the team's, so `APPLE_API_ISSUER` serves both.                                                                                                              |
| `APPLE_APP_MANAGER_PRIVATE_KEY`      | `macos-signing` environment secret | The contents of that key's `AuthKey_<id>.p8` file.                                                                                                                                                                                                                                                                                                              |
| `IOS_TEAM_ID`                        | Repository variable                | The 10-character Apple Team ID the iPhone app is signed for.                                                                                                                                                                                                                                                                                                    |
| `IOS_BUNDLE_ID`                      | Repository variable                | The iPhone app's bundle ID, as its App Store Connect record has it.                                                                                                                                                                                                                                                                                             |
| `IOS_APP_STORE_ID`                   | Repository variable                | Optional. The app's Apple ID from its App Store Connect record, all digits. The phone's Update in TestFlight button opens the app's page with it, and opens TestFlight itself without it.                                                                                                                                                                       |

Create the `macos-signing` environment under **Settings > Environments**, add yourself as a required reviewer, and limit its deployment branches and tags to `v*` tags. The update key goes there and never in plain repository secrets, so only an approved Mac job on a release tag can read it. Inside that job, only the steps that check or sign with the key get it.

Without the Apple secrets the Mac jobs still run, sign ad hoc and skip notarizing. The iPhone job stops at its first step when its key or a variable is missing, and then nothing ships. A Mac build signed ad hoc won't open on another Mac without a right-click, so don't publish it. The update key is always required.

## When something fails

- **The pipeline stopped while waiting for CI.** Fix the failure on dev, push, and run the pipeline again. Nothing was pushed.
- **The pipeline failed before pushing.** Nothing changed on GitHub. Fix the cause and run it again.
- **The push failed.** It's atomic, so neither `main` nor the tag moved. Run the pipeline again.
- **A Release job failed after the tag was pushed.** Open the run and click **Re-run failed jobs**. Uploads replace files already on the draft, so re-running is safe. If the code needs a fix, delete the draft and the tag (`git push origin :refs/tags/vX.Y.Z`), fix on dev, and release again. The next run bumps from the version on `main`, so you may skip a number; that's fine.
- **The Mac jobs or the TestFlight upload are waiting.** They need your approval of `macos-signing`; see steps 4 and 5 above.
- **The iPhone build failed.** Nothing ships until it passes. Fix the cause, then **Re-run failed jobs**: the new attempt gets a higher build number, and the TestFlight upload and the publishing follow it.
- **A Linux or Mac build failed after the iPhone build passed.** The iPhone build waits; it isn't in TestFlight. **Re-run failed jobs** rebuilds what failed, then uploads the kept iPhone build. When more than a day has passed, the kept `.ipa` is gone: re-run `release-ios` on its own instead (**Re-run this job**), which builds it again under a higher number, then uploads and publishes.
- **The TestFlight upload failed.** **Re-run failed jobs** uploads the same build again. When App Store Connect says it already has that build number, re-run `release-ios` instead, as above.
- **Notarization was rejected.** The job prints Apple's log. Usually a binary is missing the hardened runtime or a timestamp; `scripts/release/verify-signing.sh` names it.
- **`main` was bumped but no tag exists.** Run **Release tag**. It tags `main`'s head with the version in `Cargo.toml` and pushes the tag, with the same **publish** box.
- **You want to release `main` as it is, without merging dev.** Run **Release bump**, then **Release tag**.
- **`sync-dev` opened a pull request.** Resolve the conflicts in it and merge it into dev.
- **Check a published release.** `sh scripts/release/validate-release.sh` downloads `latest.json` and checks every URL in it, the DMGs, the Linux binaries and `SHA256SUMS`.

## Build a release locally

`scripts/release/package-macos.sh` does on your Mac what the Mac job does. With no identity it signs ad hoc, and it skips Linux binaries it can't find with a warning:

```sh
TAURI_SIGNING_PRIVATE_KEY_PATH=<path to your update key> MAXHQ_RELEASES_REPO=<owner>/<repo> \
  sh scripts/release/package-macos.sh --linux-dir <folder with the Linux maxd>
```

`MAXHQ_RELEASES_REPO` names the repository whose releases the app updates from. The build writes its `latest.json` link into the app, so no tracked file names the owner; without it the app can't look for updates.

`pnpm --filter maxhq-mobile mobile:testflight` uploads an iPhone build from your Mac under the number in `apps/mobile/native-build.json`, signed through the account in Xcode, or through an API key when `MAXHQ_ASC_KEY_PATH`, `MAXHQ_ASC_KEY_ID` and `MAXHQ_ASC_ISSUER_ID` are set. Local numbers stay below 10000 and the release job's start at 10000, so the two never meet.

Set `MAXHQ_SIGN_IDENTITY` to sign with a Developer ID from your keychain, and add `--notarize` with `APPLE_API_KEY`, `APPLE_API_ISSUER` and `APPLE_API_KEY_PATH` set to notarize. `MAXHQ_UNSIGNED_UPDATES=1` skips the update key entirely, for a build you only run yourself; Update everything then asks before it sends that build's maxd to other devices. The files land in `target/release/bundle/release`.
