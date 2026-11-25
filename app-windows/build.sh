#!/bin/bash
# Builds MyDiskIsFull for Windows: mdif.exe and mdif-watch.exe, then the app
# around them, then its installer (NSIS, per user, no administrator needed):
#
#   app-windows/build.sh [target]     # x86_64-pc-windows-msvc (default) or aarch64-pc-windows-msvc
#
# On Windows (Git Bash, or the CI runner) with Rust and the Tauri CLI
# (`cargo install tauri-cli --version "^2" --locked`). From a Mac or Linux it
# cross-builds with cargo-xwin and makensis (`cargo install cargo-xwin`,
# `brew install makensis`, and LLVM for llvm-rc, lld-link and clang-cl).
#
# The installer lands in app-windows/src-tauri/target/<target>/release/bundle/nsis
# (with its updater signature, .sig, when TAURI_SIGNING_PRIVATE_KEY is set).
set -euo pipefail

target=${1:-x86_64-pc-windows-msvc}
case $target in
  x86_64-pc-windows-msvc | aarch64-pc-windows-msvc) ;;
  *) echo "build.sh: $target isn't a Windows target this builds" >&2; exit 2 ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)

case $(uname -s) in
  MINGW* | MSYS* | CYGWIN* | Windows_NT) build=(cargo build); runner=() ;;
  *)
    # llvm-rc gives the app its icon and version (tauri-winres); lld-link and
    # clang-cl link it and compile SQLite.
    for llvm in /opt/homebrew/opt/llvm/bin /opt/homebrew/opt/llvm@*/bin /usr/local/opt/llvm/bin; do
      if [ -x "$llvm/llvm-rc" ]; then PATH="$llvm:$PATH"; break; fi
    done
    command -v llvm-rc >/dev/null || { echo "build.sh: llvm-rc not found (install LLVM)" >&2; exit 1; }
    build=(cargo xwin build); runner=(--runner cargo-xwin)
    ;;
esac

# mdif and its watcher, built as the command line's own release.
(cd "$root" && "${build[@]}" --release --locked --target "$target" -p mdif-cli --bins)

# Bundled beside the app (tauri.release.json's externalBin), named by target.
bins="$here/src-tauri/binaries"
mkdir -p "$bins"
for bin in mdif mdif-watch; do
  cp "$root/target/$target/release/$bin.exe" "$bins/$bin-$target.exe"
done

# Signed for the updater when its key is set (release.yml, from the release
# environment's secret): Tauri writes the installer's .sig beside it, which
# feed.sh puts in latest.json. Without it the installer is the same, unsigned
# for updates.
signing=()
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then signing=(--config tauri.updater.json); fi

cd "$here/src-tauri"
cargo tauri build ${runner[@]+"${runner[@]}"} --target "$target" --config tauri.release.json \
  ${signing[@]+"${signing[@]}"} --bundles nsis
ls -l "target/$target/release/bundle/nsis/"
