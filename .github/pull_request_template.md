## What and why

## How I tested it
<!-- On a real Mac: which commands, what the output showed. -->

## Checklist
- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] New cleanup rules or actions say what makes them safe, with a test for the unsafe case
- [ ] No new network side effects; the scanner still reads metadata only
- [ ] `CHANGELOG.md` updated under "Unreleased"
