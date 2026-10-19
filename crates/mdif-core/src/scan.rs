//! Parallel tree walk that measures space honestly.
//!
//! `du` adds up allocated blocks, so APFS clones are counted once per copy,
//! hard links once per name, and space trapped in a snapshot looks like it
//! belongs to live files. Here:
//!
//! - every file also carries its *private* size, what deleting it frees;
//! - a hard-linked file is counted once, and its space is only called
//!   reclaimable when all its links are inside the scanned folder;
//! - files that may share blocks with clones are measured by their physical
//!   extents, so a modified clone (which no longer has its original's clone
//!   ID but still shares most of its blocks) is counted once too. When the
//!   extents can't be read the overlap may be counted twice, and how much
//!   is reported as uncertainty instead of hidden.
//!
//! The walk never leaves the starting volume, never follows symlinks and
//! never descends into dataless (cloud-only) directories, which would make
//! the File Provider download their listing.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use mdif_macos::bulk::{self, EF_MAY_SHARE_BLOCKS, Entry, Kind};
use mdif_macos::extents;
use rayon::prelude::*;
use serde::Serialize;

/// APFS allocation block.
const BLOCK: u64 = 4096;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Totals {
    /// What `du` reports, hard links counted once.
    pub allocated: u64,
    /// Freed immediately if everything here were deleted.
    pub private: u64,
    /// Sum of file lengths.
    pub logical: u64,
    /// Files (a hard-linked file counts once).
    pub files: u64,
    pub dirs: u64,
    /// Cloud placeholders: listed here, content not on this disk.
    pub dataless_files: u64,
    pub dataless_logical: u64,
    /// Allocated bytes in files macOS may delete on its own under pressure.
    pub purgeable: u64,
    pub unreadable_dirs: u64,
    /// Mount points of other volumes, skipped.
    pub other_volumes: u64,
    /// Private bytes of hard-linked files that also have links outside this
    /// folder: deleting the folder doesn't free them.
    pub linked_elsewhere: u64,
}

impl Totals {
    fn add(&mut self, o: &Totals) {
        self.allocated += o.allocated;
        self.private += o.private;
        self.logical += o.logical;
        self.files += o.files;
        self.dirs += o.dirs;
        self.dataless_files += o.dataless_files;
        self.dataless_logical += o.dataless_logical;
        self.purgeable += o.purgeable;
        self.unreadable_dirs += o.unreadable_dirs;
        self.other_volumes += o.other_volumes;
        self.linked_elsewhere += o.linked_elsewhere;
    }

    /// One file, all of whose links are in this folder.
    fn add_file(&mut self, e: &Entry) {
        self.add_linked(e);
        self.private += e.private;
    }

