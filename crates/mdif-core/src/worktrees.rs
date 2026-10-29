//! Git worktrees whose work is done: removing one loses nothing.
//!
//! Coding agents (Claude Code) check out a git worktree for each task, and
//! each worktree builds its own output: 3–5 GB for a Rust or Flutter app.
//! Once the work is merged nothing is left in it but that output, and
//! nothing removes it: on the Mac this was written on, 8 merged worktrees of
//! one project held 27 GB. A worktree is finished when
//! - what it has checked out is merged into the default branch, local or the
//!   remote's: contained in it, or merging it would change nothing (a squash
//!   merge);
//! - nothing in it is uncommitted: no changes, no untracked files;
//! - what git ignores in it, and `git worktree remove` deletes with it, can
//!   be made again: build output ([`ARTIFACTS`]), or what the main checkout
//!   has too (generated code, a copied `.env`: a file must have the same
//!   size there, so one with other values keeps the worktree);
//! - nothing happened in it for a while: an hour for an agent's (in a
//!   `.claude/worktrees` folder, [`QUIET`]), so one that has just started
//!   isn't counted; a week for one made by hand ([`IDLE`]), a workspace
//!   that may be used again. Git's HEAD and log for it count, and the folder
//!   itself;
//! - it isn't locked.
//!
//! Git reads, never writes: status runs without optional locks (no index
//! refresh), and the trial merge writes its objects to a scratch folder
//! instead of the repository. Repositories are the projects in the code
//! folders ([`CODE_ROOTS`]) whose `.git` has worktrees, so git only runs
//! where there are some.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::owners::{ARTIFACTS, CODE_ROOTS};
use crate::privacy;

/// How long nothing must have happened in an agent's worktree.
pub const QUIET: i64 = 3600;
/// How long nothing must have happened in a worktree made by hand.
pub const IDLE: i64 = 7 * 86_400;

/// Default branches, local and the remote's, in the order they're named.
const DEFAULTS: &[&str] = &[
    "refs/heads/main",
    "refs/heads/master",
    "refs/remotes/origin/HEAD",
    "refs/remotes/origin/main",
    "refs/remotes/origin/master",
];

/// Real git binaries. `/usr/bin/git` is left out: without the developer
/// tools it offers to install them, in a dialog.
const GITS: &[&str] = &[
    "/Library/Developer/CommandLineTools/usr/bin/git",
    "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
    "/opt/homebrew/bin/git",
    "/usr/local/bin/git",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub path: PathBuf,
    /// The repository it belongs to (its main worktree).
    pub repo: PathBuf,
    /// The branch it is merged into: `main`, `origin/main`.
    pub into: String,
    /// An agent's (in a `.claude/worktrees` folder), not one made by hand.
    pub agents: bool,
    /// When something last happened in it (Unix seconds).
    pub touched: i64,
}

/// The finished worktrees of every project in the code folders.
pub fn finished(home: &Path, now: i64) -> Vec<Finished> {
    let Some(git) = Git::find() else {
        return Vec::new();
    };
    repos(home)
        .iter()
        .flat_map(|r| git.finished_in(r, now))
        .collect()
}

