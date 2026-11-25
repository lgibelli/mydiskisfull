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
        printable(&c.container_reference),
        bytes(c.capacity_ceiling),
        bytes(c.used()),
        bytes(c.capacity_free)
    );
    let mut vols: Vec<_> = c.volumes.iter().collect();
    vols.sort_by_key(|v| Reverse(v.capacity_in_use));
    let vols: Vec<String> = vols
        .iter()
        // A volume's name is the user's to choose: shown, not obeyed.
        .map(|v| format!("{} {}", printable(v.label()), bytes(v.capacity_in_use)))
        .collect();
    println!("  volumes: {}", vols.join(" · "));
    if cfg!(not(target_os = "macos")) {
        // Snapshots are a Mac thing here (System Restore points come with the
        // backups page).
    } else if m.snapshots.is_empty() {
        println!("  local Time Machine snapshots: none");
    } else {
        println!(
            "  local Time Machine snapshots: {} (they keep deleted files' space allocated)",
            m.snapshots.len()
        );
    }

    let t = &m.scan.root.totals;
    // The root is the volume's own root, whichever drive: not only the boot volume.
    // As an absolute path, so `D:` (the current folder of D:) is not taken for `D:\`;
    // (A share, a quota and an unknown quota are `quota_limited`: no reconciliation.)
    let trimmed = |p: &str| p.trim_end_matches(['\\', '/']).to_lowercase();
    let absolute = std::path::absolute(&m.root).unwrap_or_else(|_| m.root.clone());
    let whole_volume = !m.quota_limited
        && (m.root == map::boot_volume()
            || trimmed(&absolute.to_string_lossy()) == trimmed(&m.fs.mount_point));
    println!();
    println!(
        "{} volume: {} in use",
        m.volume.label(),
        bytes(m.volume.capacity_in_use)
    );
    let mac = cfg!(target_os = "macos");
    println!(
        "  measured       {:>9}   = {} private + {} {}",
        bytes(m.measured()),
        bytes(t.private),
        bytes(m.scan.shared_once),
        if mac {
            "shared with clones/snapshots (counted once)"
        } else {
            "not freed by deleting: other names or held open (counted once)"
        }
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
    if t.links_unknown > 0 {
        println!(
            "  names unknown  {:>9}   files that couldn't be opened to count their names (the page \
             file and others Windows holds open): counted once, not as freed by deleting",
            bytes(t.links_unknown)
        );
    }
    if whole_volume {
        let unmeasured = m.unmeasured();
        if unmeasured < 0 && !mac {
            // Found more than is in use: what a file under 1 MiB with several
            // names (Windows keeps thousands, in WinSxS) adds once per name.
            println!(
                "  counted twice  {:>9}   at least: small files with several names (hard links, as in \
                 Windows' component store) are counted once per name",
                bytes(unmeasured.unsigned_abs())
            );
        } else {
            println!(
                "  not measured   {:>9}   {}",
                signed(unmeasured),
                if mac {
                    "root-only files, file-system metadata, snapshot-only blocks"
                } else {
                    "files only an administrator can read, file-system metadata, restore points"
                }
            );
        }
    }
    if t.dataless_logical > 0 {
        println!(
            "  cloud files    {:>9}   the full size of {} placeholders; only what is on this disk is counted",
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
            "  unreadable     {:>9}   folders ({}), e.g. {}",
            t.unreadable_dirs,
            if cfg!(target_os = "macos") {
                "permissions; Full Disk Access shows more"
            } else {
                "permissions"
            },
            m.scan
                .unreadable
                .first()
                .map(|p| printable(&m.display_path(p)).into_owned())
                .unwrap_or_default()
        );
    }

    for note in &m.notes {
        println!("  note: {note}");
    }
    println!();
    println!(
        "{:>10} {:>10} {:>10} {:>10}",
        "ON DISK", "ONCE", "PRIVATE", "CLOUD-ONLY"
    );
    print_node(m, &m.scan.root, 0, depth, true);
    println!();
    if mac {
        println!(
            "ON DISK = allocated blocks, what du and Finder add up (a clone is counted once per \
             copy, a hard link once)."
        );
        println!(
            "ONCE = what it really takes: the blocks its clones share counted once (Chrome leaves \
             copies of itself that share all of theirs)."
        );
        println!(
            "PRIVATE = freed immediately if deleted (not shared with a clone, a hard link \
             elsewhere or a snapshot)."
        );
    } else {
        println!(
            "ON DISK = allocated clusters (a hard link of 1 MiB or more counted once, a smaller \
             one once per name)."
        );
        println!("ONCE = what it really takes: on NTFS the same, as files don't share clusters.");
        println!(
            "PRIVATE = freed if deleted (not a file with another name elsewhere, nor one whose \
             names couldn't be counted)."
        );
    }
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
