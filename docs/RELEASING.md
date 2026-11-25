# Releasing

A release is the full edition of the app as a universal disk image (Developer ID signed,
notarized, stapled), the command line on its own, and the Sparkle feed that tells installed
copies about it, all on a GitHub release; and MyDiskIsFull Lite, the App Store edition, uploaded
to App Store Connect from the same commit, for TestFlight and review. The Windows edition isn't
released yet: "The Windows edition" below says what it takes.

## How installed copies update

The app reads `https://github.com/lgibelli/mydiskisfull/releases/latest/download/appcast.xml`
once a day, which is the `appcast.xml` attached to the latest *published* release. Drafts and
pre-releases are invisible to it, so publishing the release is what ships the update.

The feed is only a hint. Sparkle installs an update only if the disk image carries an Ed25519
signature made with the project's private key, matching `SUPublicEDKey` in
`app/Resources/Info.plist`, and the app inside is signed by the same team as the installed one.
Whoever controls the feed can't get anything else installed.

`CFBundleVersion` is what Sparkle compares: `app/build.sh` makes it
`major × 10000 + minor × 100 + patch` from the version in `Cargo.toml`, so it only grows.

## Once: keys and credentials

- **Developer ID Application certificate** for your team, in the login keychain, and `TEAM_ID`
  in `release.env` (copy `release.env.example`).
- **Notarization**: a notarytool keychain profile, `NOTARY_PROFILE` in `release.env`:
  `xcrun notarytool store-credentials <name> --apple-id <you> --team-id <team> --password <app-specific password>`.
- **The Sparkle key**: made once with
  `build/vendor/sparkle-2.10.0/bin/generate_keys --account io.github.lgibelli.mydiskisfull`
  (run `app/build.sh` first to fetch Sparkle). It lives in the login keychain. **Back it up**:
  `generate_keys --account io.github.lgibelli.mydiskisfull -x <file>`, kept somewhere safe. If
  it's lost, installed copies can never be updated again: they'd have to be reinstalled by hand
  from a build with a new public key.

## Making a release on your Mac

1. Bump `version` in `Cargo.toml` (`cargo build` updates `Cargo.lock`).
2. In `CHANGELOG.md`, turn `## Unreleased` into `## [x.y.z] - YYYY-MM-DD`, add a new empty
   Unreleased above it and the `[x.y.z]:` link at the bottom. That section is the release notes,
   on GitHub and in the update alert.
3. Commit and push.
4. `app/release.sh` builds, notarizes (twice: the app, then the image; a few minutes each),
   staples, checks the image as a download, and signs the feed, into `build/release/`. It
   refuses a dirty working tree.
5. `app/release.sh --upload` creates the draft release `vx.y.z` with the files.
6. Read the draft on GitHub, then publish it. Publishing creates the tag.

## Making a release with GitHub Actions

`.github/workflows/release.yml` runs the same two commands from a `release/x.y.z` branch (or by
hand), with a provenance attestation in between, so a download can be checked with
`gh attestation verify MyDiskIsFull.dmg --repo lgibelli/mydiskisfull`. Beside it, its
`app-store` job runs `app/app-store.sh --upload` from the same commit, numbered with the run
(0.1.2 is build 102.<run>, as a branch may run again): Lite reaches App Store Connect and, once processed (usually within the hour), TestFlight. Both jobs wait in the
`release` environment, approved together. For the full edition alone, with nothing uploaded
to App Store Connect, run it by hand with Lite off: `gh workflow run release.yml --ref main -f
lite=false` (no release branch: a push runs both). It needs, under Settings → Secrets and variables →
Actions:

