# Contributing

Thanks for helping. MyDiskIsFull is macOS-only, so you need a Mac with Rust 1.88 or newer.

## Build and test

```sh
cargo build
cargo test
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
```

CI runs exactly these on macOS. The tests use real APFS features (clones, sparse files), so they
need an APFS volume; the default temp directory is fine.

All dependencies are ordinary crates.io crates. If you are on a metered connection, everything
after the first fetch builds with `cargo build --offline`.

## Ground rules for code

- **Metadata only.** The scanner must never read file contents. Reading would update access
  times (which the cold-data feature relies on) and could download cloud placeholders.
- **No network side effects.** Any code path that might trigger a download or upload has to be
  explicit and labelled.
- **Never delete without proof.** New cleanup rules must say what makes the item safe to remove
  (regenerable, or a verified copy elsewhere) and come with a test that shows the unsafe case is
  left alone.
- **Stay within the budget.** The watcher has a CPU and disk-write budget (see
  [docs/PLAN.md](docs/PLAN.md)). Changes to the hot path should say what they cost.
- **Explain, don't just detect.** Incidents read as one sentence with the evidence in it.

## Pull requests

- Keep PRs focused, and describe what you tested on a real Mac.
- Add or update tests with behaviour changes.
- Update `CHANGELOG.md` under "Unreleased".