    /// One file, counted without its private bytes being reclaimable.
    fn add_linked(&mut self, e: &Entry) {
        self.files += 1;
        self.logical += e.logical;
        self.allocated += e.allocated;
        if e.is_dataless() {
            self.dataless_files += 1;
            self.dataless_logical += e.logical;
        }
        if e.is_purgeable() {
            self.purgeable += e.allocated;
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub path: PathBuf,
    pub totals: Totals,
    /// Largest first. Only kept down to `Options::max_depth` and for
    /// subtrees of at least `Options::min_node` allocated bytes; the rest is
    /// still included in `totals`.
    pub children: Vec<Node>,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub max_depth: usize,
    pub min_node: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_depth: 6,
            min_node: 50_000_000,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Scan {
    pub root: Node,
    /// Bytes the tree really occupies: every block once, however many
    /// clones, hard links or snapshots share it.
    pub real: u64,
    /// The part of `real` that isn't private: blocks shared with clones or
    /// held by a snapshot (counted once), and files linked from elsewhere.
    pub shared_once: u64,
    /// Bytes of possibly cloned files whose physical layout couldn't be
    /// read. They are counted in full, so `real` may be up to this much too
    /// high.
    pub shared_uncertain: u64,
    /// Sample of directories we could not read (permissions).
    pub unreadable: Vec<PathBuf>,
}

const UNREADABLE_SAMPLE: usize = 50;

/// A hard-linked file whose links haven't all been seen yet.
struct Link {
    seen: u32,
    entry: Entry,
    path: PathBuf,
}

type Links = HashMap<(i32, u64), Link>;

/// Physical accounting shared by the whole walk.
struct Ctx<'a> {
    dev: i32,
    opts: &'a Options,
    /// Blocks of files that share nothing with other live files.
    plain: AtomicU64,
    clones: Mutex<Clones>,
}

#[derive(Default)]
struct Clones {
    /// Clone IDs already measured: pure clones have identical extents.
    seen: HashSet<u64>,
    extents: Vec<(u64, u64)>,
    uncertain: u64,
}

impl Ctx<'_> {
    /// Count the blocks of one file (once per inode).
    fn account(&self, path: &Path, e: &Entry) {
        if e.is_dataless() {
            return;
        }
        if e.kind != Kind::File || e.ext_flags & EF_MAY_SHARE_BLOCKS == 0 || e.shared() == 0 {
            // Its shared bytes, if any, are only shared with a snapshot.
            self.plain.fetch_add(e.allocated, Ordering::Relaxed);
            return;
        }
        if !self.clones.lock().unwrap().seen.insert(e.clone_id) {
            return;
        }
        match extents::physical(path, BLOCK) {
            Ok(mut x) => {
                let mapped = extents::union_len(&mut x);
                // Allocated but not mapped to extents: APFS reserves the
                // tail of small sparse files.
                self.plain
                    .fetch_add(e.allocated.saturating_sub(mapped), Ordering::Relaxed);
                self.clones.lock().unwrap().extents.extend(x);
            }
            Err(_) => {
                self.plain.fetch_add(e.allocated, Ordering::Relaxed);
                self.clones.lock().unwrap().uncertain += e.shared();
            }
        }
    }
}

struct Walked {
    /// Totals as reported for this folder.
    node: Node,
    /// The same without the hard links still in `links`, which the parent
    /// adds once it has merged its children's.
    base: Totals,
    links: Links,
    unreadable: Vec<PathBuf>,
}

/// Allocated bytes of `path`, a folder (every file in it once) or a file: 0
/// when it doesn't exist, `None` when it can't be read.
pub fn size_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    // Measuring must never download a cloud placeholder.
    mdif_macos::fs::forbid_dataless_materialization().ok()?;
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {
            let opts = Options {
                max_depth: 0,
                min_node: u64::MAX,
            };
            scan(path, &opts).ok().map(|s| s.root.totals.allocated)
        }
        Ok(m) => Some(m.blocks() * 512),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(0),
        Err(_) => None,
    }
}

pub fn scan(root: &Path, opts: &Options) -> io::Result<Scan> {
    let dev = mdif_macos::fs::dev_of(root)?;
    // Fails loudly instead of reporting an empty tree.
    bulk::open_dir(root)?;
    let ctx = Ctx {
        dev,
        opts,
        plain: AtomicU64::new(0),
        clones: Mutex::new(Clones::default()),
    };
    let w = walk(root, 0, &ctx);
    // Links to these files live outside the scanned tree (or in folders we
    // couldn't read), but their blocks are still used here.
    for l in w.links.values() {
        ctx.account(&l.path, &l.entry);
    }
    let mut clones = ctx.clones.into_inner().unwrap();
    let real = ctx.plain.into_inner() + extents::union_len(&mut clones.extents);
    let t = w.node.totals;
    Ok(Scan {
        real,
        shared_once: real.saturating_sub(t.private),
        shared_uncertain: clones.uncertain,
        root: w.node,
        unreadable: w.unreadable,
    })
}

