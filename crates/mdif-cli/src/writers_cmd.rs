//! `mdif writers`: which processes write to disk.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Duration;

use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_macos::proc;

pub fn writers(secs: u64, top: usize) {
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
            printable(&p.name)
        );
    }

    let mut total: Vec<&proc::ProcIo> = after.iter().collect();
    total.sort_by_key(|p| Reverse(p.bytes_written));
    println!("\nMost written since they started:");
    for p in total.iter().take(top) {
        println!(
            "  {:>9}  {:>7}  {}",
            bytes(p.bytes_written),
            p.pid,
            printable(&p.name)
        );
    }
}
