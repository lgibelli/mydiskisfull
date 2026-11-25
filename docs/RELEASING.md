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

## The Windows edition (not released yet)

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

Still to decide before a first release (`FINISH-UP-PLAN.md`, Phase 7):

- **Signing.** The programs and the installer are unsigned, so SmartScreen warns on download
  ("Windows protected your PC", More info → Run anyway). A code-signing certificate or Azure
  Trusted Signing goes in `bundle > windows > signCommand`.
- **Updates.** None yet: installed copies don't hear of a new version. `tauri-plugin-updater` is
  the way, with a key made once (as the Sparkle one is), its public half in `tauri.conf.json`, and
  a `latest.json` on the latest published release, so that publishing ships, as on the Mac.
- **The release job.** `release.yml`'s `windows` job builds both installers (unsigned), attests
  them and puts them, with `SHA256SUMS-windows.txt`, on the draft the macOS job made; only when
  asked: `gh workflow run release.yml --ref main -f windows=true`, never on a push to
  `release/**`. It has never run. Signing goes in before it's turned on for a real release.

## After publishing

- Installed copies see the update within a day; "Check for Updates…" sees it at once.
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