fn walk(dir: &Path, depth: usize, ctx: &Ctx) -> Walked {
    let mut base = Totals {
        dirs: 1,
        ..Totals::default()
    };
    let mut links = Links::new();
    let entries = match bulk::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            base.unreadable_dirs = 1;
            return Walked {
                node: Node {
                    path: dir.to_path_buf(),
                    totals: base,
                    children: Vec::new(),
                },
                base,
                links,
                unreadable: vec![dir.to_path_buf()],
            };
        }
    };

    let mut subdirs = Vec::new();
    for e in entries {
        match e.kind {
            Kind::Dir if e.dev != ctx.dev => base.other_volumes += 1,
            // Cloud-only folder: its listing isn't local; don't fetch it.
            Kind::Dir if e.is_dataless() => base.dataless_files += 1,
            Kind::Dir => subdirs.push(dir.join(&e.name)),
            _ if e.nlink > 1 => {
                let path = dir.join(&e.name);
                links
                    .entry((e.dev, e.inode))
                    .and_modify(|l| l.seen += 1)
                    .or_insert(Link {
                        seen: 1,
                        entry: e,
                        path,
                    });
            }
            _ => {
                base.add_file(&e);
                ctx.account(&dir.join(&e.name), &e);
            }
        }
    }

    let walked: Vec<Walked> = subdirs
        .par_iter()
        .map(|d| walk(d, depth + 1, ctx))
        .collect();
    let mut children = Vec::new();
    let mut unreadable = Vec::new();
    for w in walked {
        base.add(&w.base);
        merge(&mut links, w.links);
        if unreadable.len() < UNREADABLE_SAMPLE {
            unreadable.extend(w.unreadable);
            unreadable.truncate(UNREADABLE_SAMPLE);
        }
        if depth < ctx.opts.max_depth && w.node.totals.allocated >= ctx.opts.min_node {
            children.push(w.node);
        }
    }
    // Hard links all found below here: count the file once, as ours.
    links.retain(|_, l| {
        if l.seen < l.entry.nlink {
            return true;
        }
        base.add_file(&l.entry);
        ctx.account(&l.path, &l.entry);
        false
    });
    let mut totals = base;
    for l in links.values() {
        totals.add_linked(&l.entry);
        totals.linked_elsewhere += l.entry.private;
    }
    children.sort_by_key(|n| std::cmp::Reverse(n.totals.allocated));
    Walked {
        node: Node {
            path: dir.to_path_buf(),
            totals,
            children,
        },
        base,
        links,
        unreadable,
    }
}

