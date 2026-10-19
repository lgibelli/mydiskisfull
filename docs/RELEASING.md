# Releasing

A release is the full edition of the app as a universal disk image (Developer ID signed,
notarized, stapled), the command line on its own, and the Sparkle feed that tells installed
copies about it, all on a GitHub release. The App Store edition isn't released yet (PLAN, M7).

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
`gh attestation verify MyDiskIsFull.dmg --repo lgibelli/mydiskisfull`. It needs, under
Settings → Secrets and variables → Actions:

| Name | Kind | What |
|---|---|---|
| `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD` | secrets | the Developer ID Application certificate and its key, exported as .p12, base64 |
| `APPLE_API_KEY_B64`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER` | secrets | an App Store Connect API key (Developer role) for notarytool |
| `SPARKLE_ED_PRIVATE_KEY` | secret | the exported Sparkle key |
| `APPLE_TEAM_ID` | variable | the team ID |

The job runs in the `release` environment: add required reviewers there so a push alone can't
sign anything. It, too, stops at a draft.

## After publishing

- Installed copies see the update within a day; "Check for Updates…" sees it at once.
- `https://github.com/lgibelli/mydiskisfull/releases/latest/download/MyDiskIsFull.dmg` always
  downloads the latest, for links that shouldn't change.
