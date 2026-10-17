//! Parallel tree walk that measures space honestly.
//!
//! `du` adds up allocated blocks, so APFS clones are counted once per copy
//! and space trapped in a snapshot looks like it belongs to live files. Every
//! file here also carries its *private* size (freed if it were deleted); the
//! rest of its allocation is *shared* with a clone or a snapshot. Shared
//! bytes are counted once per clone family, which gives a total that matches
//! what the volume really uses.
//!
//! The walk never leaves the starting volume, never follows symlinks and
//! never descends into dataless (cloud-only) directories, which would make
//! the File Provider download their listing.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use mdif_macos::bulk::{self, Entry, Kind};
use rayon::prelude::*;
use serde::Serialize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Totals {
    /// What `du` reports.
    pub allocated: u64,
    /// Freed immediately if everything here were deleted.
    pub private: u64,
    /// Sum of file lengths.
    pub logical: u64,
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
}

impl Totals {
    /// Allocated bytes shared with a clone or trapped in a snapshot.
    pub fn shared(&self) -> u64 {
        self.allocated.saturating_sub(self.private)
    }

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
    }

    fn add_file(&mut self, e: &Entry) {
        self.files += 1;
        self.logical += e.logical;
        self.allocated += e.allocated;
        self.private += e.private;
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
    /// Shared bytes (clones, snapshots) counted once per clone family.
    pub shared_once: u64,
    /// Sample of directories we could not read (permissions).
    pub unreadable: Vec<PathBuf>,
}

impl Scan {
    /// Space the scanned tree really uses: private bytes plus shared bytes
    /// counted once.
    pub fn real(&self) -> u64 {
        self.root.totals.private + self.shared_once
    }
}

const UNREADABLE_SAMPLE: usize = 50;

/// Clone family -> bytes shared by its members (counted once).
type Families = HashMap<u64, u64>;

struct Walked {
    node: Node,
    families: Families,
    unreadable: Vec<PathBuf>,
}

pub fn scan(root: &Path, opts: &Options) -> io::Result<Scan> {
    let dev = mdif_macos::fs::dev_of(root)?;
    // Fails loudly instead of reporting an empty tree.
    bulk::open_dir(root)?;
    let w = walk(root, dev, 0, opts);
    Ok(Scan {
        shared_once: w.families.values().sum(),
        root: w.node,
        unreadable: w.unreadable,
    })
}

fn walk(dir: &Path, dev: i32, depth: usize, opts: &Options) -> Walked {
    let mut totals = Totals {
        dirs: 1,
        ..Totals::default()
    };
    let mut families = Families::new();
    let entries = match bulk::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            totals.unreadable_dirs = 1;
            return Walked {
                node: Node {
                    path: dir.to_path_buf(),
                    totals,
                    children: Vec::new(),
                },
                families,
                unreadable: vec![dir.to_path_buf()],
            };
        }
    };

    let mut subdirs = Vec::new();
    for e in &entries {
        match e.kind {
            Kind::Dir if e.dev != dev => totals.other_volumes += 1,
            // Cloud-only folder: its listing isn't local; don't fetch it.
            Kind::Dir if e.is_dataless() => totals.dataless_files += 1,
            Kind::Dir => subdirs.push(dir.join(&e.name)),
            _ => {
                totals.add_file(e);
                let shared = e.shared();
                if shared > 0 {
                    let s = families.entry(e.clone_id).or_default();
                    *s = (*s).max(shared);
                }
            }
        }
    }

    let walked: Vec<Walked> = subdirs
        .par_iter()
        .map(|d| walk(d, dev, depth + 1, opts))
        .collect();
    let mut children = Vec::new();
    let mut unreadable = Vec::new();
    for w in walked {
        totals.add(&w.node.totals);
        merge(&mut families, w.families);
        if unreadable.len() < UNREADABLE_SAMPLE {
            unreadable.extend(w.unreadable);
            unreadable.truncate(UNREADABLE_SAMPLE);
        }
        if depth < opts.max_depth && w.node.totals.allocated >= opts.min_node {
            children.push(w.node);
        }
    }
    children.sort_by_key(|n| std::cmp::Reverse(n.totals.allocated));
    Walked {
        node: Node {
            path: dir.to_path_buf(),
            totals,
            children,
        },
        families,
        unreadable,
    }
}

fn merge(into: &mut Families, mut from: Families) {
    if from.len() > into.len() {
        std::mem::swap(into, &mut from);
    }
    for (id, shared) in from {
        let s = into.entry(id).or_default();
        *s = (*s).max(shared);
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
        assert!(s.real() < tot.allocated);
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