fn merge(into: &mut Links, mut from: Links) {
    if from.len() > into.len() {
        std::mem::swap(into, &mut from);
    }
    for (k, l) in from {
        match into.get_mut(&k) {
            Some(x) => x.seen += l.seen,
            None => {
                into.insert(k, l);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn write(path: &Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Non-repeating content so APFS can't compress it away.
        let data: Vec<u8> = (0..len as u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        fs::write(path, data).unwrap();
        // APFS reports a private size of 0 until dirty data is flushed.
        fs::File::open(path).unwrap().sync_all().unwrap();
    }

    fn opts(min: u64) -> Options {
        Options {
            max_depth: 6,
            min_node: min,
        }
    }

    #[test]
    fn clones_are_counted_once() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("a/orig.bin"), 8_000_000);
        fs::create_dir(t.path().join("b")).unwrap();
        assert!(
            Command::new("cp")
                .arg("-c")
                .arg(t.path().join("a/orig.bin"))
                .arg(t.path().join("b/clone.bin"))
                .status()
                .unwrap()
                .success()
        );
        write(&t.path().join("c/own.bin"), 2_000_000);

        let s = scan(t.path(), &opts(0)).unwrap();
        let tot = s.root.totals;
        assert_eq!(tot.files, 3);
        // du-style allocation counts the clone twice ...
        assert!(tot.allocated >= 18_000_000, "{tot:?}");
        // ... but only the independent file is private ...
        assert!(
            tot.private >= 2_000_000 && tot.private < 3_000_000,
            "{tot:?}"
        );
        // ... and the clone pair is counted once.
        assert!(
            s.shared_once >= 8_000_000 && s.shared_once < 9_000_000,
            "{}",
            s.shared_once
        );
        assert!(s.real < tot.allocated);
        assert_eq!(s.shared_uncertain, 0);
    }

    fn clone(from: &Path, to: &Path) {
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        assert!(
            Command::new("cp")
                .arg("-c")
                .arg(from)
                .arg(to)
                .status()
                .unwrap()
                .success()
        );
    }

    const MIB: u64 = 1 << 20;

    #[test]
    fn a_modified_clone_is_counted_once() {
        // Clone 8 MiB, then overwrite 1 MiB of the clone: it gets its own
        // clone ID but still shares 7 MiB with the original.
        let t = tempfile::tempdir().unwrap();
        let (orig, copy) = (t.path().join("a/orig.bin"), t.path().join("b/copy.bin"));
        write(&orig, 8 * MIB as usize);
        clone(&orig, &copy);
        let f = fs::OpenOptions::new().write(true).open(&copy).unwrap();
        std::os::unix::fs::FileExt::write_all_at(&f, &vec![1u8; MIB as usize], 2 * MIB).unwrap();
        f.sync_all().unwrap();

        let s = scan(t.path(), &opts(0)).unwrap();
        assert_eq!(s.root.totals.allocated, 16 * MIB, "du counts both copies");
        assert_eq!(s.root.totals.private, 2 * MIB, "1 MiB of each is its own");
        assert_eq!(s.real, 9 * MIB, "7 MiB shared + 1 + 1");
        assert_eq!(s.shared_once, 7 * MIB);
        assert_eq!(s.shared_uncertain, 0);
    }

    #[test]
    fn hard_links_are_counted_once() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("x/data.bin"), t.path().join("y/link.bin"));
        write(&a, 8 * MIB as usize);
        fs::create_dir(t.path().join("y")).unwrap();
        fs::hard_link(&a, &b).unwrap();

        // Both links inside: one file, fully reclaimable.
        let s = scan(t.path(), &opts(0)).unwrap();
        let tot = s.root.totals;
        assert_eq!(
            (tot.files, tot.allocated, tot.private),
            (1, 8 * MIB, 8 * MIB)
        );
        assert_eq!(tot.linked_elsewhere, 0);
        assert_eq!(s.real, 8 * MIB);
        // Each child holds one link: there, deleting it frees nothing.
        for child in &s.root.children {
            let c = child.totals;
            assert_eq!(
                (c.files, c.allocated, c.private),
                (1, 8 * MIB, 0),
                "{child:?}"
            );
            assert_eq!(c.linked_elsewhere, 8 * MIB);
        }

        // Scanning only one side: the file is used here but not reclaimable.
        let s = scan(&t.path().join("y"), &opts(0)).unwrap();
        let tot = s.root.totals;
        assert_eq!(
            (tot.allocated, tot.private, tot.linked_elsewhere),
            (8 * MIB, 0, 8 * MIB)
        );
        assert_eq!(s.real, 8 * MIB);
    }

    #[test]
    fn small_sparse_files_count_what_apfs_reserves() {
        // APFS reserves the whole length of a small sparse file although its
        // private size and extents cover only the written part.
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("sparse.bin");
        let f = fs::File::create(&p).unwrap();
        f.set_len(16 * MIB).unwrap();
        std::os::unix::fs::FileExt::write_all_at(&f, &[1u8; 4096], 4 * MIB).unwrap();
        f.sync_all().unwrap();
        let allocated = std::os::unix::fs::MetadataExt::blocks(&f.metadata().unwrap()) * 512;
        let s = scan(t.path(), &opts(0)).unwrap();
        assert_eq!(s.real, allocated);
    }

    #[test]
    fn tree_is_sorted_and_pruned_but_totals_stay_complete() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("big/x.bin"), 3_000_000);
        write(&t.path().join("small/y.bin"), 10_000);
        write(&t.path().join("top.bin"), 1_000);

        let s = scan(t.path(), &opts(1_000_000)).unwrap();
        let names: Vec<_> = s
            .root
            .children
            .iter()
            .map(|n| n.path.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["big"], "small subtree pruned from the tree");
        assert_eq!(s.root.totals.files, 3, "but still counted");
        assert_eq!(s.root.totals.dirs, 3);
    }

    #[test]
    fn unreadable_dirs_are_reported_not_fatal() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("locked/secret.bin"), 1_000);
        let locked = t.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let s = scan(t.path(), &opts(0));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let s = s.unwrap();
        assert_eq!(s.root.totals.unreadable_dirs, 1);
        assert_eq!(s.unreadable, [locked]);
    }

    #[test]
    fn symlinks_are_not_followed() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("real/data.bin"), 4_000_000);
        std::os::unix::fs::symlink(t.path().join("real"), t.path().join("alias")).unwrap();
        let s = scan(t.path(), &opts(0)).unwrap();
        assert_eq!(s.root.totals.files, 2, "data.bin + the symlink itself");
        assert!(s.root.totals.allocated < 5_000_000);
    }
}
