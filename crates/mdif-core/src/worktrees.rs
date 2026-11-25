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
//! - it isn't locked, and has no submodules (`git worktree remove` refuses
//!   one with them checked out).
//!
//! Git reads, never writes: status runs without optional locks (no index
//! refresh), and the trial merge writes its objects to a scratch folder
//! instead of the repository. Repositories are the projects in the code
//! folders ([`owners::code_folders`]) whose `.git` has worktrees, so git
//! only runs where there are some. Git never fetches, and runs no command a
//! repository's own configuration names: such a repository is left alone
//! ([`Git::runs_its_own_commands`]).
//!
//! [`owners::code_folders`]: crate::owners::code_folders

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::owners::ARTIFACTS;
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
#[cfg(target_os = "macos")]
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
    // `git worktree remove` runs `git status` in it first.
    if git.repo_runs_its_own_commands(&f.repo) || git.runs_its_own_commands(&f.path) {
        return Err(format!(
            "{}: its repository's own settings name a command that git would run (a filter \
             or a merge driver), so it's left to you.",
            f.path.display()
        ));
    }
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
/// In one that git can't be asked about, it counts as tracked. Its name
/// without case, as the disk finds it (`Bin` opens the `bin` git tracks), and
/// as it's spelled (a `*` in it is a `*`).
pub fn tracks_files_in(dir: &Path) -> Option<bool> {
    let (parent, name) = (dir.parent()?, dir.file_name()?);
    parent.ancestors().find(|a| a.join(".git").exists())?;
    let mut spec = std::ffi::OsString::from(":(icase,literal)");
    spec.push(name);
    let tracked = Git::find()
        .and_then(|git| {
            git.out(
                parent,
                &[OsStr::new("ls-files"), "-z".as_ref(), "--".as_ref(), &spec],
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

/// Projects in the code folders ([`crate::owners::code_folders`]) with
/// linked worktrees.
fn repos(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::owners::code_folders(home)
        .into_iter()
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
#[derive(Debug, Clone, PartialEq)]
struct Listed {
    path: PathBuf,
    head: String,
    locked: bool,
    prunable: bool,
}

impl Git {
    fn find() -> Option<Git> {
        #[cfg(target_os = "macos")]
        let candidates = std::env::var_os("DEVELOPER_DIR")
            .map(|d| Path::new(&d).join("usr/bin/git"))
            .into_iter()
            .chain(GITS.iter().map(PathBuf::from));
        // Git for Windows puts itself on PATH; where it doesn't, it's where
        // its installer puts it, for everyone or for the user alone.
        #[cfg(windows)]
        let candidates = crate::tools::on_path("git.exe")
            .into_iter()
            .chain(crate::tools::program_files(r"Git\cmd\git.exe"))
            .chain(crate::os::sys::local_app_data().map(|d| d.join(r"Programs\Git\cmd\git.exe")));
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .map(|bin| Git { bin })
    }

    /// git in `dir`, reading only, in a predictable language, with nothing
    /// inherited that would point it at another repository, read another
    /// configuration file (`GIT_CONFIG`, which only `git config` reads), read
    /// paths another way (a pathspec taken literally, or as a glob) or
    /// history another way (replaced commits, another namespace's branches).
    fn cmd(&self, dir: &Path) -> Command {
        let mut c = Command::new(&self.bin);
        // No pager: choosing one reads the settings with what they include,
        // before `config --no-includes` has a say.
        c.arg("--no-pager")
            .arg("-C")
            .arg(dir)
            .args(["-c", "core.fsmonitor=false"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            // A partial clone fetches an object it's missing (git 2.44 on:
            // older ones don't know it, and partial clones are left alone).
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        c.env("GIT_NO_REPLACE_OBJECTS", "1");
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG",
            "GIT_LITERAL_PATHSPECS",
            "GIT_GLOB_PATHSPECS",
            "GIT_NOGLOB_PATHSPECS",
            "GIT_ICASE_PATHSPECS",
            "GIT_NAMESPACE",
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
        if self.repo_runs_its_own_commands(repo) {
            return Vec::new();
        }
        let Some(listed) = self.out(repo, &["worktree", "list", "--porcelain", "-z"]) else {
            return Vec::new();
        };
        let defaults = self.defaults(repo);
        if defaults.is_empty() {
            return Vec::new();
        }
        let objects = repo.join(".git/objects");
        let all = parse_list(&listed);
        // Removing one takes every worktree inside it, and what they hold.
        let holds_another = |w: &Listed| {
            all.iter()
                .any(|o| o.path != w.path && o.path.starts_with(&w.path))
        };
        let mut list = all.iter().cloned();
        // The first is the main worktree.
        let Some(main) = list.next() else {
            return Vec::new();
        };
        list.filter(|w| !w.locked && !w.prunable && !holds_another(w) && privacy::may_open(&w.path))
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
        // What git uses there (a worktree's `.git` file may name another
        // repository). A submodule: `git worktree remove` refuses one with
        // its submodules checked out, and looking into them would run git in
        // each, with their own settings.
        if self.runs_its_own_commands(worktree) || self.has_submodules(worktree) {
            return false;
        }
        let Some(out) = self.out(
            worktree,
            &[
                "status",
                "--porcelain",
                "-z",
                "--untracked-files=normal",
                "--ignored=matching",
                // A submodule's commit as recorded (a staged removal of one
                // shows), never what's in it: that's git run in it.
                "--ignore-submodules=dirty",
            ],
        ) else {
            return false;
        };
        // "XY path", NUL-ended; an ignored folder is listed, not what's in it.
        out.split(|&b| b == 0).filter(|e| !e.is_empty()).all(|e| {
            e.strip_prefix(b"!! ").is_some_and(|rel| {
                let rel = crate::portable::os_str_from_bytes(rel.strip_suffix(b"/").unwrap_or(rel));
                let rel = Path::new(&*rel);
                remade(worktree, main, rel)
            })
        })
    }
}

/// Whether `rel`, ignored in `worktree`, can be made again: build output, a
/// link, Finder's view settings, or what the main checkout has the same: a
/// file with the same bytes, a folder whose every file main has the same.
/// A folder of the same name isn't enough: `.idea/shelf`'s patches, a clone
/// with commits of its own or a run's outputs are only in the worktree.
fn remade(worktree: &Path, main: &Path, rel: &Path) -> bool {
    let mut budget = REMADE_ENTRIES;
    same_in_main(worktree, main, rel, &mut budget)
}

/// The most entries of one ignored folder [`remade`] looks at; past it, the
/// folder is taken as the worktree's own.
const REMADE_ENTRIES: usize = 5_000;

/// The biggest file compared byte for byte; a bigger one (a database, a
/// model) is taken as the worktree's own.
const REMADE_BYTES: u64 = 256 * 1024;

fn same_in_main(worktree: &Path, main: &Path, rel: &Path, budget: &mut usize) -> bool {
    if rel
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|n| ARTIFACTS.contains(&n) || n == ".DS_Store")
    {
        return true;
    }
    let Some(left) = budget.checked_sub(1) else {
        return false;
    };
    *budget = left;
    let here_at = worktree.join(rel);
    let Ok(here) = std::fs::symlink_metadata(&here_at) else {
        return false;
    };
    if here.is_symlink() {
        return true;
    }
    // A cloud file or folder, on either side: listing or reading it may
    // download it (Windows has no switch to forbid that), and what's online
    // only is no copy here.
    let there_at = main.join(rel);
    let managed =
        |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| crate::portable::managed(&m));
    if crate::portable::managed(&here) || managed(&there_at) {
        return false;
    }
    let Ok(there) = std::fs::metadata(&there_at) else {
        return false;
    };
    if here.is_dir() {
        return there.is_dir()
            && std::fs::read_dir(&here_at).is_ok_and(|entries| {
                entries.into_iter().all(|e| {
                    e.is_ok_and(|e| same_in_main(worktree, main, &rel.join(e.file_name()), budget))
                })
            });
    }
    here.is_file()
        && there.is_file()
        && here.len() == there.len()
        && here.len() <= REMADE_BYTES
        && same_bytes(&here_at, &there_at)
}

/// Whether two small files hold the same bytes. Read with materialization
/// forbidden: a cloud file not on this Mac fails to read, and is no copy.
fn same_bytes(a: &Path, b: &Path) -> bool {
    if crate::os::fs::forbid_dataless_materialization().is_err() {
        return false;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

impl Git {
    /// Whether the configuration git uses in `dir` sets, of the
    /// repository's own, a command that checking it could run
    /// ([`names_command`]), or git can't say. Asked of git, which reads it as
    /// the commands that follow will: after a byte-order mark, with two
    /// sections on a line, the repository a worktree's `.git` file names. An archive unpacked into a code folder brings its
    /// `.git` with it, and nobody asked to run what that says: such a
    /// repository is left alone, none of its worktrees ever finished.
    fn runs_its_own_commands(&self, dir: &Path) -> bool {
        // An include is refused, so what it names isn't opened (it may be in
        // a folder that asks for permission first, and nobody's there).
        let list = [
            "config",
            "--list",
            "--no-includes",
            "--show-scope",
            "--name-only",
            "-z",
        ];
        self.out(dir, &list)
            .is_none_or(|listed| own_commands(&listed))
    }

    /// [`Git::runs_its_own_commands`] for the repository at `repo` and the
    /// submodules' repositories it keeps (`.git/modules`), which a trial
    /// merge opens.
    fn repo_runs_its_own_commands(&self, repo: &Path) -> bool {
        self.runs_its_own_commands(repo)
            || submodule_repos(&repo.join(".git"))
                .is_none_or(|subs| subs.iter().any(|s| self.runs_its_own_commands(s)))
    }

    /// Whether `worktree` has a submodule (or git can't say).
    fn has_submodules(&self, worktree: &Path) -> bool {
        self.out(worktree, &["ls-files", "--stage", "-z"])
            .is_none_or(|listed| listed.split(|&b| b == 0).any(|e| e.starts_with(b"160000 ")))
    }
}

/// Submodules' repositories (`modules/<name>`; a name may have folders in it,
/// and theirs are in `modules/<name>/modules`), or `None` past a bound.
fn submodule_repos(git_dir: &Path) -> Option<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![git_dir.join("modules")];
    let mut looked = 0;
    while let Some(dir) = stack.pop() {
        looked += 1;
        if looked > 1_000 {
            return None;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let at = e.path();
            if at.join("HEAD").is_file() {
                stack.push(at.join("modules"));
                found.push(at);
            } else {
                stack.push(at);
            }
        }
    }
    Some(found)
}

/// Whether `git config --list --show-scope --name-only -z` (a scope and a
/// name, each ended by NUL) lists a name that [`names_command`] set by the
/// repository: its own files and what they include. What the user set
/// (`global`, `system`; git-lfs's filter is set globally) runs as it would
/// for them, and `command` is what mdif passes.
fn own_commands(listed: &[u8]) -> bool {
    let mut fields = listed.split(|&b| b == 0);
    while let (Some(scope), Some(name)) = (fields.next(), fields.next()) {
        let users = matches!(scope, b"global" | b"system" | b"command" | b"unknown");
        if !users && names_command(&String::from_utf8_lossy(name)) {
            return true;
        }
    }
    false
}

/// Whether a configuration name (`filter.lfs.clean`, as git lists it) sets a
/// command that what mdif runs could start, or pulls in another file:
/// - a filter, run on a file `git status` compares with what's committed;
/// - a merge driver (`merge.<driver>.driver`), run by the trial merge;
/// - a promisor remote (a partial clone): an object it's missing is
///   fetched, with whatever transport the remote names;
/// - an include, for what it might bring in later (a conditional one).
///
/// `core.fsmonitor` is turned off on git's command line, and nothing mdif
/// runs writes the index (the hook that would run then) or shows a diff.
fn names_command(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let (section, rest) = name.split_once('.').unwrap_or((name.as_str(), ""));
    // `merge.<driver>.driver` has a subsection; `merge.conflictstyle` doesn't.
    let subsection = rest.contains('.');
    match section {
        "filter" | "include" | "includeif" => true,
        // A driver, or which one files without an attribute get.
        "merge" => subsection || rest == "default",
        "extensions" => rest == "partialclone",
        "remote" => {
            subsection && (rest.ends_with(".promisor") || rest.ends_with(".partialclonefilter"))
        }
        _ => false,
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
                    path: PathBuf::from(crate::portable::os_str_from_bytes(value).into_owned()),
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
    let text = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let admin = worktree.join(text.strip_prefix("gitdir:")?.trim());
    let mtime = |p: PathBuf| {
        std::fs::symlink_metadata(p)
            .ok()
            .map(|m| crate::portable::mtime(&m))
    };
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

    /// Where the disk ignores case, "Projects" and "projects" are one folder:
    /// its repositories are listed once, as the disk spells them, or each
    /// finished worktree would be suggested twice.
    #[test]
    fn a_repository_is_found_once_whatever_its_code_folders_case() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        for repo in ["Projects/shop", "CODE/tool", "Elsewhere/site"] {
            std::fs::create_dir_all(h.join(repo).join(".git/worktrees/feature")).unwrap();
        }
        std::fs::create_dir_all(h.join("Projects/plain/.git")).unwrap();
        assert_eq!(repos(h), [h.join("CODE/tool"), h.join("Projects/shop")]);
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
                // No maintenance after a commit or a merge: run in the
                // background, it would pack the objects a test counts.
                .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
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
        // As the disk finds it, without case; as it's spelled, with no
        // wildcards.
        assert_eq!(tracks_files_in(&repo.join("Dist")), Some(true));
        assert_eq!(tracks_files_in(&repo.join("d*")), Some(false));
        assert!(!crate::clean::build_output(&repo.join("dist")));
        assert!(crate::clean::build_output(&repo.join("build")));
    }

    /// The setup's git, isolated from the user's configuration (hooks,
    /// signing); what's tested isn't, as in real use.
    fn setup_git(git: &Git, dir: &Path, args: &[&str]) {
        let st = Command::new(&git.bin)
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            // No maintenance after a commit or a merge: run in the
            // background, it would pack the objects a test counts.
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    /// An npm project's `build` is also where electron-builder keeps what an
    /// app is built from: build output only when git ignores it.
    #[test]
    fn an_npm_projects_build_folder_is_output_only_when_ignored() {
        let Some(git) = Git::find() else {
            eprintln!("no git: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("electron-app");
        let build = app.join("build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("icon.icns"), "icon").unwrap();
        std::fs::write(app.join("package.json"), "{}").unwrap();
        // Outside a repository nothing says which it is.
        assert!(!crate::clean::build_output(&build));
        // In one, not ignored: an icon not committed yet.
        setup_git(&git, &app, &["init", "-q", "-b", "main"]);
        assert!(!crate::clean::build_output(&build));
        std::fs::write(app.join(".gitignore"), "/build\n").unwrap();
        assert!(crate::clean::build_output(&build));
        // Beside a Gradle file it's what Gradle builds, ignored or not.
        std::fs::write(app.join(".gitignore"), "").unwrap();
        std::fs::write(app.join("build.gradle"), "").unwrap();
        assert!(crate::clean::build_output(&build));
    }

    /// Removing a worktree takes every worktree inside it, and their work:
    /// one that holds another isn't finished, however merged.
    #[test]
    fn a_worktree_holding_another_is_not_finished() {
        let Some(git) = Git::find() else {
            eprintln!("no git: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        setup_git(&git, &repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        std::fs::write(repo.join(".gitignore"), ".claude/worktrees/\n").unwrap();
        setup_git(&git, &repo, &["add", "."]);
        setup_git(&git, &repo, &["commit", "-qm", "start"]);
        let outer = repo.join(".claude/worktrees/outer");
        let add = |at: &Path, branch: &str, path: &Path| {
            setup_git(
                &git,
                at,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    path.to_str().unwrap(),
                ],
            );
        };
        add(&repo, "outer", &outer);
        let later = crate::watch::unix_now() + QUIET + 60;
        let finished = || {
            git.finished_in(&repo, later)
                .into_iter()
                .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        // On its own it's finished: main's commit, nothing else in it.
        assert_eq!(finished(), ["outer"]);
        // A session in it starts another inside it, with work of its own.
        let inner = outer.join(".claude/worktrees/inner");
        add(&outer, "inner", &inner);
        std::fs::write(inner.join("notes.md"), "not committed\n").unwrap();
        assert_eq!(finished(), Vec::<String>::new());
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
                // No maintenance after a commit or a merge: run in the
                // background, it would pack the objects a test counts.
                .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
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
        put(repo.join("gen/code.rs"), "fn x() {}\n");
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
            ("same-size", "same-size"),
            ("own-files", "own-files"),
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
        // The same size as main's, but not the same settings.
        put(wt("same-size").join(".env"), "KEY=2\n");
        // A folder main has too, with a file only the worktree has.
        std::fs::create_dir(wt("own-files").join("gen")).unwrap();
        put(wt("own-files").join("gen/code.rs"), "fn x() {}\n");
        put(wt("own-files").join("gen/notes.md"), "mine\n");
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

        // A command of the repository's own, which checking would run: the
        // repository is left alone, and it never runs. Wherever git finds it:
        // after a byte-order mark, after another section on its line, with
        // only carriage returns between lines, in a file it includes, in a
        // worktree's own settings. (Quoted: in git's files `;` starts a
        // comment, and the probe would be cut short and never run.)
        let ran = dir.path().join("ran");
        let slashed = |p: &Path| p.display().to_string().replace('\\', "/");
        // The path quoted for sh (a temporary folder may have a space).
        let filter = format!(
            "[filter \"probe\"]\n\tclean = \"sh -c 'touch \\\"{}\\\"; cat'\"\n",
            slashed(&ran)
        );
        std::fs::create_dir_all(repo.join(".git/info")).unwrap();
        put(repo.join(".git/info/attributes"), "* filter=probe\n");
        let config = std::fs::read_to_string(repo.join(".git/config")).unwrap();
        let included = dir.path().join("included.cfg");
        put(included.clone(), &filter);
        let worktree_config = repo.join(".git/worktrees/ff/config.worktree");
        let layouts = [
            ("after the rest", format!("{config}{filter}"), None),
            (
                "after a byte-order mark",
                format!("\u{feff}{filter}{config}"),
                None,
            ),
            (
                "on another section's line",
                format!("{config}[user] {filter}"),
                None,
            ),
            (
                "between carriage returns",
                format!("{config}{}", filter.replace('\n', "\r")),
                None,
            ),
            (
                "in a file it includes",
                format!("{config}[include]\n\tpath = \"{}\"\n", slashed(&included)),
                None,
            ),
            (
                "in a worktree's own settings",
                format!(
                    "{}[extensions]\n\tworktreeConfig = true\n",
                    config.replace("repositoryformatversion = 0", "repositoryformatversion = 1")
                ),
                Some(filter.as_str()),
            ),
        ];
        let probe = || {
            // A file read again, so status compares it (with the filter).
            put(wt("ff").join("a.txt"), "one\n");
            let _ = std::fs::remove_file(&ran);
        };
        for (how, text, in_worktree) in &layouts {
            put(repo.join(".git/config"), text);
            if let Some(f) = in_worktree {
                put(worktree_config.clone(), f);
            }
            // The probe works: git's own status runs it.
            probe();
            assert!(
                git.out(&wt("ff"), &["status", "--porcelain"]).is_some(),
                "{how}"
            );
            assert!(ran.exists(), "the probe didn't run {how}");
            probe();
            assert!(!found(now + IDLE + 60).iter().any(|f| f.0 == "ff"), "{how}");
            assert!(!ran.exists(), "checking ran it {how}");
            let _ = std::fs::remove_file(&worktree_config);
        }
        // Without it, the same worktree is finished.
        put(repo.join(".git/config"), &config);
        probe();
        assert!(found(now + IDLE + 60).iter().any(|f| f.0 == "ff"));
    }

    #[test]
    fn a_repositorys_own_commands_are_told_from_the_users() {
        for name in [
            "filter.lfs.clean",
            "filter.x.process",
            "merge.ours.driver",
            "Merge.Ours.Driver",
            "merge.default",
            "include.path",
            "includeif.gitdir:~/w/.path",
            "extensions.partialclone",
            "remote.origin.promisor",
            "remote.my.fork.partialclonefilter",
        ] {
            assert!(names_command(name), "{name}");
        }
        for name in [
            "core.bare",
            "merge.conflictstyle",
            "remote.origin.url",
            "remote.promisor.url",
            "user.name",
            "extensions.worktreeconfig",
        ] {
            assert!(!names_command(name), "{name}");
        }
        // Only what the repository sets: the user's own filter (git-lfs's,
        // set globally) runs as it would for them.
        let listed = |pairs: &[(&str, &str)]| -> Vec<u8> {
            pairs
                .iter()
                .flat_map(|(scope, name)| format!("{scope}\0{name}\0").into_bytes())
                .collect()
        };
        assert!(!own_commands(&listed(&[
            ("global", "filter.lfs.clean"),
            ("system", "filter.lfs.process"),
            ("command", "core.fsmonitor"),
            ("local", "core.bare"),
        ])));
        assert!(own_commands(&listed(&[
            ("local", "core.bare"),
            ("local", "filter.x.clean"),
        ])));
        assert!(own_commands(&listed(&[("worktree", "merge.x.driver")])));
        assert!(!own_commands(b""));

        // A submodule's repository has its own settings; a submodule is
        // told by the index.
        let Some(git) = Git::find() else {
            eprintln!("no git: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        let run = |args: &[&str]| {
            let st = Command::new(&git.bin)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };
        run(&["init", "-q", repo.to_str().unwrap()]);
        let sub = repo.join(".git/modules/libs/one");
        run(&["init", "-q", "--bare", sub.to_str().unwrap()]);
        assert!(!git.repo_runs_its_own_commands(&repo));
        let config = std::fs::read_to_string(sub.join("config")).unwrap();
        std::fs::write(
            sub.join("config"),
            format!("{config}[filter \"x\"]\n\tsmudge = x\n"),
        )
        .unwrap();
        assert!(git.repo_runs_its_own_commands(&repo));
        // Settings git can't read: it can't say what they'd run.
        std::fs::write(sub.join("config"), "[broken\n").unwrap();
        assert!(git.repo_runs_its_own_commands(&repo));
        std::fs::write(sub.join("config"), config).unwrap();

        // An include is refused without being opened: it may be in a folder
        // that asks for permission first. One git can't open says so.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let kept = dir.path().join("kept.gitconfig");
            std::fs::write(&kept, "[filter \"x\"]\n\tclean = x\n").unwrap();
            std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o000)).unwrap();
            let config = repo.join(".git/config");
            let before = std::fs::read_to_string(&config).unwrap();
            std::fs::write(
                &config,
                format!("{before}[include]\n\tpath = \"{}\"\n", kept.display()),
            )
            .unwrap();
            assert!(git.runs_its_own_commands(&repo));
            let o = git
                .cmd(&repo)
                .args(["config", "--list", "--no-includes", "--name-only"])
                .stderr(Stdio::piped())
                .output()
                .unwrap();
            let said = String::from_utf8_lossy(&o.stderr);
            assert!(o.status.success() && said.is_empty(), "{said}");
            assert!(String::from_utf8_lossy(&o.stdout).contains("include.path"));
            std::fs::write(&config, before).unwrap();
            std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(!git.repo_runs_its_own_commands(&repo));

        assert!(!git.has_submodules(&repo));
        let at = repo.to_str().unwrap();
        run(&[
            "-C",
            at,
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1234567890123456789012345678901234567890,libs/one",
        ]);
        assert!(git.has_submodules(&repo));
    }
}
