//! `mdif map`: the reconciled disk map.

use std::cmp::Reverse;

use mdif_core::map::{self, DiskMap};
use mdif_core::scan::Node;
use mdif_core::text::printable;
use mdif_core::units::{bytes, signed};

pub fn print_map(m: &DiskMap, depth: usize) {
    let c = &m.container;
    println!(
        "Disk {}  {}  ·  {} used  ·  {} free",
        c.container_reference,
        bytes(c.capacity_ceiling),
        bytes(c.used()),
        bytes(c.capacity_free)
    );
    let mut vols: Vec<_> = c.volumes.iter().collect();
    vols.sort_by_key(|v| Reverse(v.capacity_in_use));
    let vols: Vec<String> = vols
        .iter()
        .map(|v| format!("{} {}", v.label(), bytes(v.capacity_in_use)))
        .collect();
    println!("  volumes: {}", vols.join(" · "));
    if m.snapshots.is_empty() {
        println!("  local Time Machine snapshots: none");
    } else {
        println!(
            "  local Time Machine snapshots: {} (they keep deleted files' space allocated)",
            m.snapshots.len()
        );
    }

    let t = &m.scan.root.totals;
    let whole_volume = m.root == std::path::Path::new(map::DATA_ROOT);
    println!();
    println!(
        "{} volume: {} in use",
        m.volume.label(),
        bytes(m.volume.capacity_in_use)
    );
    println!(
        "  measured       {:>9}   = {} private + {} shared with clones/snapshots (counted once)",
        bytes(m.measured()),
        bytes(t.private),
        bytes(m.scan.shared_once)
    );
    if m.scan.shared_uncertain > 0 {
        println!(
            "  uncertain   up to {:>6}   possibly cloned files whose layout couldn't be read: their \
             shared blocks may be counted twice",
            bytes(m.scan.shared_uncertain)
        );
    }
    if t.linked_elsewhere > 0 {
        println!(
            "  linked out     {:>9}   hard-linked files also linked from outside: not freed by \
             deleting this folder",
            bytes(t.linked_elsewhere)
        );
    }
    if whole_volume {
        println!(
            "  not measured   {:>9}   root-only files, file-system metadata, snapshot-only blocks",
            signed(m.unmeasured())
        );
    }
    if t.dataless_logical > 0 {
        println!(
            "  cloud-only     {:>9}   listed in {} placeholders, not on this disk (not counted)",
            bytes(t.dataless_logical),
            t.dataless_files
        );
    }
    if t.purgeable > 0 {
        println!(
            "  purgeable      {:>9}   macOS can delete these by itself when space runs low",
            bytes(t.purgeable)
        );
    }
    if t.unreadable_dirs > 0 {
        println!(
            "  unreadable     {:>9}   folders (permissions; Full Disk Access shows more), e.g. {}",
            t.unreadable_dirs,
            m.scan
                .unreadable
                .first()
                .map(|p| printable(&m.display_path(p)).into_owned())
                .unwrap_or_default()
        );
    }

    println!();
    println!(
        "{:>10} {:>10} {:>10} {:>10}",
        "ON DISK", "ONCE", "PRIVATE", "CLOUD-ONLY"
    );
    print_node(m, &m.scan.root, 0, depth, true);
    println!();
    println!(
        "ON DISK = allocated blocks, what du and Finder add up (a clone is counted once per copy, a \
         hard link once)."
    );
    println!(
        "ONCE = what it really takes: the blocks its clones share counted once (Chrome leaves \
         copies of itself that share all of theirs)."
    );
    println!(
        "PRIVATE = freed immediately if deleted (not shared with a clone, a hard link elsewhere \
         or a snapshot)."
    );
    // A local snapshot shares every block that existed when it was taken.
    if !m.snapshots.is_empty() && t.private < m.measured() / 10 {
        println!(
            "          Almost nothing is private now because a local Time Machine snapshot holds \
             the same blocks: deleting files frees their space only once the snapshot is \
             released (`tmutil deletelocalsnapshots`, or automatically within a day)."
        );
    }
}

fn print_node(m: &DiskMap, n: &Node, level: usize, depth: usize, root: bool) {
    let t = &n.totals;
    let name = if root {
        m.display_path(&n.path)
    } else {
        n.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let cloud = if t.dataless_logical > 0 {
        bytes(t.dataless_logical)
    } else {
        String::new()
    };
    println!(
        "{:>10} {:>10} {:>10} {:>10}  {}{}",
        bytes(t.allocated),
        bytes(t.allocated - t.twice),
        bytes(t.private),
        cloud,
        "  ".repeat(level),
        printable(&name)
    );
    if level >= depth {
        return;
    }
    for c in &n.children {
        print_node(m, c, level + 1, depth, false);
    }
    let shown: u64 = n.children.iter().map(|c| c.totals.allocated).sum();
    let rest = t.allocated.saturating_sub(shown);
    if !n.children.is_empty() && rest >= 1_000_000_000 {
        println!(
            "{:>10} {:>10} {:>10} {:>10}  {}(files here and smaller folders)",
            bytes(rest),
            "",
            "",
            "",
            "  ".repeat(level + 1)
        );
    }
}