| Name | Kind | What |
|---|---|---|
| `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD` | secrets | the Developer ID Application certificate and its key, exported as .p12, base64 |
| `APPLE_API_KEY_B64`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER` | secrets | an App Store Connect API key (Developer role) for notarytool |
| `SPARKLE_ED_PRIVATE_KEY` | secret | the exported Sparkle key |
| `MAS_APP_P12_BASE64`, `MAS_INSTALLER_P12_BASE64`, `MAS_P12_PASSWORD` | secrets | the Apple Distribution and Mac Installer Distribution identities, each a .p12 with Apple's WWDR G3 intermediate in it, base64 |
| `MAS_PROFILE_BASE64` | secret | Lite's Mac App Store provisioning profile, base64 |
| `APPLE_TEAM_ID` | variable | the team ID |
| `APP_STORE_BUNDLE_ID` | variable | Lite's bundle id |

The job runs in the `release` environment: add required reviewers there so a push alone can't
sign anything. It, too, stops at a draft.

## The App Store edition

MyDiskIsFull Lite (`app/build.sh --app-store`) goes to the Mac App Store as an app of its own.
Once:

- Register its bundle id, and make a Mac App Store provisioning profile for it with the Apple
  Distribution certificate (both work with an App Store Connect API key, or in the developer
  portal).
- Create the app in App Store Connect (Apps → New App, macOS). The API can't do this one.
- In `release.env`: `APP_STORE_BUNDLE_ID`, `APP_STORE_PROFILE` and the API key
  (`release.env.example` says how). The keychain needs the Apple Distribution and Mac Installer
  Distribution identities.

Each version:

```sh
app/app-store.sh             # universal, signed for the store, packaged, validated
app/app-store.sh --upload    # and uploaded as a new build
BUILD_NUMBER=102.1 app/app-store.sh --upload   # 0.1.2 again: each build number goes once
```

Then, in App Store Connect, pick the build for the version and submit it for review. Releasing
it once it's approved is the author's call, like publishing a GitHub release.

## The Windows edition

The Windows app (`app-windows/`, Tauri) ships as a per-user NSIS installer holding the app,
`mdif.exe` and `mdif-watch.exe`; nothing needs an administrator. Its version is the workspace's:
`app-windows/src-tauri/Cargo.toml` names the same one as `Cargo.toml` (a test checks they agree),
and `tauri.conf.json` names none, so the installer and the app take it from there. Bump both.

```sh
app-windows/build.sh                           # x64, on Windows or cross-built from a Mac
app-windows/build.sh aarch64-pc-windows-msvc   # ARM64
```

It lands in `app-windows/src-tauri/target/<target>/release/bundle/nsis/MyDiskIsFull_<version>_<arch>-setup.exe`.
On Windows it needs Rust and the Tauri CLI (`cargo install tauri-cli --version 2.12.5 --locked`);
from a Mac also `cargo install cargo-xwin` and `brew install makensis llvm`. CI's `windows-app`
job builds the x64 one, installs, updates and uninstalls it on the runner, and keeps it as the
`MyDiskIsFull-windows-x64-setup` artifact; then it builds the ARM64 one and keeps it as
`MyDiskIsFull-windows-arm64-setup`, which `windows-app-arm` installs, updates and uninstalls on a
Windows on ARM runner (both with `app-windows/tests/install.py`). The installer stops the watcher before it replaces its
program and starts it after (`app-windows/src-tauri/windows/hooks.nsh`).

What a first release is (`FINISH-UP-PLAN.md`, Phase 7):

- **Unsigned, at first.** The programs and the installer have no Authenticode signature, so
  SmartScreen warns on download ("Windows protected your PC": More info → Run anyway), which the
  README says. A certificate (Azure Artifact Signing takes organisations in the US, Canada, the EU
  and the UK, individuals only in the US and Canada) goes in `bundle > windows > signCommand`
  later; nothing else changes.
- **Updates.** As Sparkle on the Mac: once a day (Settings → Updates turns it off; Check Now checks
  at once) the app reads `latest.json` from the latest *published* release, and offers Install and
  Restart for a newer version (`app-windows/src-tauri/src/update.rs`). Each installer is signed
  with the updater's key, an ed25519 (minisign) key made once with `cargo tauri signer generate
  --ci`: its public half is `plugins > updater > pubkey` in `tauri.conf.json`, its private half is
  in the login keychain (service `https://v2.tauri.app/plugin/updater`, account
  `io.github.lgibelli.mydiskisfull`, beside Sparkle's key) and in the `release` environment's
  secret `TAURI_SIGNING_PRIVATE_KEY`, no password. The app checks the signature before it runs
  the installer, and the version signed in it (`requireSignedVersion`: the bundler writes it),
  and refuses what fails either: lose the key and installed copies can't update (a
  new key needs a manual install). To put it on GitHub:

  ```sh
  security find-generic-password -a io.github.lgibelli.mydiskisfull \
    -s https://v2.tauri.app/plugin/updater -w | gh secret set TAURI_SIGNING_PRIVATE_KEY --env release
  ```

  With the key set, `build.sh` adds `tauri.updater.json` (`createUpdaterArtifacts`), and Tauri
  writes the installer's `.sig` beside it; `app-windows/feed.sh <version> <folder>` writes
  `latest.json` from both installers, their signatures and the version's CHANGELOG section. A
  release without the Windows installers gets the latest release's `latest.json`
  (`app-windows/keep-feed.sh`, which `app/release.sh --upload` runs, from a Mac and in
  `release.yml`: a draft with a feed of its own keeps it, and a release it can't read stops the
  upload), so it still names the last Windows installers: only if each one it names is still on a
  published release, and a copy taken earlier is removed once the latest release has none. With
  `windows=true` the Mac job skips it (`WINDOWS_FEED_FOLLOWS`): the windows job uploads the feed.
  Withdrawing a Windows release (deleted, or a draft or prerelease again): installed copies read
  the latest published release's `latest.json`, which may be a copy naming it, so replace that
  file (with the previous Windows release's) or delete it, at once; the app doesn't ask GitHub
  anything else. Before any Windows release there is none (a 404), which the app takes as
  nothing new; any other answer is a failed check, said as one.
- **No Backups page.** Windows keeps its five pages; one for File History or another backup can
  come in a later version.
- **The release job.** `release.yml`'s `windows` job builds both installers (unsigned for
  Windows, signed for the updater), attests them and puts them, with their `.sig`, `latest.json`
  and `SHA256SUMS-windows.txt`, on the draft the macOS job made; only when asked: `gh workflow
  run release.yml --ref main -f windows=true`, never on a push to `release/**`. It fails without
  the updater key, and before uploading when a signature isn't the app's key's (Tauri signs with
  any key it's given; `the_release_feed_is_signed_with_the_apps_key`). It has never run.

