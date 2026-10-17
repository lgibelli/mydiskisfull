//! `mdif`: the MyDiskIsFull command line. Honest disk-space tooling for macOS.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use mdif_core::map::{self, DiskMap};
use mdif_core::scan::{Node, Options};
use mdif_core::units::{bytes, signed};
use mdif_macos::proc;

#[derive(Parser)]
#[command(
    name = "mdif",
    version,
    about = "MyDiskIsFull: honest disk-space map and tools for macOS"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Where the disk space is, reconciled with what APFS reports
    Map {
        /// Directory to scan (default: the whole Data volume)
        #[arg(long, default_value = map::DATA_ROOT)]
        root: PathBuf,
        /// Levels of the tree to print
        #[arg(long, default_value_t = 3)]
        depth: usize,
        /// Hide subtrees smaller than this many MB
        #[arg(long, default_value_t = 500)]
        min_mb: u64,
        /// Print the full result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Which of your processes are writing to disk, and how fast
    Writers {
        /// Sampling interval in seconds
        #[arg(long, default_value_t = 10)]
        secs: u64,
        /// How many processes to show
        #[arg(long, default_value_t = 10)]
        top: usize,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Map {
            root,
            depth,
            min_mb,
            json,
        } => {
            let opts = Options {
                max_depth: depth.max(1),
                min_node: min_mb * 1_000_000,
            };
            let m = map::build(&root, &opts)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&m)?);
            } else {
                print_map(&m, depth);
            }
        }
        Cmd::Writers { secs, top } => writers(secs, top),
    }
    Ok(())
}

fn print_map(m: &DiskMap, depth: usize) {
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
                .map(|p| m.display_path(p))
                .unwrap_or_default()
        );
    }

    println!();
    println!("{:>10} {:>10} {:>10}", "ON DISK", "PRIVATE", "CLOUD-ONLY");
    print_node(m, &m.scan.root, 0, depth, true);
    println!();
    println!(
        "ON DISK = allocated blocks, what du and Finder add up (a clone is counted once per copy)."
    );
    println!("PRIVATE = freed immediately if deleted (not shared with a clone or a snapshot).");
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
        "{:>10} {:>10} {:>10}  {}{}",
        bytes(t.allocated),
        bytes(t.private),
        cloud,
        "  ".repeat(level),
        name
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
            "{:>10} {:>10} {:>10}  {}(files here and smaller folders)",
            bytes(rest),
            "",
            "",
            "  ".repeat(level + 1)
        );
    }
}

fn writers(secs: u64, top: usize) {
    let before: HashMap<i32, proc::ProcIo> = proc::own_processes()
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
    std::thread::sleep(Duration::from_secs(secs));
    let after = proc::own_processes();

    let mut now: Vec<(u64, &proc::ProcIo)> = after
        .iter()
        .filter_map(|p| {
            let b = before.get(&p.pid).filter(|b| b.name == p.name)?;
            Some((p.bytes_written.saturating_sub(b.bytes_written), p))
        })
        .collect();
    now.sort_by_key(|&(d, _)| Reverse(d));
    println!("Writing now ({secs} s sample, your processes):");
    for (d, p) in now.iter().take(top).filter(|(d, _)| *d > 0) {
        println!(
            "  {:>9}/s  {:>7}  {}",
            bytes(d / secs.max(1)),
            p.pid,
            p.name
        );
    }

    let mut total: Vec<&proc::ProcIo> = after.iter().collect();
    total.sort_by_key(|p| Reverse(p.bytes_written));
    println!("\nMost written since they started:");
    for p in total.iter().take(top) {
        println!("  {:>9}  {:>7}  {}", bytes(p.bytes_written), p.pid, p.name);
    }
}
