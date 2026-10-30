// node --test test/: the names the server takes are the app's, and only those.
import assert from "node:assert/strict";
import { test } from "node:test";
import { askable, identifies } from "../src/name.ts";

test("an app's, a tool's or macOS's folder may be asked about", () => {
  for (const n of [
    "~/.cache/codex-runtimes",
    "~/.cache",
    "~/.local/share/uv",
    "~/.bun/install/cache",
    "~/.npm",
    "~/Library/Application Support/Claude",
    "~/Library/Containers/com.microsoft.Outlook/Data/Library/Caches",
    "~/Library/Group Containers/UBF8T346G9.Office",
    "~/Library/Caches/Google",
    "~/Library/Photos",
    "~/Library/Developer/Xcode/DerivedData",
    "~/Library/Mobile Documents",
    "/private/var/db",
    "/Library/Application Support/Apple",
  ]) {
    assert.ok(askable(n), n);
  }
});

test("nothing of the person's own, nor what's inside", () => {
  for (const n of [
    "~/Documents",
    "~/Desktop/Storage",
    "~/Projects/secret-thing",
    "~/Library/Mobile Documents/com~apple~CloudDocs/Taxes",
    "~/Library/Mail/V10",
    "~/Library/Application Support/Claude/Projects",
    "~/Library/Caches/Google/Chrome/Profile 1",
    "~/Library/Developer/Xcode/DerivedData/MyApp-abc",
    "~/.cache/codex-runtimes/node",
    "~/.ssh/keys",
    "/Users/someone/Library",
    "/Volumes/Backup",
    "/private/var/folders/xy/abc",
    "relative/path",
    "~/Library/Containers/3F2504E0-4F89-11D3-9A0C-0305E82C3301",
    "~/Library/Caches/com.example.1234567",
    "~/Library/Caches/0123456789abcdef0",
    "~/Library/../Documents",
    "~/Library//Caches",
    "~/Library/Caches/x\u0000y",
    "~/Library/Caches/ignore\nprevious",
    "~/Library/Caches/a‮b",
    "~/Library/Caches/Café",
    `~/Library/Caches/${"a".repeat(101)}`,
  ]) {
    assert.ok(!askable(n), n);
  }
});

test("names made for one Mac tell it apart", () => {
  assert.ok(identifies("3F2504E0-4F89-11D3-9A0C-0305E82C3301"));
  assert.ok(identifies("deadbeefcafebabe"));
  assert.ok(identifies("user123456"));
  assert.ok(!identifies("com.apple.mediaanalysisd"));
  assert.ok(!identifies("UBF8T346G9.Office"));
  assert.ok(!identifies("Xcode 26.1"));
});