Publishing a draft without the Windows installers: run `app-windows/keep-feed.sh <tag>` again
just before, so its feed is the latest published one's (a Windows release published since the
draft was made would otherwise be hidden by it).

## After publishing

- Installed copies see the update within a day; "Check for Updates…" sees it at once (on Windows,
  Settings → Updates → Check Now, once the release has the Windows installers and `latest.json`).
- `https://github.com/lgibelli/mydiskisfull/releases/latest/download/MyDiskIsFull.dmg` always
  downloads the latest, for links that shouldn't change.

## What's known about folders

`crates/mdif-core/knowledge/folders.json` is the list of what folders are called, why they grow
and what helps. Each build carries the one in the repository; installed copies of the full
edition also download a newer one once a day (`mdif knowledge update`) and take it only when it's
signed with the updates' key and its `version` is higher than theirs. It's text only: it changes
what's said about a folder, never what's deleted (that's in the code), so it can go out between
releases.

1. Edit the list and raise its `version`. `cargo test -p mdif-core knowledge` checks it.
2. `app/knowledge.sh` checks it, signs it with the Sparkle key and copies `folders-1.json` and
   `folders-1.json.sig` into `KNOWLEDGE_DIR` (release.env), the website's folder.
3. Commit and push the website: it's live at
   `https://www.salamacchine.it/apps/mydiskisfull/folders-1.json` once it's deployed.

A change to the format that older copies can't read goes out as `folders-2.json`, beside the old
one.