/// Removes a finished worktree with `git worktree remove`, which refuses one
/// with changes (whatever was checked before); its branch stays. The error
/// is git's own.
pub fn remove(f: &Finished) -> Result<(), String> {
    let git = Git::find().ok_or("No git found: install the Command Line Tools.")?;
    let out = git
        .cmd(&f.repo)
        .args(["worktree", "remove"])
        .arg(&f.path)
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Whether git tracks a file in `dir`; `None` when no repository holds it.
/// In one that git can't be asked about, it counts as tracked.
pub fn tracks_files_in(dir: &Path) -> Option<bool> {
    let (parent, name) = (dir.parent()?, dir.file_name()?);
    parent.ancestors().find(|a| a.join(".git").exists())?;
    let tracked = Git::find()
        .and_then(|git| {
            git.out(
                parent,
                &[OsStr::new("ls-files"), "-z".as_ref(), "--".as_ref(), name],
            )
        })
        .is_none_or(|listed| !listed.is_empty());
    Some(tracked)
}

/// Whether the repository holding `dir` ignores it (its `.gitignore`):
/// `None` when no repository holds it or git can't be asked.
pub fn ignores(dir: &Path) -> Option<bool> {
    let (parent, name) = (dir.parent()?, dir.file_name()?);
    parent.ancestors().find(|a| a.join(".git").exists())?;
    let git = Git::find()?;
    // `check-ignore -q` says it by its exit status: 0 ignored, 1 not.
    let status = git
        .cmd(parent)
        .args([
            OsStr::new("check-ignore"),
            "-q".as_ref(),
            "--".as_ref(),
            name,
        ])
        .stdout(Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Projects in the code folders with linked worktrees.
fn repos(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = CODE_ROOTS
        .iter()
        .map(|r| home.join(r))
        .filter(|r| privacy::may_open(r))
        .filter_map(|r| std::fs::read_dir(r).ok())
        .flat_map(|rd| rd.flatten())
        .map(|e| e.path())
        .filter(|p| p.join(".git/worktrees").is_dir())
        .collect();
    out.sort();
    out
}

struct Git {
    bin: PathBuf,
}

/// A default branch: its name and its tree.
struct Base {
    name: String,
    tree: String,
}

/// A worktree as `git worktree list` describes it.
#[derive(Debug, PartialEq)]
struct Listed {
    path: PathBuf,
    head: String,
    locked: bool,
    prunable: bool,
}

impl Git {
    fn find() -> Option<Git> {
        let developer =
            std::env::var_os("DEVELOPER_DIR").map(|d| Path::new(&d).join("usr/bin/git"));
        developer
            .into_iter()
            .chain(GITS.iter().map(PathBuf::from))
            .find(|p| p.is_file())
            .map(|bin| Git { bin })
    }

    /// git in `dir`, reading only, in a predictable language, with nothing
    /// inherited that would point it at another repository.
    fn cmd(&self, dir: &Path) -> Command {
        let mut c = Command::new(&self.bin);
        c.arg("-C")
            .arg(dir)
            .args(["-c", "core.fsmonitor=false"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            c.env_remove(var);
        }
        c
    }

    /// Standard output, when it succeeded.
    fn out<S: AsRef<OsStr>>(&self, dir: &Path, args: &[S]) -> Option<Vec<u8>> {
        let o = self.cmd(dir).args(args).output().ok()?;
        o.status.success().then_some(o.stdout)
    }

    fn ok<S: AsRef<OsStr>>(&self, dir: &Path, args: &[S]) -> bool {
        self.cmd(dir)
            .args(args)
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn finished_in(&self, repo: &Path, now: i64) -> Vec<Finished> {
        let Some(listed) = self.out(repo, &["worktree", "list", "--porcelain", "-z"]) else {
            return Vec::new();
        };
        let defaults = self.defaults(repo);
        if defaults.is_empty() {
            return Vec::new();
        }
        let objects = repo.join(".git/objects");
        let mut list = parse_list(&listed).into_iter();
        // The first is the main worktree.
        let Some(main) = list.next() else {
            return Vec::new();
        };
        list.filter(|w| !w.locked && !w.prunable && privacy::may_open(&w.path))
            .filter_map(|w| {
                let agents = is_agents(&w.path);
                let touched = last_touched(&w.path)?;
                if now - touched < if agents { QUIET } else { IDLE } {
                    return None;
                }
                let into = self.merged_into(repo, &objects, &w.head, &defaults)?;
                self.nothing_to_lose(&w.path, &main.path).then(|| Finished {
                    path: w.path,
                    repo: repo.to_path_buf(),
                    into,
                    agents,
                    touched,
                })
            })
            .collect()
    }

    /// The default branches that exist, each once.
    fn defaults(&self, repo: &Path) -> Vec<Base> {
        let mut args = vec!["for-each-ref", "--format=%(refname)%00%(symref)%00%(tree)"];
        args.extend(DEFAULTS);
        let Some(out) = self.out(repo, &args) else {
            return Vec::new();
        };
        let mut found: Vec<(usize, Base)> = Vec::new();
        for line in String::from_utf8_lossy(&out).lines() {
            let mut f = line.split('\0');
            let (Some(refname), Some(symref), Some(tree)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            // A pattern also matches names below it (`refs/heads/main/x`).
            let Some(order) = DEFAULTS.iter().position(|d| *d == refname) else {
                continue;
            };
            let name = if symref.is_empty() { refname } else { symref };
            if tree.is_empty() || found.iter().any(|(_, d)| d.name == name) {
                continue;
            }
            found.push((
                order,
                Base {
                    name: name.to_string(),
                    tree: tree.to_string(),
                },
            ));
        }
        found.sort_by_key(|(order, _)| *order);
        found.into_iter().map(|(_, d)| d).collect()
    }

    /// The default branch `head` is merged into: contained in it, or else
    /// merging it would change nothing (it was squashed into it).
    fn merged_into(
        &self,
        repo: &Path,
        objects: &Path,
        head: &str,
        defaults: &[Base],
    ) -> Option<String> {
        if let Some(d) = defaults
            .iter()
            .find(|d| self.ok(repo, &["merge-base", "--is-ancestor", head, &d.name]))
        {
            return Some(short(&d.name));
        }
        let scratch = Scratch::new().ok()?;
        defaults
            .iter()
            .find(|d| {
                let out = self
                    .cmd(repo)
                    .env("GIT_OBJECT_DIRECTORY", &scratch.0)
                    .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects)
                    .args(["merge-tree", "--write-tree", &d.name, head])
                    .output();
                // Exit status 1 is a conflict; the first line is the tree.
                out.is_ok_and(|o| {
                    o.status.success()
                        && o.stdout.split(|&b| b == b'\n').next() == Some(d.tree.as_bytes())
                })
            })
            .map(|d| short(&d.name))
    }

    /// No changes and no untracked files (whatever `status.showUntrackedFiles`
    /// says), and nothing ignored that couldn't be made again.
    fn nothing_to_lose(&self, worktree: &Path, main: &Path) -> bool {
        let Some(out) = self.out(
            worktree,
            &[
                "status",
                "--porcelain",
                "-z",
                "--untracked-files=normal",
                "--ignored=matching",
                "--ignore-submodules=none",
            ],
        ) else {
            return false;
        };
        // "XY path", NUL-ended; an ignored folder is listed, not what's in it.
        out.split(|&b| b == 0).filter(|e| !e.is_empty()).all(|e| {
            e.strip_prefix(b"!! ").is_some_and(|rel| {
                let rel = Path::new(OsStr::from_bytes(rel.strip_suffix(b"/").unwrap_or(rel)));
                remade(worktree, main, rel)
            })
        })
    }
}

/// Whether `rel`, ignored in `worktree`, can be made again: build output, a
/// link, or what the main checkout has too (a file the same size).
fn remade(worktree: &Path, main: &Path, rel: &Path) -> bool {
    if rel
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|n| ARTIFACTS.contains(&n))
    {
        return true;
    }
    let Ok(here) = std::fs::symlink_metadata(worktree.join(rel)) else {
        return false;
    };
    if here.is_symlink() {
        return true;
    }
    match std::fs::metadata(main.join(rel)) {
        Ok(there) if here.is_dir() => there.is_dir(),
        Ok(there) => there.is_file() && there.len() == here.len(),
        Err(_) => false,
    }
}

/// A folder for a trial merge's objects, removed when done.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Scratch> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!("mdif-merge-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `git worktree list --porcelain -z`: attributes separated by NUL, each
/// worktree ended by an empty one.
fn parse_list(out: &[u8]) -> Vec<Listed> {
    let mut list = Vec::new();
    let mut cur: Option<Listed> = None;
    for field in out.split(|&b| b == 0) {
        if field.is_empty() {
            list.extend(cur.take());
            continue;
        }
        let (key, value) = match field.iter().position(|&b| b == b' ') {
            Some(i) => (&field[..i], &field[i + 1..]),
            None => (field, &b""[..]),
        };
        match key {
            b"worktree" => {
                list.extend(cur.take());
                cur = Some(Listed {
                    path: PathBuf::from(OsStr::from_bytes(value)),
                    head: String::new(),
                    locked: false,
                    prunable: false,
                });
            }
            b"HEAD" => {
                if let Some(c) = &mut cur {
                    c.head = String::from_utf8_lossy(value).into_owned();
                }
            }
            b"locked" | b"bare" => {
                if let Some(c) = &mut cur {
                    c.locked = true;
                }
            }
            b"prunable" => {
                if let Some(c) = &mut cur {
                    c.prunable = true;
                }
            }
            _ => {}
        }
    }
    list.extend(cur);
    list.retain(|w| !w.head.is_empty());
    list
}

/// In a `.claude/worktrees` folder.
fn is_agents(path: &Path) -> bool {
    let mut up = path.iter().rev().skip(1);
    up.next() == Some(OsStr::new("worktrees")) && up.next() == Some(OsStr::new(".claude"))
}

/// When git's HEAD or log for `worktree`, or the folder itself, last changed
/// (Unix seconds). Its `.git` file says where git keeps them. Not the index:
/// any `git status` rewrites it (a shell prompt, an editor looking), and
/// work shows anyway, uncommitted (not finished) or committed (the log).
fn last_touched(worktree: &Path) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    let text = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let admin = worktree.join(text.strip_prefix("gitdir:")?.trim());
    let mtime = |p: PathBuf| std::fs::symlink_metadata(p).ok().map(|m| m.mtime());
    [
        admin.join("HEAD"),
        admin.join("logs/HEAD"),
        worktree.to_path_buf(),
    ]
    .into_iter()
    .filter_map(mtime)
    .max()
}

/// `main`, `origin/main`.
fn short(refname: &str) -> String {
    refname
        .strip_prefix("refs/heads/")
        .or_else(|| refname.strip_prefix("refs/remotes/"))
        .unwrap_or(refname)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_parses_with_any_path() {
        let out = b"worktree /r\0HEAD aaa\0branch refs/heads/main\0\0\
                    worktree /r/.claude/worktrees/a b\0HEAD bbb\0detached\0\0\
                    worktree /r/.claude/worktrees/c\0HEAD ccc\0branch refs/heads/c\0locked why\0\0\
                    worktree /gone\0HEAD ddd\0prunable gitdir file points to non-existent location\0\0";
        let l = parse_list(out);
        assert_eq!(l.len(), 4);
        assert_eq!(l[1].path, Path::new("/r/.claude/worktrees/a b"));
        assert_eq!(l[1].head, "bbb");
        assert!(!l[1].locked && l[2].locked && l[3].prunable);
    }

    #[test]
    fn agents_worktrees_are_in_claude_worktrees() {
        assert!(is_agents(Path::new("/r/.claude/worktrees/agent-1")));
        assert!(!is_agents(Path::new("/r/.claude/worktrees")));
        assert!(!is_agents(Path::new("/devel/r-editor")));
        assert!(!is_agents(Path::new("/r/worktrees/agent-1")));
    }

    #[test]
    fn a_folder_with_a_tracked_file_is_not_build_output() {
        let Some(git) = Git::find() else {
            eprintln!("no git: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("web");
        std::fs::create_dir_all(repo.join("dist")).unwrap();
        std::fs::create_dir_all(repo.join("build")).unwrap();
        std::fs::write(repo.join("package.json"), "{}").unwrap();
        std::fs::write(repo.join("dist/lib.js"), "x").unwrap();
        std::fs::write(repo.join("build/out.js"), "x").unwrap();
        std::fs::write(repo.join(".gitignore"), "build/\n").unwrap();
        // Outside a repository there is nothing to ask.
        assert_eq!(tracks_files_in(&repo.join("dist")), None);
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["add", "."],
            &["commit", "-qm", "start"],
        ] {
            let st = Command::new(&git.bin)
                .arg("-C")
                .arg(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        }
        assert_eq!(tracks_files_in(&repo.join("dist")), Some(true));
        assert_eq!(tracks_files_in(&repo.join("build")), Some(false));
        assert!(!crate::clean::build_output(&repo.join("dist")));
        assert!(crate::clean::build_output(&repo.join("build")));
    }

    /// A repository with a worktree in each state, built by the real git.
    #[test]
    fn finished_worktrees_are_merged_with_nothing_to_lose() {
        let Some(git) = Git::find() else {
            eprintln!("no git: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        // The setup is isolated from the user's git configuration (hooks,
        // signing); the check that's tested isn't, as in real use.
        let run = |dir: &Path, args: &[&str]| {
            let st = Command::new(&git.bin)
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };
        let put = |p: PathBuf, text: &str| std::fs::write(p, text).unwrap();
        let wt = |name: &str| repo.join(".claude/worktrees").join(name);

        run(&repo, &["init", "-q", "-b", "main"]);
        put(repo.join("a.txt"), "one\n");
        put(repo.join(".gitignore"), "target/\ngen/\n.env\n");
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-qm", "start"]);
        // What building the project makes, and its settings, in main too.
        std::fs::create_dir(repo.join("gen")).unwrap();
        put(repo.join(".env"), "KEY=1\n");
        for (name, branch) in [
            ("ff", "ff"),
            ("squash", "sq"),
            ("open", "open"),
            ("dirty", "dirty"),
            ("untracked", "untracked"),
            ("ignored", "ignored"),
            ("generated", "generated"),
            ("env", "env"),
            ("locked", "locked"),
        ] {
            let p = wt(name);
            run(
                &repo,
                &["worktree", "add", "-q", "-b", branch, p.to_str().unwrap()],
            );
        }
        let by_hand = dir.path().join("app-editor");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "hand",
                by_hand.to_str().unwrap(),
            ],
        );

        // Merged by a fast-forward.
        put(wt("ff").join("b.txt"), "ff\n");
        run(&wt("ff"), &["add", "."]);
        run(&wt("ff"), &["commit", "-qm", "ff"]);
        run(&repo, &["merge", "-q", "--ff-only", "ff"]);
        // Squashed into main: not contained in it, but already in it.
        put(wt("squash").join("c.txt"), "squash\n");
        run(&wt("squash"), &["add", "."]);
        run(&wt("squash"), &["commit", "-qm", "sq 1"]);
        put(wt("squash").join("c.txt"), "squash 2\n");
        run(&wt("squash"), &["commit", "-qam", "sq 2"]);
        run(&repo, &["merge", "-q", "--squash", "sq"]);
        run(&repo, &["commit", "-qm", "squashed"]);
        // Work that isn't merged.
        put(wt("open").join("d.txt"), "open\n");
        run(&wt("open"), &["add", "."]);
        run(&wt("open"), &["commit", "-qm", "open"]);
        // Merged, but with something that isn't committed.
        put(wt("dirty").join("a.txt"), "changed\n");
        put(wt("untracked").join("new.txt"), "new\n");
        // Build output is ignored, and goes with the worktree.
        std::fs::create_dir(wt("ignored").join("target")).unwrap();
        put(wt("ignored").join("target/big.bin"), "build\n");
        // Generated, as in main; a copy of main's settings.
        std::fs::create_dir(wt("generated").join("gen")).unwrap();
        put(wt("generated").join("gen/code.rs"), "fn x() {}\n");
        put(wt("generated").join(".env"), "KEY=1\n");
        // Settings of its own: ignored, but not to be thrown away.
        put(wt("env").join(".env"), "KEY=other\n");
        run(&repo, &["worktree", "lock", wt("locked").to_str().unwrap()]);

        let objects = || {
            std::fs::read_dir(repo.join(".git/objects"))
                .unwrap()
                .flatten()
                .map(|d| std::fs::read_dir(d.path()).map_or(0, |f| f.count()))
                .sum::<usize>()
        };
        let index = repo.join(".git/worktrees/ignored/index");
        let index_time = || std::fs::metadata(&index).unwrap().modified().unwrap();
        let (objects_before, index_before) = (objects(), index_time());

        let now = crate::watch::unix_now();
        let found = |at: i64| {
            let mut f: Vec<(String, String, bool)> = git
                .finished_in(&repo, at)
                .into_iter()
                .map(|f| {
                    assert_eq!(f.repo, repo);
                    let name = f.path.file_name().unwrap().to_string_lossy().into_owned();
                    (name, f.into, f.agents)
                })
                .collect();
            f.sort();
            f
        };
        let named = |w: &[(&str, bool)]| {
            w.iter()
                .map(|(n, a)| (n.to_string(), "main".to_string(), *a))
                .collect::<Vec<_>>()
        };
        // Everything was just touched: nothing is finished yet.
        assert_eq!(found(now), []);
        let agents = [
            ("ff", true),
            ("generated", true),
            ("ignored", true),
            ("squash", true),
        ];
        assert_eq!(found(now + QUIET + 60), named(&agents));
        // The one made by hand waits longer.
        let mut all = agents.to_vec();
        all.insert(0, ("app-editor", false));
        assert_eq!(found(now + IDLE + 60), named(&all));

        // Checking wrote nothing into the repository.
        assert_eq!(objects(), objects_before);
        assert_eq!(index_time(), index_before);

        // Removing one takes its build output with it, and keeps its branch.
        let ignored = git
            .finished_in(&repo, now + QUIET + 60)
            .into_iter()
            .find(|f| f.path.ends_with("ignored"))
            .unwrap();
        remove(&ignored).unwrap();
        assert!(!wt("ignored").exists());
        assert!(git.ok(
            &repo,
            &["rev-parse", "--verify", "-q", "refs/heads/ignored"]
        ));
        // Whatever was checked, git refuses one with changes.
        let dirty = Finished {
            path: wt("dirty"),
            ..ignored
        };
        let err = remove(&dirty).unwrap_err();
        assert!(err.contains("modified or untracked"), "{err}");
        assert!(wt("dirty").join("a.txt").exists());
    }
}
